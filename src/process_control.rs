use std::{
    fs,
    io::{BufRead as _, BufReader, Read as _, Write as _},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, RecvTimeoutError, Sender, channel},
    },
    thread,
    time::{Duration, Instant},
};
use zeroize::{Zeroize as _, Zeroizing};

#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(windows)]
use uds_windows::{UnixListener, UnixStream};

use anyhow::{Context as _, Result};
use futures::channel::mpsc::UnboundedSender;
use gpui::Hsla;
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use subtle::ConstantTimeEq as _;
use sysinfo::{Pid, ProcessesToUpdate, System};
use ui::IconName;

use crate::background_sessions::SessionSecret;
use crate::command_panes::{
    MAX_PANE_COMMAND_BYTES, PaneCommand, pane_command_byte_len, parse_pane_direction,
};
use crate::pane::{OverlayFontSize, PaneOverlayRequest, overlay_color_from_value};

/// Bumped when a control command's meaning changes, so a Zetta that cannot
/// serve a request is not sent one.
///
/// 14 added multiplexer-held sessions: reconnecting one has to reach a window
/// that knows to ask the multiplexer for it. An older window accepts the
/// request and reports that the session does not exist, which is exactly the
/// confusing failure this guards against.
///
/// 15 adds the originating tab target to reconnect requests. Without it, a
/// shared session was sent to whichever process-control endpoint happened to
/// answer first, and that process then chose its first window.
/// The shared value is also used by the standalone `zmux reconnect` client.
///
/// 16 adds the disk-session resume request. Its private identity payload is
/// carried in `config_path` so older request construction stays unchanged; the
/// command is version-gated and the decoder accepts that field only for this
/// request.
///
/// 17 adds passphrases for encrypted identity files to that private payload.
pub(crate) const CONTROL_VERSION: u32 = zmux::protocol::CONTROL_VERSION;
// A 64 KiB argv payload can expand substantially when it contains many
// one-character arguments and each value is represented as JSON. Keep enough
// framing headroom for that worst case as well as the endpoint token.
const MAX_CONTROL_MESSAGE_BYTES: usize = 256 * 1024;
const CONTROL_COMPLETION_TIMEOUT: Duration = Duration::from_secs(2);
const CONTROL_COMPLETION_POLL_INTERVAL: Duration = Duration::from_millis(25);
const CONTROL_CLIENT_TIMEOUT: Duration = Duration::from_secs(3);
// Reconnecting a protected session costs an Argon2 verification and, when the
// secret is wrong, `background_sessions::FAILED_AUTHENTICATION_DELAY` on top of
// it. Give that path its own budget instead of squeezing it into the generic
// one: otherwise raising the Argon2 cost or the anti-guessing delay would
// silently turn "the session secret was incorrect" into "Zetta rejected the
// reconnect request". The `zmux reconnect` client covers the resulting
// ordering. The budget
// is sized to keep an Argon2 verification under a quarter of it on slow
// machines: memory-constrained VMs and debug builds can take seconds per
// verification.
const RECONNECT_COMPLETION_TIMEOUT: Duration = Duration::from_secs(16);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReconnectSessionResult {
    Reconnected,
    AuthenticationFailed,
    SessionNotFound,
    StillStarting,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReplacePaneRequest {
    pub(crate) split: Option<String>,
    pub(crate) profile: Option<String>,
    pub(crate) theme: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TabAttentionRequest {
    pub(crate) attention_id: u64,
    pub(crate) summary: String,
    pub(crate) body: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TabNameRequest {
    pub(crate) attention_id: u64,
    pub(crate) name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorktreeNameRequest {
    pub(crate) attention_id: u64,
    pub(crate) name: Option<String>,
}

pub(crate) enum ProcessControlCommand {
    ReloadConfiguration {
        config_path: String,
        completion: Sender<bool>,
    },
    OpenWindow {
        completion: Sender<bool>,
    },
    OpenProject {
        root: PathBuf,
        completion: Sender<bool>,
    },
    ReloadProjects {
        completion: Sender<bool>,
    },
    ReplacePane {
        request: ReplacePaneRequest,
        completion: Sender<bool>,
    },
    RunPane {
        request: PaneCommand,
        completion: Sender<std::result::Result<(), String>>,
    },
    ListPaneLabels {
        completion: Sender<std::result::Result<Vec<String>, String>>,
    },
    ReconnectSession {
        runner_id: u64,
        session_id: u64,
        attention_id: Option<u64>,
        secret: Option<SessionSecret>,
        completion: Sender<ReconnectSessionResult>,
    },
    ResumeDiskSession {
        session_id: u64,
        identity_paths: Vec<PathBuf>,
        identity_passphrases: Vec<Option<SessionSecret>>,
        secret: Option<SessionSecret>,
        completion: Sender<ReconnectSessionResult>,
    },
    SetTabIcon {
        icon: Option<IconName>,
        completion: Sender<bool>,
    },
    SetPaneTheme {
        theme: Option<String>,
        completion: Sender<bool>,
    },
    ListPaneThemes {
        completion: Sender<Vec<String>>,
    },
    GetPaneTheme {
        attention_id: u64,
        pane_id: Option<u64>,
        completion: Sender<std::result::Result<String, String>>,
    },
    SetPaneOverlay {
        text: Option<String>,
        font_size: Option<OverlayFontSize>,
        opacity: Option<f32>,
        color: Option<Hsla>,
        completion: Sender<bool>,
    },
    SetTabAttention {
        request: TabAttentionRequest,
        completion: Sender<bool>,
    },
    FocusTab {
        attention_id: u64,
        completion: Sender<bool>,
    },
    SetTabName {
        request: TabNameRequest,
        completion: Sender<bool>,
    },
    SetWorktreeName {
        request: WorktreeNameRequest,
        completion: Sender<bool>,
    },
    GetSilentMode {
        attention_id: Option<u64>,
        completion: Sender<bool>,
    },
}

#[derive(Clone, Debug, PartialEq)]
enum ControlRequestCommand {
    ReloadConfiguration {
        config_path: String,
    },
    OpenWindow,
    OpenProject {
        root: PathBuf,
    },
    ReloadProjects,
    ReplacePane {
        split: Option<String>,
        profile: Option<String>,
        theme: Option<String>,
    },
    RunPane {
        request: PaneCommand,
    },
    ListPaneLabels,
    ReconnectSession {
        runner_id: u64,
        session_id: u64,
        attention_id: Option<u64>,
        secret: Option<SessionSecret>,
    },
    ResumeDiskSession {
        session_id: u64,
        identity_paths: Vec<PathBuf>,
        identity_passphrases: Vec<Option<SessionSecret>>,
        secret: Option<SessionSecret>,
    },
    SetTabIcon {
        icon: Option<IconName>,
    },
    SetPaneTheme {
        theme: Option<String>,
    },
    ListPaneThemes,
    GetPaneTheme {
        attention_id: u64,
        pane_id: Option<u64>,
    },
    SetPaneOverlay {
        text: Option<String>,
        font_size: Option<OverlayFontSize>,
        opacity: Option<f32>,
        color: Option<String>,
    },
    SetTabAttention {
        attention_id: u64,
        summary: String,
        body: Option<String>,
    },
    FocusTab {
        attention_id: u64,
    },
    SetTabName {
        attention_id: u64,
        name: Option<String>,
    },
    SetWorktreeName {
        attention_id: u64,
        name: Option<String>,
    },
    GetSilentMode {
        attention_id: Option<u64>,
    },
}

#[derive(Clone, Serialize, Deserialize)]
struct ControlEndpoint {
    version: u32,
    process_id: u32,
    socket_path: PathBuf,
    token: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlRequest {
    token: String,
    command: String,
    runner_id: Option<u64>,
    session_id: Option<u64>,
    secret: Option<String>,
    icon: Option<String>,
    pane_theme: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pane_id: Option<u64>,
    pane_overlay: Option<String>,
    pane_overlay_font_size: Option<String>,
    pane_overlay_opacity: Option<u8>,
    pane_overlay_color: Option<String>,
    attention_id: Option<u64>,
    attention_summary: Option<String>,
    attention_body: Option<String>,
    tab_name: Option<String>,
    worktree_name: Option<String>,
    config_path: Option<String>,
    split: Option<String>,
    profile: Option<String>,
    theme: Option<String>,
    pane_request: Option<PaneControlRequest>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResumeIdentityPayload {
    identity_paths: Vec<String>,
    identity_passphrases: Vec<Option<ResumeIdentityPassphrase>>,
}

struct ResumeIdentityPassphrase(Zeroizing<String>);

impl<'de> Deserialize<'de> for ResumeIdentityPassphrase {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(|passphrase| Self(Zeroizing::new(passphrase)))
    }
}

impl ResumeIdentityPassphrase {
    fn expose(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PaneControlRequest {
    direction: Option<String>,
    label: Option<String>,
    pane: Option<String>,
    overlay: Option<PaneControlOverlayRequest>,
    stack: bool,
    command: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PaneControlOverlayRequest {
    text: Option<String>,
    font_size: Option<String>,
    opacity: Option<u8>,
    color: Option<String>,
}

impl From<&PaneOverlayRequest> for PaneControlOverlayRequest {
    fn from(request: &PaneOverlayRequest) -> Self {
        Self {
            text: request.text.clone(),
            font_size: request.font_size.map(|size| size.cli_name().to_owned()),
            opacity: request.opacity,
            color: request.color.clone(),
        }
    }
}

impl PaneControlOverlayRequest {
    fn into_request(self) -> Option<PaneOverlayRequest> {
        if self.text.is_none() || self.opacity.is_some_and(|opacity| opacity > 100) {
            return None;
        }
        let font_size = match self.font_size {
            Some(font_size) => Some(OverlayFontSize::parse(&font_size)?),
            None => None,
        };
        if self
            .color
            .as_deref()
            .is_some_and(|color| overlay_color_from_value(color).is_none())
        {
            return None;
        }
        Some(PaneOverlayRequest {
            text: self.text,
            font_size,
            opacity: self.opacity,
            color: self.color,
        })
    }
}

impl From<&PaneCommand> for PaneControlRequest {
    fn from(request: &PaneCommand) -> Self {
        Self {
            direction: request.direction.map(|direction| {
                match direction {
                    crate::pane::PaneDirection::Left => "left",
                    crate::pane::PaneDirection::Right => "right",
                    crate::pane::PaneDirection::Up => "up",
                    crate::pane::PaneDirection::Down => "down",
                }
                .to_owned()
            }),
            label: request.label.clone(),
            pane: request.pane.clone(),
            overlay: request.overlay.as_ref().map(Into::into),
            stack: request.stack,
            command: request.command.clone(),
        }
    }
}

impl PaneControlRequest {
    fn into_command(self) -> Option<PaneCommand> {
        let direction = match self.direction {
            Some(direction) => Some(parse_pane_direction(&direction)?),
            None => None,
        };
        let overlay = match self.overlay {
            Some(overlay) => Some(overlay.into_request()?),
            None => None,
        };
        if self.command.is_empty()
            || pane_command_byte_len(&self.command) > MAX_PANE_COMMAND_BYTES
            || self.label.as_deref().is_some_and(str::is_empty)
            || self.pane.as_deref().is_some_and(str::is_empty)
            || (self.label.is_some() && direction.is_none())
            || (overlay.is_some() && direction.is_none())
            || (direction.is_some() && (self.pane.is_some() || self.stack))
        {
            return None;
        }
        Some(PaneCommand {
            direction,
            label: self.label,
            pane: self.pane,
            overlay,
            stack: self.stack,
            list: false,
            command: self.command,
        })
    }
}

#[derive(Serialize, Deserialize)]
struct ControlResponse {
    status: String,
    #[serde(default)]
    themes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pane_theme: Option<String>,
    #[serde(default)]
    silent_mode: bool,
    #[serde(default)]
    pane_labels: Vec<String>,
    #[serde(default)]
    error: Option<ControlError>,
}

#[derive(Clone, Serialize, Deserialize)]
struct ControlError {
    code: String,
    message: String,
}

pub(crate) struct ProcessControlServer {
    endpoint_path: PathBuf,
    socket_path: PathBuf,
    stopping: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl ProcessControlServer {
    pub(crate) fn start(commands: UnboundedSender<ProcessControlCommand>) -> Result<Self> {
        Self::start_at(commands, control_endpoint_path(std::process::id()))
    }

    fn start_at(
        commands: UnboundedSender<ProcessControlCommand>,
        endpoint_path: PathBuf,
    ) -> Result<Self> {
        let parent = endpoint_path
            .parent()
            .context("control endpoint has no parent")?;
        crate::background_sessions::create_private_dir(parent)?;
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
        let stopping_for_thread = stopping.clone();
        let thread = thread::Builder::new()
            .name("zetta-process-control".to_owned())
            .spawn(move || {
                for stream in listener.incoming() {
                    if stopping_for_thread.load(Ordering::Acquire) {
                        break;
                    }
                    let Ok(mut stream) = stream else {
                        continue;
                    };
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                    let mut response_themes = Vec::new();
                    let mut response_pane_theme = None;
                    let mut response_silent_mode = false;
                    let mut response_pane_labels = Vec::new();
                    let mut response_error = None;
                    let status = match handle_control_request(&mut stream, &token) {
                        Some(ControlRequestCommand::ReloadConfiguration { config_path }) => {
                            let (completion, completed) = channel();
                            let accepted = commands
                                .unbounded_send(ProcessControlCommand::ReloadConfiguration {
                                    config_path,
                                    completion,
                                })
                                .is_ok()
                                && wait_for_control_completion(&completed, &stopping_for_thread);
                            if accepted { "ok" } else { "rejected" }
                        }
                        Some(ControlRequestCommand::OpenWindow) => {
                            let (completion, completed) = channel();
                            let accepted = commands
                                .unbounded_send(ProcessControlCommand::OpenWindow { completion })
                                .is_ok()
                                && wait_for_control_completion(&completed, &stopping_for_thread);
                            if accepted { "ok" } else { "rejected" }
                        }
                        Some(ControlRequestCommand::OpenProject { root }) => {
                            let (completion, completed) = channel();
                            let accepted = commands
                                .unbounded_send(ProcessControlCommand::OpenProject {
                                    root,
                                    completion,
                                })
                                .is_ok()
                                && wait_for_control_completion(&completed, &stopping_for_thread);
                            if accepted { "ok" } else { "rejected" }
                        }
                        Some(ControlRequestCommand::ReloadProjects) => {
                            let (completion, completed) = channel();
                            let accepted = commands
                                .unbounded_send(ProcessControlCommand::ReloadProjects {
                                    completion,
                                })
                                .is_ok()
                                && wait_for_control_completion(&completed, &stopping_for_thread);
                            if accepted { "ok" } else { "rejected" }
                        }
                        Some(ControlRequestCommand::ReplacePane {
                            split,
                            profile,
                            theme,
                        }) => {
                            let (completion, completed) = channel();
                            let accepted = commands
                                .unbounded_send(ProcessControlCommand::ReplacePane {
                                    request: ReplacePaneRequest {
                                        split,
                                        profile,
                                        theme,
                                    },
                                    completion,
                                })
                                .is_ok()
                                && wait_for_control_completion(&completed, &stopping_for_thread);
                            if accepted { "ok" } else { "rejected" }
                        }
                        Some(ControlRequestCommand::RunPane { request }) => {
                            let (completion, completed) = channel();
                            if commands
                                .unbounded_send(ProcessControlCommand::RunPane {
                                    request,
                                    completion,
                                })
                                .is_err()
                            {
                                "rejected"
                            } else {
                                match wait_for_result_completion(&completed, &stopping_for_thread) {
                                    Some(Ok(())) => "ok",
                                    Some(Err(message)) => {
                                        response_error = Some(ControlError {
                                            code: "pane_rejected".to_owned(),
                                            message,
                                        });
                                        "rejected"
                                    }
                                    None => "rejected",
                                }
                            }
                        }
                        Some(ControlRequestCommand::ListPaneLabels) => {
                            let (completion, completed) = channel();
                            if commands
                                .unbounded_send(ProcessControlCommand::ListPaneLabels {
                                    completion,
                                })
                                .is_err()
                            {
                                "rejected"
                            } else {
                                match wait_for_result_completion(&completed, &stopping_for_thread) {
                                    Some(Ok(labels)) => {
                                        response_pane_labels = labels;
                                        "ok"
                                    }
                                    Some(Err(message)) => {
                                        response_error = Some(ControlError {
                                            code: "pane_list_rejected".to_owned(),
                                            message,
                                        });
                                        "rejected"
                                    }
                                    None => "rejected",
                                }
                            }
                        }
                        Some(ControlRequestCommand::ReconnectSession {
                            runner_id,
                            session_id,
                            attention_id,
                            secret,
                        }) => {
                            let (completion, completed) = channel();
                            let result = if commands
                                .unbounded_send(ProcessControlCommand::ReconnectSession {
                                    runner_id,
                                    session_id,
                                    attention_id,
                                    secret,
                                    completion,
                                })
                                .is_ok()
                            {
                                wait_for_reconnect_completion(&completed, &stopping_for_thread)
                            } else {
                                ReconnectSessionResult::Rejected
                            };
                            reconnect_session_status(result)
                        }
                        Some(ControlRequestCommand::ResumeDiskSession {
                            session_id,
                            identity_paths,
                            identity_passphrases,
                            secret,
                        }) => {
                            let (completion, completed) = channel();
                            let result = if commands
                                .unbounded_send(ProcessControlCommand::ResumeDiskSession {
                                    session_id,
                                    identity_paths,
                                    identity_passphrases,
                                    secret,
                                    completion,
                                })
                                .is_ok()
                            {
                                wait_for_reconnect_completion(&completed, &stopping_for_thread)
                            } else {
                                ReconnectSessionResult::Rejected
                            };
                            reconnect_session_status(result)
                        }
                        Some(ControlRequestCommand::SetTabIcon { icon }) => {
                            let (completion, completed) = channel();
                            let accepted = commands
                                .unbounded_send(ProcessControlCommand::SetTabIcon {
                                    icon,
                                    completion,
                                })
                                .is_ok()
                                && wait_for_control_completion(&completed, &stopping_for_thread);
                            if accepted { "ok" } else { "rejected" }
                        }
                        Some(ControlRequestCommand::SetPaneTheme { theme }) => {
                            let (completion, completed) = channel();
                            let accepted = commands
                                .unbounded_send(ProcessControlCommand::SetPaneTheme {
                                    theme,
                                    completion,
                                })
                                .is_ok()
                                && wait_for_control_completion(&completed, &stopping_for_thread);
                            if accepted { "ok" } else { "rejected" }
                        }
                        Some(ControlRequestCommand::ListPaneThemes) => {
                            let (completion, completed) = channel();
                            let accepted = commands
                                .unbounded_send(ProcessControlCommand::ListPaneThemes {
                                    completion,
                                })
                                .is_ok();
                            match accepted
                                .then(|| {
                                    wait_for_theme_list_completion(&completed, &stopping_for_thread)
                                })
                                .flatten()
                            {
                                Some(themes) => {
                                    response_themes = themes;
                                    "ok"
                                }
                                None => "rejected",
                            }
                        }
                        Some(ControlRequestCommand::GetPaneTheme {
                            attention_id,
                            pane_id,
                        }) => {
                            let (completion, completed) = channel();
                            if commands
                                .unbounded_send(ProcessControlCommand::GetPaneTheme {
                                    attention_id,
                                    pane_id,
                                    completion,
                                })
                                .is_err()
                            {
                                "rejected"
                            } else {
                                match wait_for_result_completion(&completed, &stopping_for_thread) {
                                    Some(Ok(theme)) => {
                                        response_pane_theme = Some(theme);
                                        "ok"
                                    }
                                    Some(Err(message)) => {
                                        response_error = Some(ControlError {
                                            code: "pane_theme_unavailable".to_owned(),
                                            message,
                                        });
                                        "rejected"
                                    }
                                    None => "rejected",
                                }
                            }
                        }
                        Some(ControlRequestCommand::SetPaneOverlay {
                            text,
                            font_size,
                            opacity,
                            color,
                        }) => {
                            let (completion, completed) = channel();
                            let color = color.and_then(|value| overlay_color_from_value(&value));
                            let accepted = commands
                                .unbounded_send(ProcessControlCommand::SetPaneOverlay {
                                    text,
                                    font_size,
                                    opacity,
                                    color,
                                    completion,
                                })
                                .is_ok()
                                && wait_for_control_completion(&completed, &stopping_for_thread);
                            if accepted { "ok" } else { "rejected" }
                        }
                        Some(ControlRequestCommand::SetTabAttention {
                            attention_id,
                            summary,
                            body,
                        }) => {
                            let (completion, completed) = channel();
                            let accepted = commands
                                .unbounded_send(ProcessControlCommand::SetTabAttention {
                                    request: TabAttentionRequest {
                                        attention_id,
                                        summary,
                                        body,
                                    },
                                    completion,
                                })
                                .is_ok()
                                && wait_for_control_completion(&completed, &stopping_for_thread);
                            if accepted { "ok" } else { "rejected" }
                        }
                        Some(ControlRequestCommand::FocusTab { attention_id }) => {
                            let (completion, completed) = channel();
                            let accepted = commands
                                .unbounded_send(ProcessControlCommand::FocusTab {
                                    attention_id,
                                    completion,
                                })
                                .is_ok()
                                && wait_for_control_completion(&completed, &stopping_for_thread);
                            if accepted { "ok" } else { "rejected" }
                        }
                        Some(ControlRequestCommand::SetTabName { attention_id, name }) => {
                            let (completion, completed) = channel();
                            let accepted = commands
                                .unbounded_send(ProcessControlCommand::SetTabName {
                                    request: TabNameRequest { attention_id, name },
                                    completion,
                                })
                                .is_ok()
                                && wait_for_control_completion(&completed, &stopping_for_thread);
                            if accepted { "ok" } else { "rejected" }
                        }
                        Some(ControlRequestCommand::SetWorktreeName { attention_id, name }) => {
                            let (completion, completed) = channel();
                            let accepted = commands
                                .unbounded_send(ProcessControlCommand::SetWorktreeName {
                                    request: WorktreeNameRequest { attention_id, name },
                                    completion,
                                })
                                .is_ok()
                                && wait_for_control_completion(&completed, &stopping_for_thread);
                            if accepted { "ok" } else { "rejected" }
                        }
                        Some(ControlRequestCommand::GetSilentMode { attention_id }) => {
                            let (completion, completed) = channel();
                            if commands
                                .unbounded_send(ProcessControlCommand::GetSilentMode {
                                    attention_id,
                                    completion,
                                })
                                .is_err()
                            {
                                "rejected"
                            } else if let Some(silent_mode) =
                                wait_for_silent_mode_completion(&completed, &stopping_for_thread)
                            {
                                response_silent_mode = silent_mode;
                                "ok"
                            } else {
                                "rejected"
                            }
                        }
                        None => "rejected",
                    };
                    let status = if status == "ok" && stopping_for_thread.load(Ordering::Acquire) {
                        "rejected"
                    } else {
                        status
                    };
                    let _ = write_message(
                        &mut stream,
                        &ControlResponse {
                            status: status.to_owned(),
                            themes: response_themes,
                            pane_theme: response_pane_theme,
                            silent_mode: response_silent_mode,
                            pane_labels: response_pane_labels,
                            error: response_error,
                        },
                    );
                }
            })
            .context("starting the Zetta process control thread")?;
        Ok(Self {
            endpoint_path,
            socket_path,
            stopping,
            thread: Some(thread),
        })
    }

    pub(crate) fn is_accepting(&self) -> bool {
        !self.stopping.load(Ordering::Acquire)
    }

    pub(crate) fn begin_shutdown(&self) {
        if self.stopping.swap(true, Ordering::AcqRel) {
            return;
        }
        // Stop advertising this process before GPUI begins shutting down. A new
        // launch must start its own application instead of handing off to a
        // process that can no longer keep the requested window alive.
        let _ = fs::remove_file(&self.endpoint_path);
        let _ = UnixStream::connect(&self.socket_path);
        let _ = fs::remove_file(&self.socket_path);
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

fn handle_control_request(stream: &mut UnixStream, token: &str) -> Option<ControlRequestCommand> {
    let mut request = read_message::<ControlRequest>(stream).ok()?;
    decode_control_request(&mut request, token)
}

fn zeroize_control_request_secrets(request: &mut ControlRequest) {
    if let Some(secret) = request.secret.as_mut() {
        secret.zeroize();
    }
    if let Some(payload) = request.config_path.as_mut() {
        payload.zeroize();
    }
}

/// Compares the endpoint token without leaking how many leading bytes matched.
/// This is the only authentication check guarding the process control socket,
/// so it must not short-circuit the way `str` equality does.
fn token_matches(supplied: &str, expected: &str) -> bool {
    let supplied = supplied.as_bytes();
    let expected = expected.as_bytes();
    // ConstantTimeEq over slices already folds the length comparison in, but it
    // requires equal lengths to produce a meaningful choice, so gate on that
    // first. The length of the expected token is not itself a secret.
    supplied.len() == expected.len() && bool::from(supplied.ct_eq(expected))
}

fn decode_control_request(
    request: &mut ControlRequest,
    token: &str,
) -> Option<ControlRequestCommand> {
    if !token_matches(&request.token, token) {
        zeroize_control_request_secrets(request);
        return None;
    }
    if !matches!(request.command.as_str(), "run_pane" | "list_panes")
        && request.pane_request.is_some()
    {
        zeroize_control_request_secrets(request);
        return None;
    }
    if !matches!(
        request.command.as_str(),
        "reload_configuration" | "open_project" | "resume_disk_session"
    ) && request.config_path.is_some()
    {
        zeroize_control_request_secrets(request);
        return None;
    }
    if request.command != "set_tab_name" && request.tab_name.is_some() {
        zeroize_control_request_secrets(request);
        return None;
    }
    if request.command != "set_worktree_name" && request.worktree_name.is_some() {
        zeroize_control_request_secrets(request);
        return None;
    }
    if request.command != "get_pane_theme" && request.pane_id.is_some() {
        zeroize_control_request_secrets(request);
        return None;
    }
    if (!matches!(
        request.command.as_str(),
        "set_tab_attention"
            | "focus_tab"
            | "set_tab_name"
            | "set_worktree_name"
            | "get_silent_mode"
            | "get_pane_theme"
            | "reconnect_session"
    ) && request.attention_id.is_some())
        || (request.command != "set_tab_attention"
            && (request.attention_summary.is_some() || request.attention_body.is_some()))
    {
        zeroize_control_request_secrets(request);
        return None;
    }
    let command = match request.command.as_str() {
        "reload_configuration"
            if request.runner_id.is_none()
                && request.session_id.is_none()
                && request.secret.is_none()
                && request.icon.is_none()
                && request.pane_theme.is_none()
                && request.pane_overlay.is_none()
                && request.pane_overlay_font_size.is_none()
                && request.pane_overlay_opacity.is_none()
                && request.pane_overlay_color.is_none() =>
        {
            request
                .config_path
                .take()
                .filter(|path| !path.is_empty())
                .map(|config_path| ControlRequestCommand::ReloadConfiguration { config_path })
        }
        "open_window"
            if request.runner_id.is_none()
                && request.session_id.is_none()
                && request.secret.is_none() =>
        {
            Some(ControlRequestCommand::OpenWindow)
        }
        "open_project"
            if request.runner_id.is_none()
                && request.session_id.is_none()
                && request.secret.is_none()
                && request.icon.is_none()
                && request.pane_theme.is_none()
                && request.pane_overlay.is_none()
                && request.pane_overlay_font_size.is_none()
                && request.pane_overlay_opacity.is_none()
                && request.pane_overlay_color.is_none()
                && request.attention_id.is_none()
                && request.attention_summary.is_none()
                && request.attention_body.is_none()
                && request.tab_name.is_none()
                && request.worktree_name.is_none()
                && request.split.is_none()
                && request.profile.is_none()
                && request.theme.is_none()
                && request.pane_request.is_none() =>
        {
            request
                .config_path
                .take()
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
                .map(|root| ControlRequestCommand::OpenProject { root })
        }
        "reload_projects"
            if request.runner_id.is_none()
                && request.session_id.is_none()
                && request.secret.is_none()
                && request.icon.is_none()
                && request.pane_theme.is_none()
                && request.pane_overlay.is_none()
                && request.pane_overlay_font_size.is_none()
                && request.pane_overlay_opacity.is_none()
                && request.pane_overlay_color.is_none()
                && request.attention_id.is_none()
                && request.attention_summary.is_none()
                && request.attention_body.is_none()
                && request.tab_name.is_none()
                && request.worktree_name.is_none()
                && request.config_path.is_none()
                && request.split.is_none()
                && request.profile.is_none()
                && request.theme.is_none()
                && request.pane_request.is_none() =>
        {
            Some(ControlRequestCommand::ReloadProjects)
        }
        "get_silent_mode"
            if request.runner_id.is_none()
                && request.session_id.is_none()
                && request.secret.is_none()
                && request.icon.is_none()
                && request.pane_theme.is_none()
                && request.pane_overlay.is_none()
                && request.pane_overlay_font_size.is_none()
                && request.pane_overlay_opacity.is_none()
                && request.pane_overlay_color.is_none()
                && request.split.is_none()
                && request.profile.is_none()
                && request.theme.is_none() =>
        {
            let attention_id = match request.attention_id.take() {
                Some(0) => return None,
                attention_id => attention_id,
            };
            Some(ControlRequestCommand::GetSilentMode { attention_id })
        }
        "replace_pane"
            if request.runner_id.is_none()
                && request.session_id.is_none()
                && request.secret.is_none()
                && request.icon.is_none()
                && request.pane_theme.is_none()
                && request.pane_overlay.is_none()
                && request.pane_overlay_font_size.is_none()
                && request.pane_overlay_opacity.is_none()
                && request.pane_overlay_color.is_none() =>
        {
            let split = request.split.take();
            let profile = request.profile.take();
            let theme = request.theme.take();
            (split.as_deref().is_none_or(|value| !value.is_empty())
                && profile.as_deref().is_none_or(|value| !value.is_empty())
                && theme.as_deref().is_none_or(|value| !value.is_empty())
                && (split.is_some() || profile.is_some())
                && (theme.is_none() || profile.is_some()))
            .then_some(ControlRequestCommand::ReplacePane {
                split,
                profile,
                theme,
            })
        }
        "run_pane"
            if request.runner_id.is_none()
                && request.session_id.is_none()
                && request.secret.is_none()
                && request.icon.is_none()
                && request.pane_theme.is_none()
                && request.pane_overlay.is_none()
                && request.pane_overlay_font_size.is_none()
                && request.pane_overlay_opacity.is_none()
                && request.pane_overlay_color.is_none()
                && request.attention_id.is_none()
                && request.attention_summary.is_none()
                && request.attention_body.is_none()
                && request.tab_name.is_none()
                && request.worktree_name.is_none()
                && request.config_path.is_none()
                && request.split.is_none()
                && request.profile.is_none()
                && request.theme.is_none() =>
        {
            request
                .pane_request
                .take()
                .and_then(PaneControlRequest::into_command)
                .map(|request| ControlRequestCommand::RunPane { request })
        }
        "list_panes"
            if request.runner_id.is_none()
                && request.session_id.is_none()
                && request.secret.is_none()
                && request.icon.is_none()
                && request.pane_theme.is_none()
                && request.pane_overlay.is_none()
                && request.pane_overlay_font_size.is_none()
                && request.pane_overlay_opacity.is_none()
                && request.pane_overlay_color.is_none()
                && request.attention_id.is_none()
                && request.attention_summary.is_none()
                && request.attention_body.is_none()
                && request.tab_name.is_none()
                && request.worktree_name.is_none()
                && request.config_path.is_none()
                && request.split.is_none()
                && request.profile.is_none()
                && request.theme.is_none()
                && request.pane_request.is_none() =>
        {
            Some(ControlRequestCommand::ListPaneLabels)
        }
        "reconnect_session"
            if request.icon.is_none()
                && request.pane_theme.is_none()
                && request.pane_overlay.is_none()
                && request.pane_overlay_font_size.is_none()
                && request.pane_overlay_opacity.is_none()
                && request.pane_overlay_color.is_none()
                && request.attention_summary.is_none()
                && request.attention_body.is_none()
                && request.tab_name.is_none()
                && request.worktree_name.is_none()
                && request.config_path.is_none()
                && request.split.is_none()
                && request.profile.is_none()
                && request.theme.is_none()
                && request.pane_request.is_none() =>
        {
            let attention_id = match request.attention_id.take() {
                Some(0) => return None,
                attention_id => attention_id,
            };
            request
                .runner_id
                .zip(request.session_id)
                .map(
                    |(runner_id, session_id)| ControlRequestCommand::ReconnectSession {
                        runner_id,
                        session_id,
                        attention_id,
                        secret: request.secret.take().map(SessionSecret::new),
                    },
                )
        }
        "resume_disk_session"
            if request.runner_id.is_none()
                && request.session_id.is_some()
                && request.icon.is_none()
                && request.pane_theme.is_none()
                && request.pane_overlay.is_none()
                && request.pane_overlay_font_size.is_none()
                && request.pane_overlay_opacity.is_none()
                && request.pane_overlay_color.is_none()
                && request.attention_id.is_none()
                && request.attention_summary.is_none()
                && request.attention_body.is_none()
                && request.tab_name.is_none()
                && request.worktree_name.is_none()
                && request.split.is_none()
                && request.profile.is_none()
                && request.theme.is_none()
                && request.pane_request.is_none() =>
        {
            let session_id = request.session_id.take()?;
            // The standalone client sends a JSON object here. Keep the paths
            // and passphrases private to the authenticated local socket and
            // reject malformed or empty entries before they reach the GUI.
            let Some(mut encoded_payload) = request.config_path.take() else {
                zeroize_control_request_secrets(request);
                return None;
            };
            let payload = serde_json::from_str::<ResumeIdentityPayload>(&encoded_payload);
            encoded_payload.zeroize();
            let Ok(payload) = payload else {
                zeroize_control_request_secrets(request);
                return None;
            };
            let identity_paths = payload
                .identity_paths
                .into_iter()
                .map(PathBuf::from)
                .collect::<Vec<_>>();
            let identity_passphrases = payload
                .identity_passphrases
                .into_iter()
                .map(|passphrase| {
                    passphrase.map(|passphrase| SessionSecret::new(passphrase.expose().to_owned()))
                })
                .collect::<Vec<_>>();
            if identity_paths.len() != identity_passphrases.len()
                || identity_paths
                    .iter()
                    .any(|path| path.as_os_str().is_empty())
            {
                zeroize_control_request_secrets(request);
                return None;
            }
            Some(ControlRequestCommand::ResumeDiskSession {
                session_id,
                identity_paths,
                identity_passphrases,
                secret: request.secret.take().map(SessionSecret::new),
            })
        }
        "set_tab_icon"
            if request.runner_id.is_none()
                && request.session_id.is_none()
                && request.secret.is_none() =>
        {
            let icon = match request.icon.take() {
                Some(icon) => Some(icon.parse().ok()?),
                None => None,
            };
            Some(ControlRequestCommand::SetTabIcon { icon })
        }
        "set_pane_theme"
            if request.runner_id.is_none()
                && request.session_id.is_none()
                && request.secret.is_none() =>
        {
            Some(ControlRequestCommand::SetPaneTheme {
                theme: request.pane_theme.take(),
            })
        }
        "list_pane_themes"
            if request.runner_id.is_none()
                && request.session_id.is_none()
                && request.secret.is_none()
                && request.pane_theme.is_none() =>
        {
            Some(ControlRequestCommand::ListPaneThemes)
        }
        "get_pane_theme"
            if request.runner_id.is_none()
                && request.session_id.is_none()
                && request.secret.is_none()
                && request.icon.is_none()
                && request.pane_theme.is_none()
                && request.pane_overlay.is_none()
                && request.pane_overlay_font_size.is_none()
                && request.pane_overlay_opacity.is_none()
                && request.pane_overlay_color.is_none()
                && request.attention_summary.is_none()
                && request.attention_body.is_none()
                && request.tab_name.is_none()
                && request.worktree_name.is_none()
                && request.split.is_none()
                && request.profile.is_none()
                && request.theme.is_none()
                && request.pane_id != Some(0) =>
        {
            Some(ControlRequestCommand::GetPaneTheme {
                attention_id: request.attention_id.take().filter(|id| *id != 0)?,
                pane_id: request.pane_id.take(),
            })
        }
        "set_overlay"
            if request.runner_id.is_none()
                && request.session_id.is_none()
                && request.secret.is_none() =>
        {
            let font_size = match request.pane_overlay_font_size.take() {
                Some(name) => Some(OverlayFontSize::parse(&name)?),
                None => None,
            };
            if let Some(value) = request.pane_overlay_color.as_deref() {
                overlay_color_from_value(value)?;
            }
            Some(ControlRequestCommand::SetPaneOverlay {
                text: request.pane_overlay.take(),
                font_size,
                opacity: request
                    .pane_overlay_opacity
                    .take()
                    .map(|percent| f32::from(percent) / 100.0),
                color: request.pane_overlay_color.take(),
            })
        }
        "set_tab_attention"
            if request.runner_id.is_none()
                && request.session_id.is_none()
                && request.secret.is_none()
                && request.icon.is_none()
                && request.pane_theme.is_none()
                && request.pane_overlay.is_none()
                && request.pane_overlay_font_size.is_none()
                && request.pane_overlay_opacity.is_none()
                && request.pane_overlay_color.is_none()
                && request.config_path.is_none()
                && request.split.is_none()
                && request.profile.is_none()
                && request.theme.is_none() =>
        {
            let attention_id = request.attention_id.take().filter(|id| *id != 0)?;
            let summary = request
                .attention_summary
                .take()
                .filter(|summary| !summary.is_empty())?;
            Some(ControlRequestCommand::SetTabAttention {
                attention_id,
                summary,
                body: request.attention_body.take(),
            })
        }
        "focus_tab"
            if request.runner_id.is_none()
                && request.session_id.is_none()
                && request.secret.is_none()
                && request.icon.is_none()
                && request.pane_theme.is_none()
                && request.pane_overlay.is_none()
                && request.pane_overlay_font_size.is_none()
                && request.pane_overlay_opacity.is_none()
                && request.pane_overlay_color.is_none()
                && request.attention_summary.is_none()
                && request.attention_body.is_none()
                && request.config_path.is_none()
                && request.tab_name.is_none()
                && request.split.is_none()
                && request.profile.is_none()
                && request.theme.is_none() =>
        {
            let attention_id = request.attention_id.take().filter(|id| *id != 0)?;
            Some(ControlRequestCommand::FocusTab { attention_id })
        }
        "set_tab_name"
            if request.runner_id.is_none()
                && request.session_id.is_none()
                && request.secret.is_none()
                && request.icon.is_none()
                && request.pane_theme.is_none()
                && request.pane_overlay.is_none()
                && request.pane_overlay_font_size.is_none()
                && request.pane_overlay_opacity.is_none()
                && request.pane_overlay_color.is_none()
                && request.config_path.is_none()
                && request.attention_summary.is_none()
                && request.attention_body.is_none()
                && request.split.is_none()
                && request.profile.is_none()
                && request.theme.is_none() =>
        {
            let attention_id = request.attention_id.take().filter(|id| *id != 0)?;
            if request.tab_name.as_deref().is_some_and(str::is_empty) {
                return None;
            }
            Some(ControlRequestCommand::SetTabName {
                attention_id,
                name: request.tab_name.take(),
            })
        }
        "set_worktree_name"
            if request.runner_id.is_none()
                && request.session_id.is_none()
                && request.secret.is_none()
                && request.icon.is_none()
                && request.pane_theme.is_none()
                && request.pane_overlay.is_none()
                && request.pane_overlay_font_size.is_none()
                && request.pane_overlay_opacity.is_none()
                && request.pane_overlay_color.is_none()
                && request.config_path.is_none()
                && request.attention_summary.is_none()
                && request.attention_body.is_none()
                && request.tab_name.is_none()
                && request.split.is_none()
                && request.profile.is_none()
                && request.theme.is_none() =>
        {
            let attention_id = request.attention_id.take().filter(|id| *id != 0)?;
            if request.worktree_name.as_deref().is_some_and(str::is_empty) {
                return None;
            }
            Some(ControlRequestCommand::SetWorktreeName {
                attention_id,
                name: request.worktree_name.take(),
            })
        }
        _ => None,
    };
    if command.is_none() {
        zeroize_control_request_secrets(request);
    }
    command
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

pub(crate) fn request_existing_process_window() -> Result<bool> {
    let directory = crate::background_sessions::session_catalog_dir();
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("reading Zetta process control endpoints"),
    };
    for entry in entries {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("control-") && name.ends_with(".json"))
        {
            continue;
        }
        let endpoint = match fs::read(&path)
            .ok()
            .and_then(|contents| serde_json::from_slice::<ControlEndpoint>(&contents).ok())
        {
            Some(endpoint) if endpoint.version == CONTROL_VERSION => endpoint,
            _ => continue,
        };
        if !process_is_running(endpoint.process_id) {
            let _ = fs::remove_file(path);
            let _ = fs::remove_file(endpoint.socket_path);
            continue;
        }
        if send_open_window_request(&endpoint).unwrap_or(false) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn request_existing_process_project(root: &Path) -> Result<bool> {
    let directory = crate::background_sessions::session_catalog_dir();
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("reading Zetta process control endpoints"),
    };
    for entry in entries {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("control-") && name.ends_with(".json"))
        {
            continue;
        }
        let endpoint = match fs::read(&path)
            .ok()
            .and_then(|contents| serde_json::from_slice::<ControlEndpoint>(&contents).ok())
        {
            Some(endpoint) if endpoint.version == CONTROL_VERSION => endpoint,
            _ => continue,
        };
        if !process_is_running(endpoint.process_id) {
            let _ = fs::remove_file(path);
            let _ = fs::remove_file(endpoint.socket_path);
            continue;
        }
        if send_open_project_request(&endpoint, root).unwrap_or(false) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn request_existing_process_projects_reload() -> Result<bool> {
    let directory = crate::background_sessions::session_catalog_dir();
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("reading Zetta process control endpoints"),
    };
    let mut accepted = false;
    for entry in entries {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("control-") && name.ends_with(".json"))
        {
            continue;
        }
        let endpoint = match fs::read(&path)
            .ok()
            .and_then(|contents| serde_json::from_slice::<ControlEndpoint>(&contents).ok())
        {
            Some(endpoint) if endpoint.version == CONTROL_VERSION => endpoint,
            _ => continue,
        };
        if !process_is_running(endpoint.process_id) {
            let _ = fs::remove_file(path);
            let _ = fs::remove_file(endpoint.socket_path);
            continue;
        }
        accepted |= send_reload_projects_request(&endpoint).unwrap_or(false);
    }
    Ok(accepted)
}

pub(crate) fn request_existing_process_replace_pane(request: ReplacePaneRequest) -> Result<bool> {
    let directory = crate::background_sessions::session_catalog_dir();
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("reading Zetta process control endpoints"),
    };
    for entry in entries {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("control-") && name.ends_with(".json"))
        {
            continue;
        }
        let endpoint = match fs::read(&path)
            .ok()
            .and_then(|contents| serde_json::from_slice::<ControlEndpoint>(&contents).ok())
        {
            Some(endpoint) if endpoint.version == CONTROL_VERSION => endpoint,
            _ => continue,
        };
        if !process_is_running(endpoint.process_id) {
            let _ = fs::remove_file(path);
            let _ = fs::remove_file(endpoint.socket_path);
            continue;
        }
        if send_replace_pane_request(&endpoint, &request).unwrap_or(false) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn request_existing_process_pane(request: PaneCommand) -> Result<bool> {
    let directory = crate::background_sessions::session_catalog_dir();
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("reading Zetta process control endpoints"),
    };
    let mut last_error = None;
    for entry in entries {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("control-") && name.ends_with(".json"))
        {
            continue;
        }
        let endpoint = match fs::read(&path)
            .ok()
            .and_then(|contents| serde_json::from_slice::<ControlEndpoint>(&contents).ok())
        {
            Some(endpoint) if endpoint.version == CONTROL_VERSION => endpoint,
            _ => continue,
        };
        if !process_is_running(endpoint.process_id) {
            let _ = fs::remove_file(path);
            let _ = fs::remove_file(endpoint.socket_path);
            continue;
        }
        match send_run_pane_request(&endpoint, &request) {
            Ok(true) => return Ok(true),
            Ok(false) => {}
            Err(error) => last_error = Some(error),
        }
    }
    if let Some(error) = last_error {
        return Err(error);
    }
    Ok(false)
}

pub(crate) fn request_existing_process_pane_labels() -> Result<Option<Vec<String>>> {
    let directory = crate::background_sessions::session_catalog_dir();
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("reading Zetta process control endpoints"),
    };
    let mut last_error = None;
    for entry in entries {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("control-") && name.ends_with(".json"))
        {
            continue;
        }
        let endpoint = match fs::read(&path)
            .ok()
            .and_then(|contents| serde_json::from_slice::<ControlEndpoint>(&contents).ok())
        {
            Some(endpoint) if endpoint.version == CONTROL_VERSION => endpoint,
            _ => continue,
        };
        if !process_is_running(endpoint.process_id) {
            let _ = fs::remove_file(path);
            let _ = fs::remove_file(endpoint.socket_path);
            continue;
        }
        match send_list_pane_labels_request(&endpoint) {
            Ok(Some(labels)) => return Ok(Some(labels)),
            Ok(None) => {}
            Err(error) => last_error = Some(error),
        }
    }
    if let Some(error) = last_error {
        return Err(error);
    }
    Ok(None)
}

pub(crate) fn request_existing_process_tab_icon(icon: Option<IconName>) -> Result<bool> {
    let directory = crate::background_sessions::session_catalog_dir();
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("reading Zetta process control endpoints"),
    };
    for entry in entries {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("control-") && name.ends_with(".json"))
        {
            continue;
        }
        let endpoint = match fs::read(&path)
            .ok()
            .and_then(|contents| serde_json::from_slice::<ControlEndpoint>(&contents).ok())
        {
            Some(endpoint) if endpoint.version == CONTROL_VERSION => endpoint,
            _ => continue,
        };
        if !process_is_running(endpoint.process_id) {
            let _ = fs::remove_file(path);
            let _ = fs::remove_file(endpoint.socket_path);
            continue;
        }
        if send_set_tab_icon_request(&endpoint, icon).unwrap_or(false) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn request_existing_process_pane_theme(theme: Option<String>) -> Result<bool> {
    let directory = crate::background_sessions::session_catalog_dir();
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("reading Zetta process control endpoints"),
    };
    for entry in entries {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("control-") && name.ends_with(".json"))
        {
            continue;
        }
        let endpoint = match fs::read(&path)
            .ok()
            .and_then(|contents| serde_json::from_slice::<ControlEndpoint>(&contents).ok())
        {
            Some(endpoint) if endpoint.version == CONTROL_VERSION => endpoint,
            _ => continue,
        };
        if !process_is_running(endpoint.process_id) {
            let _ = fs::remove_file(path);
            let _ = fs::remove_file(endpoint.socket_path);
            continue;
        }
        if send_set_pane_theme_request(&endpoint, theme.clone()).unwrap_or(false) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn request_existing_process_pane_theme_list() -> Result<Option<Vec<String>>> {
    let directory = crate::background_sessions::session_catalog_dir();
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("reading Zetta process control endpoints"),
    };
    for entry in entries {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("control-") && name.ends_with(".json"))
        {
            continue;
        }
        let endpoint = match fs::read(&path)
            .ok()
            .and_then(|contents| serde_json::from_slice::<ControlEndpoint>(&contents).ok())
        {
            Some(endpoint) if endpoint.version == CONTROL_VERSION => endpoint,
            _ => continue,
        };
        if !process_is_running(endpoint.process_id) {
            let _ = fs::remove_file(path);
            let _ = fs::remove_file(endpoint.socket_path);
            continue;
        }
        if let Some(themes) = send_list_pane_themes_request(&endpoint).unwrap_or(None) {
            return Ok(Some(themes));
        }
    }
    Ok(None)
}

pub(crate) fn request_existing_process_pane_overlay(request: PaneOverlayRequest) -> Result<bool> {
    let directory = crate::background_sessions::session_catalog_dir();
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("reading Zetta process control endpoints"),
    };
    for entry in entries {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("control-") && name.ends_with(".json"))
        {
            continue;
        }
        let endpoint = match fs::read(&path)
            .ok()
            .and_then(|contents| serde_json::from_slice::<ControlEndpoint>(&contents).ok())
        {
            Some(endpoint) if endpoint.version == CONTROL_VERSION => endpoint,
            _ => continue,
        };
        if !process_is_running(endpoint.process_id) {
            let _ = fs::remove_file(path);
            let _ = fs::remove_file(endpoint.socket_path);
            continue;
        }
        if send_set_overlay_request(&endpoint, &request).unwrap_or(false) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn request_existing_process_configuration_reload(path: &Path) -> Result<bool> {
    let directory = crate::background_sessions::session_catalog_dir();
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("reading Zetta process control endpoints"),
    };
    let config_path = config_path_identity(path);
    for entry in entries {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("control-") && name.ends_with(".json"))
        {
            continue;
        }
        let endpoint = match fs::read(&path)
            .ok()
            .and_then(|contents| serde_json::from_slice::<ControlEndpoint>(&contents).ok())
        {
            Some(endpoint) if endpoint.version == CONTROL_VERSION => endpoint,
            _ => continue,
        };
        if !process_is_running(endpoint.process_id) {
            let _ = fs::remove_file(path);
            let _ = fs::remove_file(endpoint.socket_path);
            continue;
        }
        if send_reload_configuration_request(&endpoint, &config_path).unwrap_or(false) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn request_process_tab_attention(
    process_id: u32,
    request: TabAttentionRequest,
) -> Result<bool> {
    let endpoint_path = control_endpoint_path(process_id);
    let contents = fs::read(&endpoint_path).with_context(|| {
        format!(
            "reading Zetta process control endpoint {}",
            endpoint_path.display()
        )
    })?;
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&contents).context("parsing Zetta process control endpoint")?;
    anyhow::ensure!(
        endpoint.version == CONTROL_VERSION && endpoint.process_id == process_id,
        "Zetta process control endpoint is outdated"
    );
    send_set_tab_attention_request(&endpoint, &request)
}

#[cfg(feature = "syntax-highlighting")]
pub(crate) struct ProcessPaneThemeQuery {
    endpoint: ControlEndpoint,
    attention_id: u64,
    pane_id: Option<u64>,
}

#[cfg(feature = "syntax-highlighting")]
impl ProcessPaneThemeQuery {
    pub(crate) fn new(process_id: u32, attention_id: u64, pane_id: Option<u64>) -> Result<Self> {
        anyhow::ensure!(process_id != 0, "process ID must be positive");
        anyhow::ensure!(attention_id != 0, "attention ID must be positive");
        anyhow::ensure!(pane_id != Some(0), "pane ID must be positive");
        let endpoint_path = control_endpoint_path(process_id);
        let contents = fs::read(&endpoint_path).with_context(|| {
            format!(
                "reading Zetta process control endpoint {}",
                endpoint_path.display()
            )
        })?;
        let endpoint: ControlEndpoint =
            serde_json::from_slice(&contents).context("parsing Zetta process control endpoint")?;
        anyhow::ensure!(
            endpoint.version == CONTROL_VERSION && endpoint.process_id == process_id,
            "Zetta process control endpoint is outdated"
        );
        Ok(Self {
            endpoint,
            attention_id,
            pane_id,
        })
    }

    pub(crate) fn theme_name(&self) -> Result<Option<String>> {
        send_get_pane_theme_request(&self.endpoint, self.attention_id, self.pane_id)
    }
}

#[cfg(feature = "notifications")]
pub(crate) fn request_process_silent_mode(
    process_id: u32,
    attention_id: Option<u64>,
) -> Result<bool> {
    anyhow::ensure!(process_id != 0, "process ID must be positive");
    anyhow::ensure!(attention_id != Some(0), "attention ID must be positive");
    let endpoint_path = control_endpoint_path(process_id);
    let contents = fs::read(&endpoint_path).with_context(|| {
        format!(
            "reading Zetta process control endpoint {}",
            endpoint_path.display()
        )
    })?;
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&contents).context("parsing Zetta process control endpoint")?;
    anyhow::ensure!(
        endpoint.version == CONTROL_VERSION && endpoint.process_id == process_id,
        "Zetta process control endpoint is outdated"
    );
    send_get_silent_mode_request(&endpoint, attention_id)
}

#[cfg(feature = "notifications")]
#[allow(dead_code)]
pub(crate) fn request_process_focus_tab(process_id: u32, attention_id: u64) -> Result<bool> {
    anyhow::ensure!(process_id != 0, "process ID must be positive");
    anyhow::ensure!(attention_id != 0, "attention ID must be positive");
    let endpoint_path = control_endpoint_path(process_id);
    let contents = fs::read(&endpoint_path).with_context(|| {
        format!(
            "reading Zetta process control endpoint {}",
            endpoint_path.display()
        )
    })?;
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&contents).context("parsing Zetta process control endpoint")?;
    anyhow::ensure!(
        endpoint.version == CONTROL_VERSION && endpoint.process_id == process_id,
        "Zetta process control endpoint is outdated"
    );
    send_focus_tab_request(&endpoint, attention_id)
}

// Kept for process-control callers for protocol compatibility. The request is
// honored outside a worktree and is masked by the active worktree title.
#[allow(dead_code)]
pub(crate) fn request_process_tab_name(process_id: u32, request: TabNameRequest) -> Result<bool> {
    let endpoint_path = control_endpoint_path(process_id);
    let contents = fs::read(&endpoint_path).with_context(|| {
        format!(
            "reading Zetta process control endpoint {}",
            endpoint_path.display()
        )
    })?;
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&contents).context("parsing Zetta process control endpoint")?;
    anyhow::ensure!(
        endpoint.version == CONTROL_VERSION && endpoint.process_id == process_id,
        "Zetta process control endpoint is outdated"
    );
    send_set_tab_name_request(&endpoint, &request)
}

pub(crate) fn request_process_worktree_name(
    process_id: u32,
    request: WorktreeNameRequest,
) -> Result<bool> {
    anyhow::ensure!(process_id != 0, "process ID must be positive");
    anyhow::ensure!(request.attention_id != 0, "attention ID must be positive");
    let endpoint_path = control_endpoint_path(process_id);
    let contents = fs::read(&endpoint_path).with_context(|| {
        format!(
            "reading Zetta process control endpoint {}",
            endpoint_path.display()
        )
    })?;
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&contents).context("parsing Zetta process control endpoint")?;
    anyhow::ensure!(
        endpoint.version == CONTROL_VERSION && endpoint.process_id == process_id,
        "Zetta process control endpoint is outdated"
    );
    send_set_worktree_name_request(&endpoint, &request)
}

fn send_open_window_request(endpoint: &ControlEndpoint) -> Result<bool> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: "open_window".to_owned(),
            runner_id: None,
            session_id: None,
            secret: None,
            icon: None,
            pane_theme: None,
            pane_id: None,
            pane_overlay: None,
            pane_overlay_font_size: None,
            pane_overlay_opacity: None,
            pane_overlay_color: None,
            attention_id: None,
            attention_summary: None,
            attention_body: None,
            tab_name: None,
            worktree_name: None,
            config_path: None,
            split: None,
            profile: None,
            theme: None,
            pane_request: None,
        },
    )?;
    let response = read_message::<ControlResponse>(&mut stream)?;
    Ok(response.status == "ok")
}

fn send_open_project_request(endpoint: &ControlEndpoint, root: &Path) -> Result<bool> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: "open_project".to_owned(),
            runner_id: None,
            session_id: None,
            secret: None,
            icon: None,
            pane_theme: None,
            pane_id: None,
            pane_overlay: None,
            pane_overlay_font_size: None,
            pane_overlay_opacity: None,
            pane_overlay_color: None,
            attention_id: None,
            attention_summary: None,
            attention_body: None,
            tab_name: None,
            worktree_name: None,
            config_path: Some(root.to_string_lossy().into_owned()),
            split: None,
            profile: None,
            theme: None,
            pane_request: None,
        },
    )?;
    let response = read_message::<ControlResponse>(&mut stream)?;
    Ok(response.status == "ok")
}

fn send_reload_projects_request(endpoint: &ControlEndpoint) -> Result<bool> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: "reload_projects".to_owned(),
            runner_id: None,
            session_id: None,
            secret: None,
            icon: None,
            pane_theme: None,
            pane_id: None,
            pane_overlay: None,
            pane_overlay_font_size: None,
            pane_overlay_opacity: None,
            pane_overlay_color: None,
            attention_id: None,
            attention_summary: None,
            attention_body: None,
            tab_name: None,
            worktree_name: None,
            config_path: None,
            split: None,
            profile: None,
            theme: None,
            pane_request: None,
        },
    )?;
    let response = read_message::<ControlResponse>(&mut stream)?;
    Ok(response.status == "ok")
}

#[cfg(feature = "notifications")]
fn send_get_silent_mode_request(
    endpoint: &ControlEndpoint,
    attention_id: Option<u64>,
) -> Result<bool> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: "get_silent_mode".to_owned(),
            runner_id: None,
            session_id: None,
            secret: None,
            icon: None,
            pane_theme: None,
            pane_id: None,
            pane_overlay: None,
            pane_overlay_font_size: None,
            pane_overlay_opacity: None,
            pane_overlay_color: None,
            attention_id,
            attention_summary: None,
            attention_body: None,
            tab_name: None,
            worktree_name: None,
            config_path: None,
            split: None,
            profile: None,
            theme: None,
            pane_request: None,
        },
    )?;
    let response = read_message::<ControlResponse>(&mut stream)?;
    anyhow::ensure!(
        response.status == "ok",
        "target process rejected silent mode query"
    );
    Ok(response.silent_mode)
}

fn send_set_tab_attention_request(
    endpoint: &ControlEndpoint,
    request: &TabAttentionRequest,
) -> Result<bool> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: "set_tab_attention".to_owned(),
            runner_id: None,
            session_id: None,
            secret: None,
            icon: None,
            pane_theme: None,
            pane_id: None,
            pane_overlay: None,
            pane_overlay_font_size: None,
            pane_overlay_opacity: None,
            pane_overlay_color: None,
            attention_id: Some(request.attention_id),
            attention_summary: Some(request.summary.clone()),
            attention_body: request.body.clone(),
            tab_name: None,
            worktree_name: None,
            config_path: None,
            split: None,
            profile: None,
            theme: None,
            pane_request: None,
        },
    )?;
    let response = read_message::<ControlResponse>(&mut stream)?;
    Ok(response.status == "ok")
}

#[allow(dead_code)]
fn send_set_tab_name_request(endpoint: &ControlEndpoint, request: &TabNameRequest) -> Result<bool> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: "set_tab_name".to_owned(),
            runner_id: None,
            session_id: None,
            secret: None,
            icon: None,
            pane_theme: None,
            pane_id: None,
            pane_overlay: None,
            pane_overlay_font_size: None,
            pane_overlay_opacity: None,
            pane_overlay_color: None,
            attention_id: Some(request.attention_id),
            attention_summary: None,
            attention_body: None,
            tab_name: request.name.clone(),
            worktree_name: None,
            config_path: None,
            split: None,
            profile: None,
            theme: None,
            pane_request: None,
        },
    )?;
    let response = read_message::<ControlResponse>(&mut stream)?;
    Ok(response.status == "ok")
}

fn send_set_worktree_name_request(
    endpoint: &ControlEndpoint,
    request: &WorktreeNameRequest,
) -> Result<bool> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: "set_worktree_name".to_owned(),
            runner_id: None,
            session_id: None,
            secret: None,
            icon: None,
            pane_theme: None,
            pane_id: None,
            pane_overlay: None,
            pane_overlay_font_size: None,
            pane_overlay_opacity: None,
            pane_overlay_color: None,
            attention_id: Some(request.attention_id),
            attention_summary: None,
            attention_body: None,
            tab_name: None,
            worktree_name: request.name.clone(),
            config_path: None,
            split: None,
            profile: None,
            theme: None,
            pane_request: None,
        },
    )?;
    let response = read_message::<ControlResponse>(&mut stream)?;
    Ok(response.status == "ok")
}

#[cfg(feature = "notifications")]
#[allow(dead_code)]
fn send_focus_tab_request(endpoint: &ControlEndpoint, attention_id: u64) -> Result<bool> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: "focus_tab".to_owned(),
            runner_id: None,
            session_id: None,
            secret: None,
            icon: None,
            pane_theme: None,
            pane_id: None,
            pane_overlay: None,
            pane_overlay_font_size: None,
            pane_overlay_opacity: None,
            pane_overlay_color: None,
            attention_id: Some(attention_id),
            attention_summary: None,
            attention_body: None,
            tab_name: None,
            worktree_name: None,
            config_path: None,
            split: None,
            profile: None,
            theme: None,
            pane_request: None,
        },
    )?;
    let response = read_message::<ControlResponse>(&mut stream)?;
    Ok(response.status == "ok")
}

fn send_run_pane_request(endpoint: &ControlEndpoint, request: &PaneCommand) -> Result<bool> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: "run_pane".to_owned(),
            runner_id: None,
            session_id: None,
            secret: None,
            icon: None,
            pane_theme: None,
            pane_id: None,
            pane_overlay: None,
            pane_overlay_font_size: None,
            pane_overlay_opacity: None,
            pane_overlay_color: None,
            attention_id: None,
            attention_summary: None,
            attention_body: None,
            tab_name: None,
            worktree_name: None,
            config_path: None,
            split: None,
            profile: None,
            theme: None,
            pane_request: Some(request.into()),
        },
    )?;
    let response = read_message::<ControlResponse>(&mut stream)?;
    if response.status == "ok" {
        return Ok(true);
    }
    if let Some(error) = response.error {
        anyhow::bail!("{}: {}", error.code, error.message);
    }
    Ok(false)
}

fn send_list_pane_labels_request(endpoint: &ControlEndpoint) -> Result<Option<Vec<String>>> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: "list_panes".to_owned(),
            runner_id: None,
            session_id: None,
            secret: None,
            icon: None,
            pane_theme: None,
            pane_id: None,
            pane_overlay: None,
            pane_overlay_font_size: None,
            pane_overlay_opacity: None,
            pane_overlay_color: None,
            attention_id: None,
            attention_summary: None,
            attention_body: None,
            tab_name: None,
            worktree_name: None,
            config_path: None,
            split: None,
            profile: None,
            theme: None,
            pane_request: None,
        },
    )?;
    let response = read_message::<ControlResponse>(&mut stream)?;
    if response.status == "ok" {
        return Ok(Some(response.pane_labels));
    }
    if let Some(error) = response.error {
        anyhow::bail!("{}: {}", error.code, error.message);
    }
    Ok(None)
}

fn send_replace_pane_request(
    endpoint: &ControlEndpoint,
    request: &ReplacePaneRequest,
) -> Result<bool> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: "replace_pane".to_owned(),
            runner_id: None,
            session_id: None,
            secret: None,
            icon: None,
            pane_theme: None,
            pane_id: None,
            pane_overlay: None,
            pane_overlay_font_size: None,
            pane_overlay_opacity: None,
            pane_overlay_color: None,
            attention_id: None,
            attention_summary: None,
            attention_body: None,
            tab_name: None,
            worktree_name: None,
            config_path: None,
            split: request.split.clone(),
            profile: request.profile.clone(),
            theme: request.theme.clone(),
            pane_request: None,
        },
    )?;
    let response = read_message::<ControlResponse>(&mut stream)?;
    Ok(response.status == "ok")
}

fn send_reload_configuration_request(
    endpoint: &ControlEndpoint,
    config_path: &str,
) -> Result<bool> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: "reload_configuration".to_owned(),
            runner_id: None,
            session_id: None,
            secret: None,
            icon: None,
            pane_theme: None,
            pane_id: None,
            pane_overlay: None,
            pane_overlay_font_size: None,
            pane_overlay_opacity: None,
            pane_overlay_color: None,
            attention_id: None,
            attention_summary: None,
            attention_body: None,
            tab_name: None,
            worktree_name: None,
            config_path: Some(config_path.to_owned()),
            split: None,
            profile: None,
            theme: None,
            pane_request: None,
        },
    )?;
    let response = read_message::<ControlResponse>(&mut stream)?;
    Ok(response.status == "ok")
}

fn send_set_tab_icon_request(endpoint: &ControlEndpoint, icon: Option<IconName>) -> Result<bool> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: "set_tab_icon".to_owned(),
            runner_id: None,
            session_id: None,
            secret: None,
            icon: icon.map(|icon| {
                let name: &'static str = icon.into();
                name.to_owned()
            }),
            pane_theme: None,
            pane_id: None,
            pane_overlay: None,
            pane_overlay_font_size: None,
            pane_overlay_opacity: None,
            pane_overlay_color: None,
            attention_id: None,
            attention_summary: None,
            attention_body: None,
            tab_name: None,
            worktree_name: None,
            config_path: None,
            split: None,
            profile: None,
            theme: None,
            pane_request: None,
        },
    )?;
    let response = read_message::<ControlResponse>(&mut stream)?;
    Ok(response.status == "ok")
}

fn send_set_pane_theme_request(endpoint: &ControlEndpoint, theme: Option<String>) -> Result<bool> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: "set_pane_theme".to_owned(),
            runner_id: None,
            session_id: None,
            secret: None,
            icon: None,
            pane_theme: theme,
            pane_id: None,
            pane_overlay: None,
            pane_overlay_font_size: None,
            pane_overlay_opacity: None,
            pane_overlay_color: None,
            attention_id: None,
            attention_summary: None,
            attention_body: None,
            tab_name: None,
            worktree_name: None,
            config_path: None,
            split: None,
            profile: None,
            theme: None,
            pane_request: None,
        },
    )?;
    let response = read_message::<ControlResponse>(&mut stream)?;
    Ok(response.status == "ok")
}

fn send_list_pane_themes_request(endpoint: &ControlEndpoint) -> Result<Option<Vec<String>>> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: "list_pane_themes".to_owned(),
            runner_id: None,
            session_id: None,
            secret: None,
            icon: None,
            pane_theme: None,
            pane_id: None,
            pane_overlay: None,
            pane_overlay_font_size: None,
            pane_overlay_opacity: None,
            pane_overlay_color: None,
            attention_id: None,
            attention_summary: None,
            attention_body: None,
            tab_name: None,
            worktree_name: None,
            config_path: None,
            split: None,
            profile: None,
            theme: None,
            pane_request: None,
        },
    )?;
    let response = read_message::<ControlResponse>(&mut stream)?;
    Ok((response.status == "ok").then_some(response.themes))
}

#[cfg(feature = "syntax-highlighting")]
fn send_get_pane_theme_request(
    endpoint: &ControlEndpoint,
    attention_id: u64,
    pane_id: Option<u64>,
) -> Result<Option<String>> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: "get_pane_theme".to_owned(),
            runner_id: None,
            session_id: None,
            secret: None,
            icon: None,
            pane_theme: None,
            pane_id,
            pane_overlay: None,
            pane_overlay_font_size: None,
            pane_overlay_opacity: None,
            pane_overlay_color: None,
            attention_id: Some(attention_id),
            attention_summary: None,
            attention_body: None,
            tab_name: None,
            worktree_name: None,
            config_path: None,
            split: None,
            profile: None,
            theme: None,
            pane_request: None,
        },
    )?;
    let response = read_message::<ControlResponse>(&mut stream)?;
    Ok((response.status == "ok")
        .then_some(response.pane_theme)
        .flatten())
}

fn send_set_overlay_request(
    endpoint: &ControlEndpoint,
    request: &PaneOverlayRequest,
) -> Result<bool> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: "set_overlay".to_owned(),
            runner_id: None,
            session_id: None,
            secret: None,
            icon: None,
            pane_theme: None,
            pane_id: None,
            pane_overlay: request.text.clone(),
            pane_overlay_font_size: request
                .font_size
                .map(OverlayFontSize::cli_name)
                .map(str::to_owned),
            pane_overlay_opacity: request.opacity,
            pane_overlay_color: request.color.clone(),
            attention_id: None,
            attention_summary: None,
            attention_body: None,
            tab_name: None,
            worktree_name: None,
            config_path: None,
            split: None,
            profile: None,
            theme: None,
            pane_request: None,
        },
    )?;
    let response = read_message::<ControlResponse>(&mut stream)?;
    Ok(response.status == "ok")
}

/// Reads one newline-framed message. A reconnect request carries the session
/// secret in this buffer, so it is zeroized on every exit path rather than left
/// in freed heap memory.
fn read_message<T: DeserializeOwned>(stream: &mut UnixStream) -> Result<T> {
    let mut bytes = Zeroizing::new(Vec::new());
    let mut reader = BufReader::new(stream).take((MAX_CONTROL_MESSAGE_BYTES + 1) as u64);
    reader.read_until(b'\n', &mut bytes)?;
    anyhow::ensure!(
        bytes.last() == Some(&b'\n'),
        "process control message is too long or incomplete"
    );
    bytes.pop();
    serde_json::from_slice(&bytes).context("parsing process control message")
}

fn write_message(stream: &mut UnixStream, message: &impl Serialize) -> Result<()> {
    serde_json::to_writer(&mut *stream, message)?;
    stream.write_all(b"\n")?;
    Ok(())
}

fn random_hex(byte_count: usize) -> Result<String> {
    let mut bytes = vec![0; byte_count];
    getrandom::fill(&mut bytes)?;
    Ok(encode_hex(&bytes))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0xf) as usize] as char);
    }
    encoded
}

fn process_is_running(process_id: u32) -> bool {
    let process_id = Pid::from_u32(process_id);
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[process_id]), true);
    system.process(process_id).is_some()
}

pub(crate) fn config_path_identity(path: &Path) -> String {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let absolute = fs::canonicalize(&absolute).unwrap_or(absolute);
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    #[cfg(windows)]
    return normalized
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    #[cfg(not(windows))]
    normalized.to_string_lossy().into_owned()
}

fn control_endpoint_path(process_id: u32) -> PathBuf {
    crate::background_sessions::session_catalog_dir().join(format!("control-{process_id}.json"))
}

fn control_socket_path(endpoint_path: &Path) -> PathBuf {
    endpoint_path.with_extension("sock")
}

/// Restricts the bound control socket to the current user. Windows places the
/// endpoint under per-user `%APPDATA%`, so only unix needs an explicit mode.
fn restrict_socket_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).with_context(|| {
            format!(
                "restricting the Zetta process control socket {}",
                path.display()
            )
        })?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn remove_socket_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("removing stale socket {}", path.display()))
        }
    }
}

fn write_endpoint(path: &Path, endpoint: &ControlEndpoint) -> Result<()> {
    let parent = path.parent().context("control endpoint has no parent")?;
    crate::background_sessions::create_private_dir(parent)?;
    let temporary = path.with_extension("json.tmp");
    let contents = serde_json::to_vec(endpoint)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?
            .write_all(&contents)?;
    }
    #[cfg(not(unix))]
    fs::write(&temporary, contents)?;
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(temporary, path)?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/process_control.rs"]
mod tests;
