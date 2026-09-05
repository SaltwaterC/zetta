//! The per-process control socket.
//!
//! This module owns the wire format: [`ControlRequest`], [`ControlResponse`],
//! the payload types they carry, and the [`ControlRequestCommand`] a decoded
//! request becomes. Those types stay here rather than in a submodule so the
//! four halves below can read their private fields, which is also why the
//! decoder can enforce which fields each command may carry.
//!
//! - `server.rs` — the listener thread and the completion waits.
//! - `decode.rs` — [`ControlRequest`] to [`ControlRequestCommand`].
//! - `client.rs` — one function per `zetta` subcommand that reaches a window.
//! - `endpoint.rs` — finding the endpoints of running processes.

use std::{
    collections::{BTreeMap, HashSet},
    env, fs,
    io::{BufRead as _, BufReader, Read as _, Write as _},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel},
    },
    thread,
    time::{Duration, Instant},
};
use zeroize::{Zeroize as _, Zeroizing};

#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{self, AtomicU64};
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
    MAX_PANE_COMMAND_BYTES, PaneCommand, ShellCommandRequest, pane_command_byte_len,
    parse_pane_direction,
};
use crate::pane::{OverlayFontSize, PaneOverlayRequest, overlay_color_from_value};
use crate::project_commands::{
    MAX_SHELL_COMMAND_BYTES, validate_command_environment_entry, validate_command_string,
};
use crate::run_command::{
    RunPaneIdentity, RunRegistration, RunResolution, RunWaitRequest, process_run_registry,
};

mod client;
mod decode;
mod endpoint;
mod server;

#[cfg(feature = "syntax-highlighting")]
pub(crate) use client::PaneThemeAnswer;
#[cfg(feature = "syntax-highlighting")]
pub(crate) use client::ProcessPaneThemeQuery;
pub(crate) use client::{
    request_existing_process_command, request_existing_process_configuration_reload,
    request_existing_process_new_window, request_existing_process_pane,
    request_existing_process_pane_labels, request_existing_process_pane_overlay,
    request_existing_process_project_with_working_directory,
    request_existing_process_projects_reload, request_existing_process_replace_pane,
    request_existing_process_shell_command, request_existing_process_tab_icon,
    request_existing_process_theme, request_existing_process_theme_list,
    request_existing_process_window, request_process_run_wait, request_process_tab_attention,
};
#[cfg(feature = "notifications")]
pub(crate) use client::{request_process_focus_tab, request_process_silent_mode};
pub(crate) use endpoint::config_path_identity;
pub(crate) use server::ProcessControlServer;

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
///
/// 18 adds an optional working directory to open-project requests, so opening
/// a registered project from a managed worktree can preserve that directory.
///
/// 19 adds a raw shell-command request for registered project commands.
///
/// A New Window request may carry an optional profile name and a short-lived Wayland activation token in
/// the private string payload so an existing process can focus its surface.
///
/// The current version also includes an explicit fresh-window request, which
/// must not resume a dormant session the way the existing plain-launch request
/// does, and a `pane_theme_revision` on `get_pane_theme` in both directions: a
/// client that sends the revision it already has is answered `unchanged` by the
/// connection thread, without the main thread being involved.
pub(crate) const CONTROL_VERSION: u32 = zmux::protocol::CONTROL_VERSION;
// A 64 KiB argv payload can expand substantially when it contains many
// one-character arguments and each value is represented as JSON. Keep enough
// framing headroom for that worst case as well as the endpoint token.
const MAX_CONTROL_MESSAGE_BYTES: usize = 256 * 1024;
const MAX_ACTIVATION_TOKEN_BYTES: usize = 4096;
const CONTROL_COMPLETION_TIMEOUT: Duration = Duration::from_secs(2);
/// Bumped whenever anything that can change a pane's resolved theme happens.
///
/// `zetta vi` watches its pane's theme so an open editor recolours. It cannot be
/// pushed to — the control socket only runs client to server — so it polls. The
/// point of this counter is that the *server thread* can read it: an unchanged
/// revision is answered without waking the thread that draws, which is what the
/// poll used to cost twice a second for the life of the editor.
///
/// Over-bumping is harmless — it costs one extra query. Under-bumping is what
/// would leave an editor showing stale colours, which is why the watcher also
/// forces a full answer periodically rather than trusting this alone.
static PANE_THEME_REVISION: AtomicU64 = AtomicU64::new(1);

/// Records that a pane's theme may now resolve differently.
pub(crate) fn bump_pane_theme_revision() {
    PANE_THEME_REVISION.fetch_add(1, atomic::Ordering::Release);
}

pub(crate) fn pane_theme_revision() -> u64 {
    PANE_THEME_REVISION.load(atomic::Ordering::Acquire)
}

const CONTROL_COMPLETION_POLL_INTERVAL: Duration = Duration::from_millis(25);
/// How often a `zetta pane wait` connection re-checks the things that cannot
/// wake it: the shutdown flag, and whether its client has gone.
///
/// Deliberately much longer than [`CONTROL_COMPLETION_POLL_INTERVAL`]. The event
/// these loops exist for — the run resolving, or the client sending its
/// completion — arrives on a channel and wakes the `recv_timeout` immediately,
/// so this interval adds nothing to the latency a user can observe. It only
/// bounds how quickly an abandoned wait is reaped, and a wait can outlive a long
/// command, so waking forty times a second to learn nothing is the wrong trade.
const RUN_WAIT_SUPERVISION_INTERVAL: Duration = Duration::from_millis(250);
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
const RECONNECT_COMPLETION_TIMEOUT: Duration = Duration::from_secs(60);

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
    #[cfg(target_os = "macos")]
    OpenUrls(Vec<String>),
    #[cfg(windows)]
    OpenWindowsHandoff {
        request: crate::windows_integration::WindowsHandoffRequest,
        completion: Sender<bool>,
    },
    ReloadConfiguration {
        config_path: String,
        completion: Sender<bool>,
    },
    OpenWindow {
        completion: Sender<bool>,
    },
    OpenNewWindow {
        profile: Option<String>,
        activation_token: Option<String>,
        completion: Sender<bool>,
    },
    OpenProject {
        root: PathBuf,
        working_directory: Option<PathBuf>,
        completion: Sender<bool>,
    },
    ReloadProjects {
        completion: Sender<bool>,
    },
    ReplacePane {
        request: ReplacePaneRequest,
        completion: Sender<bool>,
    },
    OpenCommand {
        request: PaneCommand,
        working_directory: Option<PathBuf>,
        completion: Sender<bool>,
    },
    RunPane {
        request: PaneCommand,
        completion: Sender<std::result::Result<(), String>>,
    },
    RunShellCommand {
        request: ShellCommandRequest,
        completion: Sender<std::result::Result<(), String>>,
    },
    RunWait {
        request: RunWaitRequest,
        completion: Sender<std::result::Result<RunRegistration, String>>,
    },
    ListPaneLabels {
        attention_id: Option<u64>,
        completion: Sender<std::result::Result<Vec<String>, String>>,
    },
    ReconnectSession {
        runner_id: u64,
        session_id: u64,
        attention_id: Option<u64>,
        secret: Option<SessionSecret>,
        completion: Sender<ReconnectSessionResult>,
    },
    OpenRemoteSession {
        target: String,
        port: Option<u16>,
        session_id: u64,
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
    SetTheme {
        scope: crate::ThemeScope,
        theme: Option<String>,
        completion: Sender<bool>,
    },
    ListThemes {
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
    OpenNewWindow {
        profile: Option<String>,
        activation_token: Option<String>,
    },
    OpenProject {
        root: PathBuf,
        working_directory: Option<PathBuf>,
    },
    ReloadProjects,
    ReplacePane {
        split: Option<String>,
        profile: Option<String>,
        theme: Option<String>,
    },
    OpenCommand {
        request: PaneCommand,
        working_directory: Option<PathBuf>,
    },
    RunPane {
        request: PaneCommand,
    },
    RunShellCommand {
        request: ShellCommandRequest,
    },
    RunWait {
        request: RunWaitRequest,
    },
    RunComplete {
        id: u64,
        exit_code: Option<i32>,
    },
    ListPaneLabels {
        attention_id: Option<u64>,
    },
    ReconnectSession {
        runner_id: u64,
        session_id: u64,
        attention_id: Option<u64>,
        secret: Option<SessionSecret>,
    },
    OpenRemoteSession {
        target: String,
        port: Option<u16>,
        session_id: u64,
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
    SetTheme {
        scope: crate::ThemeScope,
        theme: Option<String>,
    },
    ListThemes,
    GetPaneTheme {
        attention_id: u64,
        pane_id: Option<u64>,
        /// The revision the client already has. When it matches, the answer is
        /// "unchanged" and the main thread is never involved.
        known_revision: Option<u64>,
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

/// Every field but `token` and `command` is optional, and a request sets only
/// the ones its command actually carries. Construct one with
/// `..Default::default()` rather than spelling out the rest as `None`:
/// `decode_control_request` is what enforces which fields a command may carry,
/// and a sender that lists all of them just makes adding a field a 20-site
/// edit.
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlRequest {
    token: String,
    command: String,
    runner_id: Option<u64>,
    session_id: Option<u64>,
    secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ssh_target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ssh_port: Option<u16>,
    icon: Option<String>,
    pane_theme: Option<String>,
    /// The pane-theme revision the client already knows, so an unchanged theme
    /// can be answered without involving the main thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pane_theme_revision: Option<u64>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    working_directory: Option<String>,
    split: Option<String>,
    profile: Option<String>,
    theme: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    pane_request: Option<PaneControlRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shell_command: Option<ShellCommandControlRequest>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RunWaitPayload {
    dependencies: Vec<String>,
    allow_failure: bool,
    command: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShellCommandControlRequest {
    command: String,
    arguments: Vec<String>,
    environment: BTreeMap<String, String>,
}

impl From<&ShellCommandRequest> for ShellCommandControlRequest {
    fn from(request: &ShellCommandRequest) -> Self {
        Self {
            command: request.command.clone(),
            arguments: request.arguments.clone(),
            environment: request.environment.clone(),
        }
    }
}

impl ShellCommandControlRequest {
    fn into_request(self) -> Option<ShellCommandRequest> {
        let mut environment_names = HashSet::new();
        if validate_command_string(&self.command).is_err()
            || self
                .arguments
                .iter()
                .any(|argument| argument.contains('\0'))
            || self.environment.iter().any(|(name, value)| {
                validate_command_environment_entry(name, value, "shell command environment")
                    .is_err()
                    || !environment_names.insert(name.to_ascii_uppercase())
            })
        {
            return None;
        }
        let payload_bytes = self.command.len()
            + self.arguments.iter().map(String::len).sum::<usize>()
            + self
                .environment
                .iter()
                .map(|(name, value)| name.len() + value.len())
                .sum::<usize>();
        (payload_bytes <= MAX_SHELL_COMMAND_BYTES).then_some(ShellCommandRequest {
            command: self.command,
            arguments: self.arguments,
            environment: self.environment,
        })
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run_id: Option<u64>,
    #[serde(default)]
    themes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pane_theme: Option<String>,
    /// The revision the returned pane theme was resolved at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pane_theme_revision: Option<u64>,
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

#[cfg(test)]
#[path = "tests/process_control.rs"]
mod tests;
