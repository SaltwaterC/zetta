//! The per-process control server: the listener thread, the connection
//! handlers, and the completion waits each command's response depends on.
//!
//! A request arrives as a [`ControlRequest`] on the socket, is decoded by
//! `decode.rs`, is sent to the application as a [`ProcessControlCommand`], and
//! is answered here once the handler in `startup/process_control_loop.rs`
//! reports back over the command's `completion` channel.

use super::*;

use std::cell::Cell;
use std::sync::atomic::AtomicUsize;

use super::decode::handle_control_request;
use super::endpoint::{
    control_endpoint_path, control_socket_path, random_hex, remove_socket_if_present,
    restrict_socket_permissions, write_endpoint,
};

/// How many control connections are served at once.
///
/// Each one is a thread, two for a `zetta pane wait`, and a wait lives as long
/// as the command it wraps. A tab holds at most [`crate::pane::MAX_PANES_PER_TAB`]
/// panes, so this admits four full tabs of waiting commands at the same time,
/// on top of the short requests that shell hooks and `zetta` subcommands make.
/// It bounds the threads one local process can make this one start to about
/// five hundred, most of their stacks never touched.
const MAX_CONTROL_CONNECTIONS: usize = 256;

/// How many commands this server may have queued for the application that the
/// application has not taken yet.
///
/// The main thread drains the queue between frames; it only backs up while
/// that thread is busy, opening a window or restoring a session. Twice a full
/// tab of panes reporting at once fits, and a request can carry up to
/// `MAX_CONTROL_MESSAGE_BYTES`, so what is queued stays near 32 MiB at worst.
/// Past it a request is refused with `process_busy` rather than queued.
const MAX_QUEUED_CONTROL_COMMANDS: usize = 128;

/// How long a connection has, from being accepted, to deliver its whole
/// request.
///
/// A client writes its request in one go as soon as it connects, and gives up
/// itself after [`CONTROL_CLIENT_TIMEOUT`], so a request that has not arrived
/// by then has no one waiting for its answer. The per-read timeout alone let a
/// peer that sent a byte a second hold a connection for days. Only the request
/// is bounded: a `zetta pane wait` keeps its connection afterwards for as long
/// as its command runs.
const CONTROL_REQUEST_DEADLINE: Duration = CONTROL_CLIENT_TIMEOUT;

/// The error code a request refused for lack of capacity carries.
const PROCESS_BUSY: &str = "process_busy";

/// The limits above, gathered so a test can start a server with small ones.
#[derive(Clone, Copy, Debug)]
pub(super) struct ControlLimits {
    pub(super) connections: usize,
    pub(super) queued_commands: usize,
    pub(super) request_deadline: Duration,
}

impl ControlLimits {
    pub(super) const DEFAULT: Self = Self {
        connections: MAX_CONTROL_CONNECTIONS,
        queued_commands: MAX_QUEUED_CONTROL_COMMANDS,
        request_deadline: CONTROL_REQUEST_DEADLINE,
    };
}

/// How many commands this server has put on the application's queue that the
/// application has not taken off it yet.
///
/// The queue itself is unbounded and shared with senders that are not this
/// server (opened URLs, the Windows terminal handoff), so the bound is kept
/// here, and the application loop releases a place as it takes each command
/// that came from the socket — see [`ProcessControlCommand::came_from_control_socket`].
#[derive(Clone, Default)]
pub(crate) struct QueuedControlCommands(Arc<AtomicUsize>);

impl QueuedControlCommands {
    /// Records that the application has taken one command off its queue.
    pub(crate) fn release(&self) {
        let _ = self
            .0
            .try_update(Ordering::AcqRel, Ordering::Acquire, |queued| {
                queued.checked_sub(1)
            });
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.0.load(Ordering::Acquire)
    }
}

/// Why a command did not reach the application.
enum QueueRefusal {
    /// The queue holds as many commands from this server as it may.
    Full,
    /// The application has stopped taking commands.
    Closed,
}

/// The application's command queue, as this server sends to it.
struct ControlQueue {
    sender: UnboundedSender<ProcessControlCommand>,
    queued: QueuedControlCommands,
    capacity: usize,
}

impl ControlQueue {
    fn send(&self, command: ProcessControlCommand) -> std::result::Result<(), QueueRefusal> {
        // Counted before it is sent, so the application taking it at once can
        // never release a place that was not yet claimed.
        self.queued
            .0
            .try_update(Ordering::AcqRel, Ordering::Acquire, |queued| {
                (queued < self.capacity).then_some(queued + 1)
            })
            .map_err(|_| QueueRefusal::Full)?;
        self.sender.unbounded_send(command).map_err(|_| {
            self.queued.release();
            QueueRefusal::Closed
        })
    }
}

/// A claim on one of [`ControlLimits::connections`], given back when dropped.
struct ConnectionSlot(Arc<AtomicUsize>);

impl ConnectionSlot {
    fn claim(in_use: &Arc<AtomicUsize>, capacity: usize) -> Option<Self> {
        in_use
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < capacity).then_some(count + 1)
            })
            .ok()
            .map(|_| Self(Arc::clone(in_use)))
    }
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// What every connection thread shares.
struct ControlServerState {
    queue: ControlQueue,
    token: String,
    stopping: Arc<AtomicBool>,
    limits: ControlLimits,
    connections: Arc<AtomicUsize>,
}

pub(crate) struct ProcessControlServer {
    endpoint_path: PathBuf,
    socket_path: PathBuf,
    stopping: Arc<AtomicBool>,
    queued: QueuedControlCommands,
    thread: Option<thread::JoinHandle<()>>,
}

impl ProcessControlServer {
    pub(crate) fn start(commands: UnboundedSender<ProcessControlCommand>) -> Result<Self> {
        Self::start_at(commands, control_endpoint_path(std::process::id()))
    }

    pub(super) fn start_at(
        commands: UnboundedSender<ProcessControlCommand>,
        endpoint_path: PathBuf,
    ) -> Result<Self> {
        Self::start_with_limits(commands, endpoint_path, ControlLimits::DEFAULT)
    }

    pub(super) fn start_with_limits(
        commands: UnboundedSender<ProcessControlCommand>,
        endpoint_path: PathBuf,
        limits: ControlLimits,
    ) -> Result<Self> {
        let parent = endpoint_path
            .parent()
            .context("control endpoint has no parent")?;
        crate::private_fs::create_private_dir(parent)?;
        let socket_path = control_socket_path(&endpoint_path);
        remove_socket_if_present(&socket_path)?;
        let listener = UnixListener::bind(&socket_path)
            .context("binding the Zetta process control listener")?;
        // Connecting to a Unix socket requires write permission on it, so the
        // umask alone decides who may reach this listener. Do not leave that to
        // the environment: the endpoint token is the only other gate.
        restrict_socket_permissions(&socket_path)?;
        let token = random_hex(32).context("generating the Zetta process control token")?;
        let endpoint = ControlEndpoint {
            version: CONTROL_VERSION,
            process_id: std::process::id(),
            socket_path: socket_path.clone(),
            token: token.clone(),
        };
        write_endpoint(&endpoint_path, &endpoint)?;
        let stopping = Arc::new(AtomicBool::new(false));
        let queued = QueuedControlCommands::default();
        let state = Arc::new(ControlServerState {
            queue: ControlQueue {
                sender: commands,
                queued: queued.clone(),
                capacity: limits.queued_commands,
            },
            token,
            stopping: stopping.clone(),
            limits,
            connections: Arc::new(AtomicUsize::new(0)),
        });
        let thread = thread::Builder::new()
            .name("zetta-process-control".to_owned())
            .spawn(move || accept_control_connections(&listener, &state))
            .context("starting the Zetta process control thread")?;
        Ok(Self {
            endpoint_path,
            socket_path,
            stopping,
            queued,
            thread: Some(thread),
        })
    }

    pub(crate) fn is_accepting(&self) -> bool {
        !self.stopping.load(Ordering::Acquire)
    }

    /// The count the application loop releases as it takes this server's
    /// commands off its queue.
    pub(crate) fn queued_commands(&self) -> &QueuedControlCommands {
        &self.queued
    }

    pub(crate) fn begin_shutdown(&self) {
        if self.stopping.swap(true, Ordering::AcqRel) {
            return;
        }
        process_run_registry().shutdown();
        // Stop advertising this process before GPUI begins shutting down. A new
        // launch must start its own application instead of handing off to a
        // process that can no longer keep the requested window alive.
        let _ = fs::remove_file(&self.endpoint_path);
        let _ = UnixStream::connect(&self.socket_path);
        let _ = fs::remove_file(&self.socket_path);
    }
}

/// Accepts control connections until the server begins shutting down.
///
/// Each connection is served on its own thread: a `zetta pane wait` connection
/// stays open for the life of the command it wraps, so serving connections in
/// turn would let one wait block every other request. A connection past
/// [`ControlLimits::connections`] is answered `process_busy` and closed here,
/// rather than left in the kernel's backlog: a backlog would hold the user's
/// own next request behind whatever filled it, where a refusal lets the client
/// report it at once and the listener stays free for the next connection.
fn accept_control_connections(listener: &UnixListener, state: &Arc<ControlServerState>) {
    for stream in listener.incoming() {
        if state.stopping.load(Ordering::Acquire) {
            break;
        }
        let Ok(stream) = stream else {
            continue;
        };
        let accepted = Instant::now();
        let Some(slot) = ConnectionSlot::claim(&state.connections, state.limits.connections) else {
            refuse_busy_connection(stream);
            continue;
        };
        let state = Arc::clone(state);
        let _ = thread::Builder::new()
            .name("zetta-process-control-request".to_owned())
            .spawn(move || {
                let _slot = slot;
                serve_control_connection(stream, &state, accepted);
            });
    }
}

/// Answers a connection there is no room for without reading from it, which
/// is why it can be done on the accepting thread: a fresh socket's send buffer
/// takes the short reply without waiting.
fn refuse_busy_connection(mut stream: UnixStream) {
    let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
    let _ = write_message(
        &mut stream,
        &refusal_response(
            "rejected",
            PROCESS_BUSY,
            "the Zetta process is serving too many control connections",
        ),
    );
}

/// A response that carries only a status and a structured error.
fn refusal_response(status: &str, code: &str, message: &str) -> ControlResponse {
    ControlResponse {
        status: status.to_owned(),
        run_id: None,
        themes: Vec::new(),
        pane_theme: None,
        pane_theme_revision: None,
        silent_mode: false,
        pane_labels: Vec::new(),
        error: Some(ControlError {
            code: code.to_owned(),
            message: message.to_owned(),
        }),
    }
}

/// Serves one connection: decode the request, apply it, answer it.
fn serve_control_connection(mut stream: UnixStream, state: &ControlServerState, accepted: Instant) {
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    let mut response = ControlResponse {
        status: String::new(),
        run_id: None,
        themes: Vec::new(),
        pane_theme: None,
        pane_theme_revision: None,
        silent_mode: false,
        pane_labels: Vec::new(),
        error: None,
    };
    let dispatch = ControlDispatch {
        queue: &state.queue,
        stopping: &state.stopping,
        refused_full: Cell::new(false),
    };
    let deadline = accepted + state.limits.request_deadline;
    let Some(status) = apply_control_request(
        &mut stream,
        &dispatch,
        &state.token,
        deadline,
        &mut response,
    ) else {
        // `run_wait` keeps the connection and answers on it itself.
        return;
    };
    if dispatch.refused_full.get() && response.error.is_none() {
        response.error = Some(queue_full_error());
    }
    // A request that was applied while the process was quitting is reported as
    // refused, so a client cannot be told a change landed that the closing
    // window may not have kept.
    response.status = if status == "ok" && state.stopping.load(Ordering::Acquire) {
        "rejected".to_owned()
    } else {
        status.to_owned()
    };
    let _ = write_message(&mut stream, &response);
}

const QUEUE_FULL_MESSAGE: &str = "the Zetta process has too many control requests waiting for it";

fn queue_full_error() -> ControlError {
    ControlError {
        code: PROCESS_BUSY.to_owned(),
        message: QUEUE_FULL_MESSAGE.to_owned(),
    }
}

/// The queue and the shutdown flag every arm of [`apply_control_request`]
/// dispatches through, bound once so an arm names only the command it builds.
struct ControlDispatch<'a> {
    queue: &'a ControlQueue,
    stopping: &'a AtomicBool,
    /// Set when a command was refused because the queue was full, so the
    /// response can say so rather than report a bare rejection.
    refused_full: Cell<bool>,
}

impl ControlDispatch<'_> {
    /// Puts a command on the application's queue, reporting whether it went.
    fn submit(&self, command: ProcessControlCommand) -> bool {
        match self.queue.send(command) {
            Ok(()) => true,
            Err(QueueRefusal::Full) => {
                self.refused_full.set(true);
                false
            }
            Err(QueueRefusal::Closed) => false,
        }
    }

    /// Sends a command and waits for the window to acknowledge it.
    fn send(&self, build: impl FnOnce(Sender<bool>) -> ProcessControlCommand) -> &'static str {
        dispatch_control_command(self, build)
    }

    /// Acknowledge when the application queue accepts the command.
    fn queue(&self, build: impl FnOnce(Sender<bool>) -> ProcessControlCommand) -> &'static str {
        let (completion, _) = channel();
        if self.stopping.load(Ordering::Acquire) || !self.submit(build(completion)) {
            "rejected"
        } else {
            "ok"
        }
    }

    /// The same, for a command that answers with a value written into
    /// `response`, or the reason it refused.
    fn send_for<T>(
        &self,
        response: &mut ControlResponse,
        code: &str,
        build: impl FnOnce(Sender<std::result::Result<T, String>>) -> ProcessControlCommand,
        apply: impl FnOnce(T, &mut ControlResponse),
    ) -> &'static str {
        dispatch_control_result(self, response, code, build, apply)
    }
}

/// Applies one decoded request, filling in whatever the response carries
/// beyond its status, and returns that status.
///
/// `None` means the request took the connection over and has answered on it
/// already; only `run_wait` does that, because it answers twice — once when the
/// wait is registered and once when the command it wraps has run.
fn apply_control_request(
    stream: &mut UnixStream,
    dispatch: &ControlDispatch<'_>,
    token: &str,
    deadline: Instant,
    response: &mut ControlResponse,
) -> Option<&'static str> {
    let Some(command) = handle_control_request(stream, token, Some(deadline)) else {
        // An unauthenticated or undecodable request is refused on the
        // connection rather than dropped, so a client is told rather than left
        // waiting on a socket that closed.
        return Some("rejected");
    };
    // `run_wait` takes the connection over rather than answering with a status
    // — the client blocks on it until the command it is waiting for finishes —
    // so it is handled here rather than in a group.
    if let ControlRequestCommand::RunWait { request } = command {
        serve_run_wait_connection(stream, dispatch, token, request);
        return None;
    }
    // Exhaustive, so a new command has to be given a group rather than falling
    // through. The groups mirror `decode.rs`'s, so a command is added to the
    // same-named function in both files; each group re-matches only its own
    // variants, which is why they end in an `unreachable!` this match rules out.
    Some(match &command {
        ControlRequestCommand::ReloadConfiguration { .. }
        | ControlRequestCommand::OpenWindow
        | ControlRequestCommand::OpenNewWindow { .. }
        | ControlRequestCommand::OpenProject { .. }
        | ControlRequestCommand::ReloadProjects => apply_window_command(command, dispatch),
        ControlRequestCommand::ReplacePane { .. }
        | ControlRequestCommand::OpenCommand { .. }
        | ControlRequestCommand::RunPane { .. }
        | ControlRequestCommand::RunShellCommand { .. }
        | ControlRequestCommand::RunComplete { .. }
        | ControlRequestCommand::ListPaneLabels { .. } => {
            apply_pane_command(command, dispatch, response)
        }
        ControlRequestCommand::ReconnectSession { .. }
        | ControlRequestCommand::OpenRemoteSession { .. }
        | ControlRequestCommand::ResumeDiskSession { .. } => {
            apply_session_command(command, dispatch)
        }
        ControlRequestCommand::SetTabIcon { .. }
        | ControlRequestCommand::ResetTabIcon { .. }
        | ControlRequestCommand::SetTheme { .. }
        | ControlRequestCommand::ListThemes
        | ControlRequestCommand::GetPaneTheme { .. }
        | ControlRequestCommand::SetPaneOverlay { .. } => {
            apply_appearance_command(command, dispatch, response)
        }
        // Already returned above; named so this match stays exhaustive and a new
        // command still has to be given a group.
        ControlRequestCommand::RunWait { .. } => return None,
        ControlRequestCommand::SetTabAttention { .. }
        | ControlRequestCommand::FocusTab { .. }
        | ControlRequestCommand::SetTabName { .. }
        | ControlRequestCommand::SetWorktreeName { .. }
        | ControlRequestCommand::GetSilentMode { .. } => {
            apply_tab_command(command, dispatch, response)
        }
    })
}

/// Configuration, window and project commands.
///
/// Only reachable for the variants [`apply_control_request`] routes here, which
/// is what the final arm relies on.
fn apply_window_command(
    command: ControlRequestCommand,
    dispatch: &ControlDispatch<'_>,
) -> &'static str {
    match command {
        ControlRequestCommand::ReloadConfiguration { config_path } => {
            let (completion, completed) = channel();
            let sent = dispatch.submit(ProcessControlCommand::ReloadConfiguration {
                config_path,
                completion,
            });
            if sent && wait_for_reload_completion(&completed, dispatch.stopping) {
                "ok"
            } else {
                "rejected"
            }
        }
        ControlRequestCommand::OpenWindow => {
            dispatch.send(|completion| ProcessControlCommand::OpenWindow { completion })
        }
        ControlRequestCommand::OpenNewWindow {
            profile,
            activation_token,
        } => dispatch.send(|completion| ProcessControlCommand::OpenNewWindow {
            profile,
            activation_token,
            completion,
        }),
        ControlRequestCommand::OpenProject {
            root,
            working_directory,
        } => dispatch.send(|completion| ProcessControlCommand::OpenProject {
            root,
            working_directory,
            completion,
        }),
        ControlRequestCommand::ReloadProjects => {
            dispatch.send(|completion| ProcessControlCommand::ReloadProjects { completion })
        }
        _ => unreachable!("apply_control_request routes only window dispatch.commands here"),
    }
}

/// Commands about a pane: what runs in it, and what it reports back.
///
/// Only reachable for the variants [`apply_control_request`] routes here, which
/// is what the final arm relies on.
fn apply_pane_command(
    command: ControlRequestCommand,
    dispatch: &ControlDispatch<'_>,
    response: &mut ControlResponse,
) -> &'static str {
    match command {
        ControlRequestCommand::ReplacePane {
            split,
            profile,
            theme,
        } => dispatch.send(|completion| ProcessControlCommand::ReplacePane {
            request: ReplacePaneRequest {
                split,
                profile,
                theme,
            },
            completion,
        }),
        ControlRequestCommand::OpenCommand {
            request,
            working_directory,
        } => dispatch.send(|completion| ProcessControlCommand::OpenCommand {
            request,
            working_directory,
            completion,
        }),
        ControlRequestCommand::RunPane { request } => dispatch.send_for(
            response,
            "pane_rejected",
            |completion| ProcessControlCommand::RunPane {
                request,
                completion,
            },
            |(), _| {},
        ),
        ControlRequestCommand::RunShellCommand { request } => dispatch.send_for(
            response,
            "shell_command_rejected",
            |completion| ProcessControlCommand::RunShellCommand {
                request,
                completion,
            },
            |(), _| {},
        ),
        ControlRequestCommand::RunComplete { .. } => "rejected",
        ControlRequestCommand::ListPaneLabels { attention_id } => dispatch.send_for(
            response,
            "pane_list_rejected",
            |completion| ProcessControlCommand::ListPaneLabels {
                attention_id,
                completion,
            },
            |labels, response| response.pane_labels = labels,
        ),
        _ => unreachable!("apply_control_request routes only pane dispatch.commands here"),
    }
}

/// Commands that attach a session to this window.
///
/// Only reachable for the variants [`apply_control_request`] routes here, which
/// is what the final arm relies on.
fn apply_session_command(
    command: ControlRequestCommand,
    dispatch: &ControlDispatch<'_>,
) -> &'static str {
    match command {
        ControlRequestCommand::ReconnectSession {
            runner_id,
            session_id,
            attention_id,
            secret,
        } => dispatch_reconnect_command(dispatch, |completion| {
            ProcessControlCommand::ReconnectSession {
                runner_id,
                session_id,
                attention_id,
                secret,
                completion,
            }
        }),
        ControlRequestCommand::OpenRemoteSession {
            target,
            port,
            session_id,
            secret,
            protocol,
            keep_alive_ms,
            forward_agent,
        } => dispatch_reconnect_command(dispatch, |completion| {
            ProcessControlCommand::OpenRemoteSession {
                target,
                port,
                session_id,
                secret,
                protocol,
                keep_alive_ms,
                forward_agent,
                completion,
            }
        }),
        ControlRequestCommand::ResumeDiskSession {
            session_id,
            identity_paths,
            identity_passphrases,
            secret,
        } => dispatch_reconnect_command(dispatch, |completion| {
            ProcessControlCommand::ResumeDiskSession {
                session_id,
                identity_paths,
                identity_passphrases,
                secret,
                completion,
            }
        }),
        _ => unreachable!("apply_control_request routes only session dispatch.commands here"),
    }
}

/// Commands that change how a tab or pane looks.
///
/// Only reachable for the variants [`apply_control_request`] routes here, which
/// is what the final arm relies on.
fn apply_appearance_command(
    command: ControlRequestCommand,
    dispatch: &ControlDispatch<'_>,
    response: &mut ControlResponse,
) -> &'static str {
    match command {
        ControlRequestCommand::SetTabIcon {
            attention_id,
            icon,
            queue,
        } => {
            let build = |completion| ProcessControlCommand::SetTabIcon {
                attention_id,
                icon,
                completion,
            };
            if queue {
                dispatch.queue(build)
            } else {
                dispatch.send(build)
            }
        }
        ControlRequestCommand::ResetTabIcon {
            attention_id,
            queue,
        } => {
            let build = |completion| ProcessControlCommand::ResetTabIcon {
                attention_id,
                completion,
            };
            if queue {
                dispatch.queue(build)
            } else {
                dispatch.send(build)
            }
        }
        ControlRequestCommand::SetTheme { scope, theme } => {
            dispatch.send(|completion| ProcessControlCommand::SetTheme {
                scope,
                theme,
                completion,
            })
        }
        ControlRequestCommand::ListThemes => {
            let (completion, completed) = channel();
            let accepted = dispatch.submit(ProcessControlCommand::ListThemes { completion });
            match accepted
                .then(|| wait_for_theme_list_completion(&completed, dispatch.stopping))
                .flatten()
            {
                Some(themes) => {
                    response.themes = themes;
                    "ok"
                }
                None => "rejected",
            }
        }
        ControlRequestCommand::GetPaneTheme {
            attention_id,
            pane_id,
            known_revision,
        } => {
            // Answered here, on the connection thread, when the client is
            // already current. `zetta vi` polls this for the life of an open
            // editor; dispatching every poll to the main thread made an idle
            // editor cost two wake-ups a second on the thread that draws.
            let revision = pane_theme_revision();
            response.pane_theme_revision = Some(revision);
            if known_revision == Some(revision) {
                return "unchanged";
            }
            dispatch.send_for(
                response,
                "pane_theme_unavailable",
                |completion| ProcessControlCommand::GetPaneTheme {
                    attention_id,
                    pane_id,
                    completion,
                },
                |theme, response| response.pane_theme = Some(theme),
            )
        }
        ControlRequestCommand::SetPaneOverlay {
            text,
            font_size,
            opacity,
            color,
        } => {
            let color = color.and_then(|value| overlay_color_from_value(&value));
            dispatch.send(|completion| ProcessControlCommand::SetPaneOverlay {
                text,
                font_size,
                opacity,
                color,
                completion,
            })
        }
        _ => unreachable!("apply_control_request routes only appearance dispatch.commands here"),
    }
}

/// Commands that address a tab by name or attention id.
///
/// Only reachable for the variants [`apply_control_request`] routes here, which
/// is what the final arm relies on.
fn apply_tab_command(
    command: ControlRequestCommand,
    dispatch: &ControlDispatch<'_>,
    response: &mut ControlResponse,
) -> &'static str {
    match command {
        ControlRequestCommand::SetTabAttention {
            attention_id,
            summary,
            body,
        } => dispatch.send(|completion| ProcessControlCommand::SetTabAttention {
            request: TabAttentionRequest {
                attention_id,
                summary,
                body,
            },
            completion,
        }),
        ControlRequestCommand::FocusTab { attention_id } => {
            dispatch.send(|completion| ProcessControlCommand::FocusTab {
                attention_id,
                completion,
            })
        }
        ControlRequestCommand::SetTabName { attention_id, name } => {
            dispatch.send(|completion| ProcessControlCommand::SetTabName {
                request: TabNameRequest { attention_id, name },
                completion,
            })
        }
        ControlRequestCommand::SetWorktreeName { attention_id, name } => {
            dispatch.send(|completion| ProcessControlCommand::SetWorktreeName {
                request: WorktreeNameRequest { attention_id, name },
                completion,
            })
        }
        ControlRequestCommand::GetSilentMode { attention_id } => {
            let (completion, completed) = channel();
            if !dispatch.submit(ProcessControlCommand::GetSilentMode {
                attention_id,
                completion,
            }) {
                "rejected"
            } else if let Some(silent_mode) =
                wait_for_silent_mode_completion(&completed, dispatch.stopping)
            {
                response.silent_mode = silent_mode;
                "ok"
            } else {
                "rejected"
            }
        }
        _ => unreachable!("apply_control_request routes only tab dispatch.commands here"),
    }
}

/// Sends a command built around a fresh completion channel and waits for the
/// window to answer it.
///
/// Every command whose answer is only "did it happen" goes through here: the
/// send and the wait have to stay together, because a command that was never
/// sent must not be waited for.
fn dispatch_control_command(
    dispatch: &ControlDispatch<'_>,
    build: impl FnOnce(Sender<bool>) -> ProcessControlCommand,
) -> &'static str {
    let (completion, completed) = channel();
    let accepted = dispatch.submit(build(completion))
        && wait_for_control_completion(&completed, dispatch.stopping);
    if accepted { "ok" } else { "rejected" }
}

/// The same, for a command that answers with a value or the reason it refused.
///
/// The reason becomes the response's structured error under `code`, so a client
/// can report why rather than just that the request was rejected.
fn dispatch_control_result<T>(
    dispatch: &ControlDispatch<'_>,
    response: &mut ControlResponse,
    code: &str,
    build: impl FnOnce(Sender<std::result::Result<T, String>>) -> ProcessControlCommand,
    apply: impl FnOnce(T, &mut ControlResponse),
) -> &'static str {
    let (completion, completed) = channel();
    if !dispatch.submit(build(completion)) {
        return "rejected";
    }
    match wait_for_result_completion(&completed, dispatch.stopping) {
        Some(Ok(value)) => {
            apply(value, response);
            "ok"
        }
        Some(Err(message)) => {
            response.error = Some(ControlError {
                code: code.to_owned(),
                message,
            });
            "rejected"
        }
        None => "rejected",
    }
}

/// The same, for the three commands that answer with a
/// [`ReconnectSessionResult`]: their status distinguishes "the secret was
/// wrong" from "no such session", which a bare rejection cannot.
fn dispatch_reconnect_command(
    dispatch: &ControlDispatch<'_>,
    build: impl FnOnce(Sender<ReconnectSessionResult>) -> ProcessControlCommand,
) -> &'static str {
    let (completion, completed) = channel();
    let result = if dispatch.submit(build(completion)) {
        wait_for_reconnect_completion(&completed, dispatch.stopping)
    } else {
        ReconnectSessionResult::Rejected
    };
    reconnect_session_status(result)
}

fn serve_run_wait_connection(
    stream: &mut UnixStream,
    dispatch: &ControlDispatch<'_>,
    token: &str,
    request: RunWaitRequest,
) {
    let stopping = dispatch.stopping;
    let Ok(mut client_stream) = stream.try_clone() else {
        let _ = write_message(
            stream,
            &ControlResponse {
                status: "rejected".to_owned(),
                run_id: None,
                themes: Vec::new(),
                pane_theme: None,
                pane_theme_revision: None,
                silent_mode: false,
                pane_labels: Vec::new(),
                error: Some(ControlError {
                    code: "process_unavailable".to_owned(),
                    message: "the Zetta process could not monitor the run connection".to_owned(),
                }),
            },
        );
        return;
    };
    let _ = client_stream.set_read_timeout(None);
    let (client_request_sender, client_requests) = channel();
    let monitor_token = token.to_owned();
    if thread::Builder::new()
        .name("zetta-run-connection".to_owned())
        .spawn(move || {
            let request = handle_control_request(&mut client_stream, &monitor_token, None);
            let _ = client_request_sender.send(request);
        })
        .is_err()
    {
        let _ = write_message(
            stream,
            &ControlResponse {
                status: "rejected".to_owned(),
                run_id: None,
                themes: Vec::new(),
                pane_theme: None,
                pane_theme_revision: None,
                silent_mode: false,
                pane_labels: Vec::new(),
                error: Some(ControlError {
                    code: "process_unavailable".to_owned(),
                    message: "the Zetta process could not monitor the run connection".to_owned(),
                }),
            },
        );
        return;
    }

    let (completion, completed) = channel();
    if !dispatch.submit(ProcessControlCommand::RunWait {
        request,
        completion,
    }) {
        let response = if dispatch.refused_full.get() {
            refusal_response("rejected", PROCESS_BUSY, QUEUE_FULL_MESSAGE)
        } else {
            refusal_response(
                "rejected",
                "process_unavailable",
                "the Zetta process is no longer accepting run requests",
            )
        };
        let _ = write_message(stream, &response);
        return;
    }

    let registration = match wait_for_run_registration(&completed, stopping, &client_requests) {
        RunWaitRegistration::Registered(Ok(registration)) => registration,
        RunWaitRegistration::Registered(Err(message)) => {
            let _ = write_message(
                stream,
                &ControlResponse {
                    status: "failed".to_owned(),
                    run_id: None,
                    themes: Vec::new(),
                    pane_theme: None,
                    pane_theme_revision: None,
                    silent_mode: false,
                    pane_labels: Vec::new(),
                    error: Some(ControlError {
                        code: "run_rejected".to_owned(),
                        message,
                    }),
                },
            );
            shutdown_run_connection(stream);
            return;
        }
        RunWaitRegistration::ClientDisconnected => return,
        RunWaitRegistration::Stopping => {
            shutdown_run_connection(stream);
            return;
        }
        RunWaitRegistration::NoRegistration => {
            let _ = write_message(
                stream,
                &ControlResponse {
                    status: "rejected".to_owned(),
                    run_id: None,
                    themes: Vec::new(),
                    pane_theme: None,
                    pane_theme_revision: None,
                    silent_mode: false,
                    pane_labels: Vec::new(),
                    error: Some(ControlError {
                        code: "run_rejected".to_owned(),
                        message: "the Zetta process did not accept the run request".to_owned(),
                    }),
                },
            );
            shutdown_run_connection(stream);
            return;
        }
    };
    let id = registration.id;
    let _ = stream.set_read_timeout(None);
    let Some(resolution) =
        await_run_resolution(stream, stopping, &registration, &client_requests, id)
    else {
        return;
    };
    write_run_resolution(stream, stopping, &client_requests, id, resolution);
}

/// Waits for the run to resolve, giving up if the process is shutting down or
/// the client hangs up first.
///
/// Returns `None` once it has already deregistered the run and closed the
/// connection, which is what every abandonment here has to do: a run left
/// registered would keep a `zetta pane wait` waiting on a client that is gone.
fn await_run_resolution(
    stream: &mut UnixStream,
    stopping: &AtomicBool,
    registration: &crate::run_command::RunRegistration,
    client_requests: &Receiver<Option<ControlRequestCommand>>,
    id: u64,
) -> Option<crate::run_command::RunResolutionMessage> {
    let resolution = loop {
        if stopping.load(Ordering::Acquire) {
            process_run_registry().connection_lost(id);
            shutdown_run_connection(stream);
            return None;
        }
        match registration.recv_timeout(RUN_WAIT_SUPERVISION_INTERVAL) {
            Ok(resolution) => break resolution,
            Err(RecvTimeoutError::Timeout) => match client_requests.try_recv() {
                Ok(_) | Err(TryRecvError::Disconnected) => {
                    process_run_registry().connection_lost(id);
                    shutdown_run_connection(stream);
                    return None;
                }
                Err(TryRecvError::Empty) => {}
            },
            Err(RecvTimeoutError::Disconnected) => {
                process_run_registry().connection_lost(id);
                shutdown_run_connection(stream);
                return None;
            }
        }
    };
    Some(resolution)
}

/// Answers the waiting client with what the run did.
fn write_run_resolution(
    stream: &mut UnixStream,
    stopping: &AtomicBool,
    client_requests: &Receiver<Option<ControlRequestCommand>>,
    id: u64,
    resolution: crate::run_command::RunResolutionMessage,
) {
    match resolution.resolution {
        RunResolution::Failed => {
            let _ = write_message(
                stream,
                &ControlResponse {
                    status: "failed".to_owned(),
                    run_id: Some(id),
                    themes: Vec::new(),
                    pane_theme: None,
                    pane_theme_revision: None,
                    silent_mode: false,
                    pane_labels: Vec::new(),
                    error: Some(ControlError {
                        code: "run_dependency_failed".to_owned(),
                        message: resolution
                            .message
                            .unwrap_or_else(|| "a run dependency failed".to_owned()),
                    }),
                },
            );
            process_run_registry().connection_lost(id);
            shutdown_run_connection(stream);
        }
        RunResolution::Ready => {
            if write_message(
                stream,
                &ControlResponse {
                    status: "ready".to_owned(),
                    run_id: Some(id),
                    themes: Vec::new(),
                    pane_theme: None,
                    pane_theme_revision: None,
                    silent_mode: false,
                    pane_labels: Vec::new(),
                    error: None,
                },
            )
            .is_err()
            {
                process_run_registry().connection_lost(id);
                return;
            }

            let completed = loop {
                if stopping.load(Ordering::Acquire) {
                    process_run_registry().connection_lost(id);
                    shutdown_run_connection(stream);
                    return;
                }
                match client_requests.recv_timeout(RUN_WAIT_SUPERVISION_INTERVAL) {
                    Ok(request) => break request,
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break None,
                }
            };
            match completed {
                Some(ControlRequestCommand::RunComplete {
                    id: completed_id,
                    exit_code,
                }) if completed_id == id => {
                    process_run_registry().complete(id, exit_code);
                    let _ = write_message(
                        stream,
                        &ControlResponse {
                            status: "ok".to_owned(),
                            run_id: Some(id),
                            themes: Vec::new(),
                            pane_theme: None,
                            pane_theme_revision: None,
                            silent_mode: false,
                            pane_labels: Vec::new(),
                            error: None,
                        },
                    );
                }
                _ => process_run_registry().connection_lost(id),
            }
        }
    }
}

fn shutdown_run_connection(stream: &UnixStream) {
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

enum RunWaitRegistration {
    Registered(std::result::Result<RunRegistration, String>),
    ClientDisconnected,
    NoRegistration,
    Stopping,
}

/// Registration is the only part of process control that may legitimately
/// outlive the ordinary two-second request budget: the UI thread can be busy
/// opening a window or restoring a background session. Keep watching the
/// client while it is queued so a dropped wrapper cannot leave a run node
/// owning its pane indefinitely.
fn wait_for_run_registration(
    completed: &Receiver<std::result::Result<RunRegistration, String>>,
    stopping: &AtomicBool,
    client_requests: &Receiver<Option<ControlRequestCommand>>,
) -> RunWaitRegistration {
    let mut client_disconnected = false;
    loop {
        if stopping.load(Ordering::Acquire) {
            return RunWaitRegistration::Stopping;
        }
        match completed.recv_timeout(CONTROL_COMPLETION_POLL_INTERVAL) {
            Ok(registration) => {
                return if client_disconnected {
                    if let Ok(registration) = registration {
                        process_run_registry().connection_lost(registration.id);
                    }
                    RunWaitRegistration::ClientDisconnected
                } else {
                    RunWaitRegistration::Registered(registration)
                };
            }
            Err(RecvTimeoutError::Timeout) if !client_disconnected => {
                match client_requests.try_recv() {
                    Ok(_) | Err(TryRecvError::Disconnected) => {
                        client_disconnected = true;
                    }
                    Err(TryRecvError::Empty) => {}
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return if client_disconnected {
                    RunWaitRegistration::ClientDisconnected
                } else {
                    RunWaitRegistration::NoRegistration
                };
            }
        }
    }
}

/// Reload preparation may legitimately outlast the ordinary control timeout.
/// Completion means applied (or failed), never merely queued. Shutdown and a
/// dropped completion sender still end the wait; this runs on a connection
/// thread, not the GUI thread.
fn wait_for_reload_completion(completed: &Receiver<bool>, stopping: &AtomicBool) -> bool {
    loop {
        if stopping.load(Ordering::Acquire) {
            return false;
        }
        match completed.recv_timeout(RUN_WAIT_SUPERVISION_INTERVAL) {
            Ok(accepted) => return accepted && !stopping.load(Ordering::Acquire),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return false,
        }
    }
}

fn wait_for_control_completion(completed: &Receiver<bool>, stopping: &AtomicBool) -> bool {
    let deadline = Instant::now() + CONTROL_COMPLETION_TIMEOUT;
    loop {
        if stopping.load(Ordering::Acquire) {
            return false;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        match completed.recv_timeout(remaining.min(CONTROL_COMPLETION_POLL_INTERVAL)) {
            Ok(accepted) => return accepted && !stopping.load(Ordering::Acquire),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return false,
        }
    }
}

fn wait_for_silent_mode_completion(
    completed: &Receiver<bool>,
    stopping: &AtomicBool,
) -> Option<bool> {
    let deadline = Instant::now() + CONTROL_COMPLETION_TIMEOUT;
    loop {
        if stopping.load(Ordering::Acquire) {
            return None;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        match completed.recv_timeout(remaining.min(CONTROL_COMPLETION_POLL_INTERVAL)) {
            Ok(silent_mode) => return (!stopping.load(Ordering::Acquire)).then_some(silent_mode),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
}

fn wait_for_theme_list_completion(
    completed: &Receiver<Vec<String>>,
    stopping: &AtomicBool,
) -> Option<Vec<String>> {
    let deadline = Instant::now() + CONTROL_COMPLETION_TIMEOUT;
    loop {
        if stopping.load(Ordering::Acquire) {
            return None;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        match completed.recv_timeout(remaining.min(CONTROL_COMPLETION_POLL_INTERVAL)) {
            Ok(themes) => return (!stopping.load(Ordering::Acquire)).then_some(themes),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
}

fn wait_for_result_completion<T>(
    completed: &Receiver<std::result::Result<T, String>>,
    stopping: &AtomicBool,
) -> Option<std::result::Result<T, String>> {
    let deadline = Instant::now() + CONTROL_COMPLETION_TIMEOUT;
    loop {
        if stopping.load(Ordering::Acquire) {
            return None;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        match completed.recv_timeout(remaining.min(CONTROL_COMPLETION_POLL_INTERVAL)) {
            Ok(result) => return (!stopping.load(Ordering::Acquire)).then_some(result),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
}

fn wait_for_reconnect_completion(
    completed: &Receiver<ReconnectSessionResult>,
    stopping: &AtomicBool,
) -> ReconnectSessionResult {
    let deadline = Instant::now() + RECONNECT_COMPLETION_TIMEOUT;
    loop {
        if stopping.load(Ordering::Acquire) {
            return ReconnectSessionResult::Rejected;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return ReconnectSessionResult::Rejected;
        }
        match completed.recv_timeout(remaining.min(CONTROL_COMPLETION_POLL_INTERVAL)) {
            Ok(result) => return result,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return ReconnectSessionResult::Rejected,
        }
    }
}

fn reconnect_session_status(result: ReconnectSessionResult) -> &'static str {
    match result {
        ReconnectSessionResult::Reconnected => "ok",
        ReconnectSessionResult::AuthenticationFailed => "authentication_failed",
        ReconnectSessionResult::SessionNotFound => "session_not_found",
        ReconnectSessionResult::StillStarting => "session_starting",
        ReconnectSessionResult::Rejected => "rejected",
    }
}

impl Drop for ProcessControlServer {
    fn drop(&mut self) {
        self.begin_shutdown();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.endpoint_path);
        let _ = fs::remove_file(&self.socket_path);
    }
}

#[cfg(test)]
#[path = "../tests/process_control/server.rs"]
mod tests;
