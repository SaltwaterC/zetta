//! The Windows pseudoconsole host.
//!
//! Windows cannot replace a process's image the way `execv` does, and a
//! pseudoconsole cannot be moved between processes: `HPCON` is meaningful only
//! to whoever created it, and closing it tears the console down. Between them
//! those two facts mean a Windows daemon that owns its consoles can never be
//! upgraded without ending every session it holds.
//!
//! So it does not own them. This host does. It creates the consoles, keeps the
//! `HPCON`s and the child process handles, and outlives the daemon — which can
//! then be replaced by starting a new one and pointing it back here.
//!
//! The host is deliberately small and deliberately dull. It is the part that
//! must not need upgrading, so its protocol is versioned and only ever gains
//! messages: a daemon that has just been replaced will find a host still
//! running the *older* image, and has to be able to talk to it.

#![cfg(windows)]

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use alacritty_terminal::{
    event::{OnResize as _, WindowSize},
    tty,
};
use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::{
    catalog::create_private_dir,
    messages::TerminalSize,
    paths::session_catalog_dir,
    transport::{Connection, Endpoint, Listener, Stream, random_hex, token_matches},
};

/// The host protocol. Additive only: a daemon that was just replaced still has
/// to be able to speak to the host that was already running.
pub const HOST_PROTOCOL_VERSION: u32 = 1;

/// The oldest host this daemon can drive. An upgrade is refused rather than
/// attempted when the running host is older than this, because the alternative
/// is a new daemon that cannot reach the sessions it just inherited.
pub const MINIMUM_HOST_PROTOCOL_VERSION: u32 = 1;

pub fn endpoint_path(directory: &Path) -> PathBuf {
    directory.join("zmux-host.json")
}

fn socket_path(directory: &Path) -> PathBuf {
    directory.join("zmux-host.sock")
}

const STOP_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostEnvelope {
    pub version: u32,
    pub token: String,
    /// The process a console's pipes should be duplicated into. The host has
    /// no other way to hand a terminal over, and the daemon asking on a
    /// client's behalf is not necessarily the destination.
    #[serde(default)]
    pub target_process_id: u32,
    pub request: HostRequest,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum HostRequest {
    /// Creates a pseudoconsole and starts a process on it.
    Open {
        program: Option<String>,
        args: Vec<String>,
        env: HashMap<String, String>,
        working_directory: Option<PathBuf>,
        size: TerminalSize,
    },
    /// Duplicates an existing console's pipes into `target_process_id`, which
    /// is how a session is handed to a client after a daemon restart.
    Handles { console_id: u64 },
    /// Only the console's creator can resize it.
    Resize {
        console_id: u64,
        columns: u16,
        lines: u16,
    },
    /// Ends the process and tears the console down.
    Close { console_id: u64 },
    /// The consoles this host is holding, so a replacement daemon can find
    /// the sessions it inherited.
    List,
    /// Which children have exited since this was last asked.
    ///
    /// Polled rather than pushed: the host must not have to know how to reach
    /// a daemon that may have been restarted since the console was created.
    Reap,
    /// Stops the host once it holds nothing.
    Shutdown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "response", rename_all = "snake_case")]
pub enum HostResponse {
    Opened {
        console_id: u64,
        child_pid: u32,
        handles: Vec<i64>,
    },
    Handles {
        child_pid: u32,
        handles: Vec<i64>,
    },
    Consoles {
        consoles: Vec<ConsoleSummary>,
    },
    Exits {
        exits: Vec<ConsoleExit>,
    },
    Ok,
    Error {
        message: String,
    },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ConsoleSummary {
    pub console_id: u64,
    pub child_pid: u32,
    pub exited: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ConsoleExit {
    pub console_id: u64,
    pub child_pid: u32,
    pub exit_code: Option<i32>,
}

struct Console {
    id: u64,
    pty: tty::PtyOwner,
    exited: bool,
    /// Set when the exit has been observed but not yet collected by a daemon.
    /// Held until it is reported, so an exit that happens while the daemon is
    /// being replaced is delivered to its replacement rather than lost.
    pending_exit: Option<ConsoleExit>,
}

#[derive(Default)]
struct Host {
    consoles: Mutex<Vec<Console>>,
    next_console_id: std::sync::atomic::AtomicU64,
}

/// Runs the host until it holds nothing.
pub fn run() -> Result<()> {
    let directory = session_catalog_dir();
    create_private_dir(&directory)?;
    let socket = socket_path(&directory);
    let endpoint = endpoint_path(&directory);

    // A live host is never replaced: it is holding consoles that cannot be
    // recreated, which is the entire reason it exists.
    if let Ok(existing) = Endpoint::read(&endpoint)
        && Stream::connect(&existing.socket_path).is_ok()
    {
        anyhow::bail!("a pseudoconsole host is already running");
    }
    let _ = std::fs::remove_file(&socket);

    let listener = Listener::bind(&socket)
        .with_context(|| format!("binding the host socket {}", socket.display()))?;
    Endpoint {
        version: crate::transport::ENDPOINT_VERSION,
        protocol_version: HOST_PROTOCOL_VERSION,
        process_id: std::process::id(),
        socket_path: socket.clone(),
        token: random_hex(32)?,
    }
    .write(&endpoint)?;
    let token = Endpoint::read(&endpoint)?.token;

    let host = Arc::new(Host {
        consoles: Mutex::new(Vec::new()),
        next_console_id: std::sync::atomic::AtomicU64::new(1),
    });
    start_watcher(host.clone());

    log::info!("zmux pseudoconsole host listening on {}", socket.display());
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let host = host.clone();
        let token = token.clone();
        thread::spawn(move || {
            if let Err(error) = serve(&host, stream, &token) {
                log::debug!("host connection ended: {error:#}");
            }
        });
    }

    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_file(&endpoint);
    Ok(())
}

fn serve(host: &Arc<Host>, stream: Stream, token: &str) -> Result<()> {
    let mut connection = Connection::new(stream);
    let envelope: HostEnvelope = match connection.receive() {
        Ok((envelope, _)) => envelope,
        Err(error) => {
            let _ = connection.send(&HostResponse::Error {
                message: format!("unreadable request: {error:#}"),
            });
            return Err(error);
        }
    };
    if !token_matches(&envelope.token, token) {
        connection.send(&HostResponse::Error {
            message: "invalid host token".to_owned(),
        })?;
        anyhow::bail!("rejected a host connection presenting the wrong token");
    }
    // Newer daemons are served: the host is the part that does not get
    // upgraded, so it has to keep working for images that came after it. A
    // request it does not understand fails on its own, by name.
    if envelope.version < HOST_PROTOCOL_VERSION {
        connection.send(&HostResponse::Error {
            message: format!(
                "this host speaks protocol version {HOST_PROTOCOL_VERSION}, newer than the \
                 requested {}",
                envelope.version
            ),
        })?;
        anyhow::bail!(
            "refused a daemon speaking host protocol {}",
            envelope.version
        );
    }

    let shutting_down = matches!(&envelope.request, HostRequest::Shutdown);
    let response = handle(host, envelope);
    let result = connection.send(&response);
    if shutting_down && matches!(response, HostResponse::Ok) && result.is_ok() {
        std::process::exit(0);
    }
    result
}

fn handle(host: &Arc<Host>, envelope: HostEnvelope) -> HostResponse {
    match envelope.request {
        HostRequest::Open {
            program,
            args,
            env,
            working_directory,
            size,
        } => match open_console(
            host,
            program,
            args,
            env,
            working_directory,
            size,
            envelope.target_process_id,
        ) {
            Ok(response) => response,
            Err(error) => HostResponse::Error {
                message: format!("{error:#}"),
            },
        },
        HostRequest::Handles { console_id } => {
            match console_handles(host, console_id, envelope.target_process_id) {
                Ok(response) => response,
                Err(error) => HostResponse::Error {
                    message: format!("{error:#}"),
                },
            }
        }
        HostRequest::Resize {
            console_id,
            columns,
            lines,
        } => {
            let mut consoles = host.consoles.lock().unwrap();
            if let Some(console) = consoles.iter_mut().find(|console| console.id == console_id) {
                console.pty.on_resize(WindowSize {
                    num_lines: lines.max(1),
                    num_cols: columns.max(1),
                    cell_width: 0,
                    cell_height: 0,
                });
            }
            HostResponse::Ok
        }
        HostRequest::Close { console_id } => {
            // Dropping the PTY closes the pseudoconsole, which ends the
            // process running on it.
            host.consoles
                .lock()
                .unwrap()
                .retain(|console| console.id != console_id);
            HostResponse::Ok
        }
        HostRequest::List => {
            let consoles = host
                .consoles
                .lock()
                .unwrap()
                .iter()
                .map(|console| ConsoleSummary {
                    console_id: console.id,
                    child_pid: console.pty.child_pid(),
                    exited: console.exited,
                })
                .collect();
            HostResponse::Consoles { consoles }
        }
        HostRequest::Reap => {
            let mut consoles = host.consoles.lock().unwrap();
            let exits = consoles
                .iter_mut()
                .filter_map(|console| console.pending_exit.take())
                .collect();
            HostResponse::Exits { exits }
        }
        HostRequest::Shutdown => {
            if host.consoles.lock().unwrap().is_empty() {
                // The serving connection sends the acknowledgement before
                // terminating the helper, so the daemon can complete a
                // normal stop without treating the helper's exit as an I/O
                // failure.
                HostResponse::Ok
            } else {
                HostResponse::Error {
                    message: "the host is still holding consoles".to_owned(),
                }
            }
        }
    }
}

fn open_console(
    host: &Arc<Host>,
    program: Option<String>,
    args: Vec<String>,
    env: HashMap<String, String>,
    working_directory: Option<PathBuf>,
    size: TerminalSize,
    target_process_id: u32,
) -> Result<HostResponse> {
    let escape_args = escape_windows_shell_args(program.as_deref());
    let options = tty::Options {
        shell: program.map(|program| tty::Shell::new(program, args)),
        working_directory,
        drain_on_exit: true,
        env,
        // `PtySpawnRequest` carries logical arguments, so use the same Windows
        // command-line quoting as a local PTY. CMD is the one exception: its
        // command arguments have shell-specific quoting that the terminal
        // layer deliberately passes through raw.
        escape_args,
    };
    let pty = tty::new_host(
        &options,
        WindowSize {
            num_lines: size.lines.max(1),
            num_cols: size.columns.max(1),
            cell_width: size.cell_width,
            cell_height: size.cell_height,
        },
    )
    .context("creating a pseudoconsole")?;

    let console_id = host
        .next_console_id
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let child_pid = pty.child_pid();
    let handles = duplicate_console(&pty, target_process_id)?;
    host.consoles.lock().unwrap().push(Console {
        id: console_id,
        pty,
        exited: false,
        pending_exit: None,
    });
    Ok(HostResponse::Opened {
        console_id,
        child_pid,
        handles,
    })
}

fn escape_windows_shell_args(program: Option<&str>) -> bool {
    !program
        .and_then(|program| {
            Path::new(program)
                .file_stem()
                .and_then(|name| name.to_str())
        })
        .is_some_and(|name| name.eq_ignore_ascii_case("cmd"))
}

fn console_handles(
    host: &Arc<Host>,
    console_id: u64,
    target_process_id: u32,
) -> Result<HostResponse> {
    let consoles = host.consoles.lock().unwrap();
    let console = consoles
        .iter()
        .find(|console| console.id == console_id)
        .context("the host is not holding that console")?;
    let handles = duplicate_console(&console.pty, target_process_id)?;
    Ok(HostResponse::Handles {
        child_pid: console.pty.child_pid(),
        handles,
    })
}

fn duplicate_console(pty: &tty::PtyOwner, target_process_id: u32) -> Result<Vec<i64>> {
    use std::os::windows::io::AsHandle as _;
    anyhow::ensure!(
        target_process_id != 0,
        "no process was named to hand the console to"
    );
    let (conout, conin) = pty.handover_handles();
    crate::transport::duplicate_to(target_process_id, &[conout.as_handle(), conin.as_handle()])
}

/// Notices children exiting.
///
/// The host holds the exit until a daemon collects it, so an exit that happens
/// while the daemon is being replaced reaches its successor instead of
/// vanishing.
fn start_watcher(host: Arc<Host>) {
    thread::spawn(move || {
        loop {
            {
                let mut consoles = host.consoles.lock().unwrap();
                for console in consoles.iter_mut() {
                    if console.exited {
                        continue;
                    }
                    if let Some(event) = console.pty.next_child_event() {
                        console.exited = true;
                        console.pending_exit = Some(ConsoleExit {
                            console_id: console.id,
                            child_pid: console.pty.child_pid(),
                            exit_code: match event {
                                alacritty_terminal::tty::ChildEvent::Exited(status) => {
                                    status.code()
                                }
                                _ => None,
                            },
                        });
                    }
                }
            }
            thread::sleep(std::time::Duration::from_millis(50));
        }
    });
}

/// Starts a host if there is not one already, and returns a client for it.
pub fn ensure_running(directory: &Path) -> Result<HostClient> {
    if let Some(client) = HostClient::connect(directory)? {
        return Ok(client);
    }
    let current = std::env::current_exe().context("locating this multiplexer")?;
    let executable = current
        .parent()
        .context("the multiplexer executable has no parent directory")?
        .join("zmux-pty.exe");
    anyhow::ensure!(
        executable.is_file(),
        "the Windows pseudoconsole host is not installed beside the multiplexer: {}",
        executable.display()
    );
    let mut command = std::process::Command::new(&executable);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Ok(directory) = std::env::current_dir() {
        command.current_dir(tty::windows::normalize_working_directory(&directory));
    }
    command
        .spawn()
        .with_context(|| format!("starting the pseudoconsole host {}", executable.display()))?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(client) = HostClient::connect(directory)? {
            return Ok(client);
        }
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "the pseudoconsole host did not start"
        );
        thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Stops the host belonging to `directory`, if it is still answering.
///
/// The daemon normally asks the host to leave as part of its own shutdown. A
/// forced stop, however, terminates the daemon before that request can run,
/// and a host may also be left behind after the daemon endpoint has gone
/// stale. Keep this cleanup beside the host protocol so both cases use the
/// same endpoint identity and process tree.
pub fn stop(directory: &Path, force: bool) -> Result<bool> {
    let Ok(endpoint) = Endpoint::read(&endpoint_path(directory)) else {
        return Ok(false);
    };
    if Stream::connect(&endpoint.socket_path).is_err() {
        remove_stopped_files(directory, &endpoint);
        return Ok(false);
    }

    let client = HostClient {
        endpoint: endpoint.clone(),
    };
    match client.shutdown() {
        Ok(()) => {}
        Err(_error) if force => force_stop(&endpoint).with_context(|| {
            format!(
                "the pseudoconsole host did not stop cleanly; forcing process {}",
                endpoint.process_id
            )
        })?,
        Err(error) => {
            return Err(error).context(
                "the pseudoconsole host was left running; rerun with --force to stop it anyway",
            );
        }
    }

    remove_stopped_files(directory, &endpoint);
    Ok(true)
}

fn force_stop(endpoint: &Endpoint) -> Result<()> {
    use std::process::{Command, Stdio};

    anyhow::ensure!(
        endpoint.process_id > 0,
        "the pseudoconsole host published an unusable process id"
    );
    let system_root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let taskkill = system_root.join(r"System32\taskkill.exe");
    let status = Command::new(&taskkill)
        .args([
            "/PID".to_owned(),
            endpoint.process_id.to_string(),
            "/T".to_owned(),
            "/F".to_owned(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("running {}", taskkill.display()))?;
    // `taskkill /T` can report an access error for one pseudoconsole child
    // even while the host is still the process we need to remove. Fall back
    // to terminating the host itself; closing its handles tears down the
    // pseudoconsole tree, and this is also what makes the executable
    // replaceable again.
    if !status.success() && Stream::connect(&endpoint.socket_path).is_ok() {
        crate::terminate_process(endpoint.process_id).with_context(|| {
            format!(
                "{} could not terminate pseudoconsole host process {}",
                taskkill.display(),
                endpoint.process_id
            )
        })?;
    }
    wait_until_stopped(endpoint)
}

fn wait_until_stopped(endpoint: &Endpoint) -> Result<()> {
    let deadline = Instant::now() + STOP_TIMEOUT;
    loop {
        if Stream::connect(&endpoint.socket_path).is_err() {
            return Ok(());
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "the pseudoconsole host is still answering on {}",
            endpoint.socket_path.display()
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn remove_stopped_files(directory: &Path, endpoint: &Endpoint) {
    // `zmux-pty` exits from its serving thread after acknowledging Shutdown,
    // so its normal cleanup code is not reached. Remove only files that still
    // describe this exact host; a replacement that started immediately after
    // it stopped must keep its own endpoint.
    if Endpoint::read(&endpoint_path(directory)).is_ok_and(|current| current == *endpoint) {
        let _ = std::fs::remove_file(&endpoint.socket_path);
        let _ = std::fs::remove_file(endpoint_path(directory));
    }
}

pub struct HostClient {
    endpoint: Endpoint,
}

impl HostClient {
    pub fn connect(directory: &Path) -> Result<Option<Self>> {
        let Ok(endpoint) = Endpoint::read(&endpoint_path(directory)) else {
            return Ok(None);
        };
        // A host older than this daemon can drive is refused here, before an
        // upgrade has committed to it.
        anyhow::ensure!(
            endpoint.protocol_version >= MINIMUM_HOST_PROTOCOL_VERSION,
            "the running pseudoconsole host speaks protocol version {}, older than the \
             {MINIMUM_HOST_PROTOCOL_VERSION} this multiplexer requires",
            endpoint.protocol_version
        );
        if Stream::connect(&endpoint.socket_path).is_err() {
            return Ok(None);
        }
        Ok(Some(Self { endpoint }))
    }

    fn request(&self, request: HostRequest, target_process_id: u32) -> Result<HostResponse> {
        let stream = Stream::connect(&self.endpoint.socket_path)
            .context("connecting to the pseudoconsole host")?;
        let mut connection = Connection::new(stream);
        connection.send(&HostEnvelope {
            version: HOST_PROTOCOL_VERSION,
            token: self.endpoint.token.clone(),
            target_process_id,
            request,
        })?;
        Ok(connection.receive::<HostResponse>()?.0)
    }

    pub fn open(
        &self,
        program: Option<String>,
        args: Vec<String>,
        env: HashMap<String, String>,
        working_directory: Option<PathBuf>,
        size: TerminalSize,
        target_process_id: u32,
    ) -> Result<(u64, u32, Vec<i64>)> {
        match self.request(
            HostRequest::Open {
                program,
                args,
                env,
                working_directory,
                size,
            },
            target_process_id,
        )? {
            HostResponse::Opened {
                console_id,
                child_pid,
                handles,
            } => Ok((console_id, child_pid, handles)),
            HostResponse::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected host response: {other:?}"),
        }
    }

    pub fn handles(&self, console_id: u64, target_process_id: u32) -> Result<(u32, Vec<i64>)> {
        match self.request(HostRequest::Handles { console_id }, target_process_id)? {
            HostResponse::Handles { child_pid, handles } => Ok((child_pid, handles)),
            HostResponse::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected host response: {other:?}"),
        }
    }

    pub fn resize(&self, console_id: u64, columns: u16, lines: u16) -> Result<()> {
        match self.request(
            HostRequest::Resize {
                console_id,
                columns,
                lines,
            },
            0,
        )? {
            HostResponse::Ok => Ok(()),
            HostResponse::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected host response: {other:?}"),
        }
    }

    pub fn close(&self, console_id: u64) -> Result<()> {
        match self.request(HostRequest::Close { console_id }, 0)? {
            HostResponse::Ok => Ok(()),
            HostResponse::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected host response: {other:?}"),
        }
    }

    pub fn list(&self) -> Result<Vec<ConsoleSummary>> {
        match self.request(HostRequest::List, 0)? {
            HostResponse::Consoles { consoles } => Ok(consoles),
            HostResponse::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected host response: {other:?}"),
        }
    }

    pub fn reap(&self) -> Result<Vec<ConsoleExit>> {
        match self.request(HostRequest::Reap, 0)? {
            HostResponse::Exits { exits } => Ok(exits),
            HostResponse::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected host response: {other:?}"),
        }
    }

    pub fn shutdown(&self) -> Result<()> {
        match self.request(HostRequest::Shutdown, 0)? {
            HostResponse::Ok => wait_until_stopped(&self.endpoint),
            HostResponse::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected host response: {other:?}"),
        }
    }
}

#[cfg(test)]
#[path = "tests/pty_host.rs"]
mod tests;
