//! Decoding a [`ControlRequest`] off the wire into the [`ControlRequestCommand`]
//! the server acts on.
//!
//! [`ControlRequest`] is one wide struct shared by every command, so which
//! fields a command may carry cannot be expressed by the type. It is expressed
//! by [`allowed_control_fields`] instead: one table, checked once, rather than a
//! chain of `is_none()` tests repeated per command. A field added to
//! [`ControlRequest`] is rejected for every command until it is named there,
//! which is the safe default.
//!
//! What the table cannot say is checked in the command's own arm: that a value
//! is present, non-empty, non-zero, or consistent with another field.
//!
//! Every rejection zeroizes the request's secrets. [`decode_control_request`] is
//! the only entry point and does that in one place, so everything below it just
//! returns `None`.

use super::*;

pub(super) fn handle_control_request(
    stream: &mut UnixStream,
    token: &str,
) -> Option<ControlRequestCommand> {
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

/// One bit per optional [`ControlRequest`] field.
///
/// The two mandatory fields, `token` and `command`, have no bit: every request
/// carries them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ControlFields(u32);

impl ControlFields {
    /// Whether every field this set names is also named by `allowed`.
    fn is_subset_of(self, allowed: Self) -> bool {
        self.0 & !allowed.0 == 0
    }
}

mod field {
    pub(super) const RUNNER_ID: u32 = 1 << 0;
    pub(super) const SESSION_ID: u32 = 1 << 1;
    pub(super) const SECRET: u32 = 1 << 2;
    pub(super) const SSH_TARGET: u32 = 1 << 3;
    pub(super) const SSH_PORT: u32 = 1 << 4;
    pub(super) const ICON: u32 = 1 << 5;
    pub(super) const PANE_THEME: u32 = 1 << 6;
    pub(super) const PANE_ID: u32 = 1 << 7;
    pub(super) const PANE_OVERLAY: u32 = 1 << 8;
    pub(super) const PANE_OVERLAY_FONT_SIZE: u32 = 1 << 9;
    pub(super) const PANE_OVERLAY_OPACITY: u32 = 1 << 10;
    pub(super) const PANE_OVERLAY_COLOR: u32 = 1 << 11;
    pub(super) const ATTENTION_ID: u32 = 1 << 12;
    pub(super) const ATTENTION_SUMMARY: u32 = 1 << 13;
    pub(super) const ATTENTION_BODY: u32 = 1 << 14;
    pub(super) const TAB_NAME: u32 = 1 << 15;
    pub(super) const WORKTREE_NAME: u32 = 1 << 16;
    pub(super) const CONFIG_PATH: u32 = 1 << 17;
    pub(super) const WORKING_DIRECTORY: u32 = 1 << 18;
    pub(super) const SPLIT: u32 = 1 << 19;
    pub(super) const PROFILE: u32 = 1 << 20;
    pub(super) const THEME: u32 = 1 << 21;
    pub(super) const SCOPE: u32 = 1 << 22;
    pub(super) const PANE_REQUEST: u32 = 1 << 23;
    pub(super) const SHELL_COMMAND: u32 = 1 << 24;

    /// The pane-styling fields that the oldest commands accept and never read.
    ///
    /// Their guards only ever asserted that `runner_id`, `session_id` and
    /// `secret` were absent, so everything else was tolerated. Keeping that
    /// exactly is what makes this table a refactor rather than a hardening;
    /// narrowing one of these sets rejects requests that are accepted today and
    /// belongs in its own change.
    pub(super) const UNREAD_STYLE: u32 = ICON
        | PANE_THEME
        | PANE_OVERLAY
        | PANE_OVERLAY_FONT_SIZE
        | PANE_OVERLAY_OPACITY
        | PANE_OVERLAY_COLOR
        | SPLIT
        | PROFILE
        | THEME;
}

const fn bit(present: bool, field: u32) -> u32 {
    if present { field } else { 0 }
}

/// The fields a request actually sets.
fn control_request_fields(request: &ControlRequest) -> ControlFields {
    ControlFields(
        bit(request.runner_id.is_some(), field::RUNNER_ID)
            | bit(request.session_id.is_some(), field::SESSION_ID)
            | bit(request.secret.is_some(), field::SECRET)
            | bit(request.ssh_target.is_some(), field::SSH_TARGET)
            | bit(request.ssh_port.is_some(), field::SSH_PORT)
            | bit(request.icon.is_some(), field::ICON)
            | bit(request.pane_theme.is_some(), field::PANE_THEME)
            | bit(request.pane_id.is_some(), field::PANE_ID)
            | bit(request.pane_overlay.is_some(), field::PANE_OVERLAY)
            | bit(
                request.pane_overlay_font_size.is_some(),
                field::PANE_OVERLAY_FONT_SIZE,
            )
            | bit(
                request.pane_overlay_opacity.is_some(),
                field::PANE_OVERLAY_OPACITY,
            )
            | bit(
                request.pane_overlay_color.is_some(),
                field::PANE_OVERLAY_COLOR,
            )
            | bit(request.attention_id.is_some(), field::ATTENTION_ID)
            | bit(
                request.attention_summary.is_some(),
                field::ATTENTION_SUMMARY,
            )
            | bit(request.attention_body.is_some(), field::ATTENTION_BODY)
            | bit(request.tab_name.is_some(), field::TAB_NAME)
            | bit(request.worktree_name.is_some(), field::WORKTREE_NAME)
            | bit(request.config_path.is_some(), field::CONFIG_PATH)
            | bit(
                request.working_directory.is_some(),
                field::WORKING_DIRECTORY,
            )
            | bit(request.split.is_some(), field::SPLIT)
            | bit(request.profile.is_some(), field::PROFILE)
            | bit(request.theme.is_some(), field::THEME)
            | bit(request.scope.is_some(), field::SCOPE)
            | bit(request.pane_request.is_some(), field::PANE_REQUEST)
            | bit(request.shell_command.is_some(), field::SHELL_COMMAND),
    )
}

/// The fields each command may carry, or `None` for a command this Zetta does
/// not serve.
///
/// This is the whole cross-field allow-list. Read it as "a `set_tab_name`
/// request may name an attention target and a name, and nothing else"; a
/// request that sets anything outside its command's row is rejected before the
/// command is decoded.
fn allowed_control_fields(command: &str) -> Option<ControlFields> {
    use field::*;

    Some(ControlFields(match command {
        "reload_configuration" => CONFIG_PATH | SPLIT | PROFILE | THEME,
        "open_window" => UNREAD_STYLE,
        "new_window" => PROFILE | CONFIG_PATH,
        "open_project" => CONFIG_PATH | WORKING_DIRECTORY,
        "reload_projects" => 0,
        "get_silent_mode" => ATTENTION_ID,
        "replace_pane" => SPLIT | PROFILE | THEME,
        "run_wait" => ATTENTION_ID | PANE_ID | CONFIG_PATH,
        "run_complete" => SESSION_ID | CONFIG_PATH,
        "run_pane" => PANE_REQUEST,
        "run_shell_command" => SHELL_COMMAND,
        "open_command" => CONFIG_PATH | PANE_REQUEST,
        "list_panes" => ATTENTION_ID,
        "open_remote_session" => SESSION_ID | SECRET | SSH_TARGET | SSH_PORT,
        "reconnect_session" => RUNNER_ID | SESSION_ID | SECRET | ATTENTION_ID,
        "resume_disk_session" => SESSION_ID | SECRET | CONFIG_PATH,
        "set_tab_icon" => UNREAD_STYLE,
        "set_theme" => UNREAD_STYLE | SCOPE,
        "list_themes" => {
            PANE_OVERLAY
                | PANE_OVERLAY_FONT_SIZE
                | PANE_OVERLAY_OPACITY
                | PANE_OVERLAY_COLOR
                | SPLIT
                | PROFILE
        }
        "get_pane_theme" => ATTENTION_ID | PANE_ID,
        "set_overlay" => UNREAD_STYLE,
        "set_tab_attention" => ATTENTION_ID | ATTENTION_SUMMARY | ATTENTION_BODY,
        "focus_tab" => ATTENTION_ID,
        "set_tab_name" => ATTENTION_ID | TAB_NAME,
        "set_worktree_name" => ATTENTION_ID | WORKTREE_NAME,
        _ => return None,
    }))
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
    let command = decode_authenticated_request(request, token);
    if command.is_none() {
        zeroize_control_request_secrets(request);
    }
    command
}

fn decode_authenticated_request(
    request: &mut ControlRequest,
    token: &str,
) -> Option<ControlRequestCommand> {
    if !token_matches(&request.token, token) {
        return None;
    }
    let allowed = allowed_control_fields(&request.command)?;
    if !control_request_fields(request).is_subset_of(allowed) {
        return None;
    }
    decode_control_command(request)
}

/// Turns a request whose fields are already known to be legal for its command
/// into that command, checking the values the table cannot describe.
fn decode_control_command(request: &mut ControlRequest) -> Option<ControlRequestCommand> {
    match request.command.as_str() {
        "reload_configuration" => request
            .config_path
            .take()
            .filter(|path| !path.is_empty())
            .map(|config_path| ControlRequestCommand::ReloadConfiguration { config_path }),
        "open_window" => Some(ControlRequestCommand::OpenWindow),
        "new_window" => {
            let profile = request.profile.take();
            if profile.as_deref().is_some_and(str::is_empty) {
                return None;
            }
            let activation_token = request.config_path.take();
            if activation_token
                .as_deref()
                .is_some_and(|token| token.len() > MAX_ACTIVATION_TOKEN_BYTES)
            {
                return None;
            }
            Some(ControlRequestCommand::OpenNewWindow {
                profile,
                activation_token: activation_token.filter(|token| !token.is_empty()),
            })
        }
        "open_project" => request
            .config_path
            .take()
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .map(|root| {
                let working_directory = request
                    .working_directory
                    .take()
                    .filter(|path| !path.is_empty())
                    .map(PathBuf::from);
                ControlRequestCommand::OpenProject {
                    root,
                    working_directory,
                }
            }),
        "reload_projects" => Some(ControlRequestCommand::ReloadProjects),
        "get_silent_mode" => {
            let attention_id = request.attention_id.take();
            if attention_id == Some(0) {
                return None;
            }
            Some(ControlRequestCommand::GetSilentMode { attention_id })
        }
        "replace_pane" => {
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
        "run_wait" => {
            let owner = RunPaneIdentity::new(request.attention_id.take()?, request.pane_id.take()?);
            if owner.attention_id == 0 || owner.routing_id == 0 {
                return None;
            }
            let mut encoded_payload = request.config_path.take()?;
            let payload = serde_json::from_str::<RunWaitPayload>(&encoded_payload);
            encoded_payload.zeroize();
            let payload = payload.ok()?;
            if payload.dependencies.is_empty()
                || payload.dependencies.iter().any(String::is_empty)
                || payload.command.is_empty()
                || payload.command.first().is_some_and(String::is_empty)
                || pane_command_byte_len(&payload.command) > MAX_PANE_COMMAND_BYTES
            {
                return None;
            }
            Some(ControlRequestCommand::RunWait {
                request: RunWaitRequest {
                    owner,
                    dependencies: payload.dependencies,
                    allow_failure: payload.allow_failure,
                    command: payload.command,
                },
            })
        }
        "run_complete" => {
            let id = request.session_id.take()?;
            if id == 0 {
                return None;
            }
            let mut encoded_exit_code = request.config_path.take()?;
            let exit_code = serde_json::from_str::<Option<i32>>(&encoded_exit_code).ok();
            encoded_exit_code.zeroize();
            Some(ControlRequestCommand::RunComplete {
                id,
                exit_code: exit_code?,
            })
        }
        "run_pane" => request
            .pane_request
            .take()
            .and_then(PaneControlRequest::into_command)
            .map(|request| ControlRequestCommand::RunPane { request }),
        "run_shell_command" => request
            .shell_command
            .take()
            .and_then(ShellCommandControlRequest::into_request)
            .map(|request| ControlRequestCommand::RunShellCommand { request }),
        "open_command" => {
            let working_directory = request
                .config_path
                .take()
                .filter(|path| !path.is_empty())
                .map(PathBuf::from);
            request
                .pane_request
                .take()
                .and_then(PaneControlRequest::into_command)
                .filter(|request| {
                    request.direction.is_none()
                        && request.label.is_none()
                        && request.pane.is_none()
                        && request.overlay.is_none()
                        && !request.stack
                        && !request.list
                })
                .map(|request| ControlRequestCommand::OpenCommand {
                    request,
                    working_directory,
                })
        }
        "list_panes" => {
            let attention_id = request.attention_id.take();
            if attention_id == Some(0) {
                return None;
            }
            Some(ControlRequestCommand::ListPaneLabels { attention_id })
        }
        "open_remote_session" => {
            let target = request.ssh_target.take().filter(|target| {
                !target.is_empty() && !target.starts_with('-') && target.len() <= 4096
            })?;
            let port = request.ssh_port.take();
            if port == Some(0) {
                return None;
            }
            let session_id = request.session_id.take().filter(|id| *id != 0)?;
            let secret = request.secret.take();
            if secret.as_deref().is_some_and(str::is_empty) {
                return None;
            }
            Some(ControlRequestCommand::OpenRemoteSession {
                target,
                port,
                session_id,
                secret: secret.map(SessionSecret::new),
            })
        }
        "reconnect_session" => {
            let attention_id = request.attention_id.take();
            if attention_id == Some(0) {
                return None;
            }
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
        "resume_disk_session" => {
            let session_id = request.session_id.take()?;
            // The standalone client sends a JSON object here. Keep the paths
            // and passphrases private to the authenticated local socket and
            // reject malformed or empty entries before they reach the GUI.
            let mut encoded_payload = request.config_path.take()?;
            let payload = serde_json::from_str::<ResumeIdentityPayload>(&encoded_payload);
            encoded_payload.zeroize();
            let payload = payload.ok()?;
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
                return None;
            }
            Some(ControlRequestCommand::ResumeDiskSession {
                session_id,
                identity_paths,
                identity_passphrases,
                secret: request.secret.take().map(SessionSecret::new),
            })
        }
        "set_tab_icon" => {
            let icon = match request.icon.take() {
                Some(icon) => Some(icon.parse().ok()?),
                None => None,
            };
            Some(ControlRequestCommand::SetTabIcon { icon })
        }
        "set_theme" => {
            let scope = match request.scope.take()?.as_str() {
                "pane" => crate::ThemeScope::Pane,
                "tab" => crate::ThemeScope::Tab,
                _ => return None,
            };
            let theme = request.theme.take();
            if theme.as_deref().is_some_and(str::is_empty) {
                return None;
            }
            Some(ControlRequestCommand::SetTheme { scope, theme })
        }
        "list_themes" => Some(ControlRequestCommand::ListThemes),
        "get_pane_theme" => {
            let pane_id = request.pane_id.take();
            if pane_id == Some(0) {
                return None;
            }
            Some(ControlRequestCommand::GetPaneTheme {
                attention_id: request.attention_id.take().filter(|id| *id != 0)?,
                pane_id,
            })
        }
        "set_overlay" => {
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
        "set_tab_attention" => {
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
        "focus_tab" => Some(ControlRequestCommand::FocusTab {
            attention_id: request.attention_id.take().filter(|id| *id != 0)?,
        }),
        "set_tab_name" => {
            let attention_id = request.attention_id.take().filter(|id| *id != 0)?;
            let name = request.tab_name.take();
            if name.as_deref().is_some_and(str::is_empty) {
                return None;
            }
            Some(ControlRequestCommand::SetTabName { attention_id, name })
        }
        "set_worktree_name" => {
            let attention_id = request.attention_id.take().filter(|id| *id != 0)?;
            let name = request.worktree_name.take();
            if name.as_deref().is_some_and(str::is_empty) {
                return None;
            }
            Some(ControlRequestCommand::SetWorktreeName { attention_id, name })
        }
        // `allowed_control_fields` has already rejected anything else.
        _ => None,
    }
}

#[cfg(test)]
#[path = "../tests/process_control/decode.rs"]
mod tests;
