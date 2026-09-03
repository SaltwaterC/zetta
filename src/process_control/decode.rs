//! Decoding a [`ControlRequest`] off the wire into the [`ControlRequestCommand`]
//! the server acts on.
//!
//! [`ControlRequest`] is one wide struct for every command, so which fields a
//! command may carry is enforced here rather than by the type: a request that
//! names fields its command does not use is rejected outright. Every rejection
//! path zeroizes the request's secrets before returning.

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

fn control_request_has_new_window_payload(request: &ControlRequest) -> bool {
    request.runner_id.is_none()
        && request.session_id.is_none()
        && request.secret.is_none()
        && request.icon.is_none()
        && request.pane_theme.is_none()
        && request.pane_id.is_none()
        && request.pane_overlay.is_none()
        && request.pane_overlay_font_size.is_none()
        && request.pane_overlay_opacity.is_none()
        && request.pane_overlay_color.is_none()
        && request.attention_id.is_none()
        && request.attention_summary.is_none()
        && request.attention_body.is_none()
        && request.tab_name.is_none()
        && request.worktree_name.is_none()
        && request.working_directory.is_none()
        && request.split.is_none()
        && request
            .profile
            .as_deref()
            .is_none_or(|profile| !profile.is_empty())
        && request.theme.is_none()
        && request.scope.is_none()
        && request.pane_request.is_none()
        && request.ssh_target.is_none()
        && request.ssh_port.is_none()
        && request
            .config_path
            .as_deref()
            .is_none_or(|token| token.len() <= MAX_ACTIVATION_TOKEN_BYTES)
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

fn control_request_has_remote_session_payload(request: &ControlRequest) -> bool {
    request.runner_id.is_none()
        && request.session_id.is_some_and(|session_id| session_id != 0)
        && request.ssh_target.as_deref().is_some_and(|target| {
            !target.is_empty() && !target.starts_with('-') && target.len() <= 4096
        })
        && request.ssh_port.is_none_or(|port| port != 0)
        && request.icon.is_none()
        && request.pane_theme.is_none()
        && request.pane_id.is_none()
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
        && request.working_directory.is_none()
        && request.split.is_none()
        && request.profile.is_none()
        && request.theme.is_none()
        && request.scope.is_none()
        && request.pane_request.is_none()
        && request
            .secret
            .as_deref()
            .is_none_or(|secret| !secret.is_empty())
}

fn decode_control_request(
    request: &mut ControlRequest,
    token: &str,
) -> Option<ControlRequestCommand> {
    if !token_matches(&request.token, token) {
        zeroize_control_request_secrets(request);
        return None;
    }
    if !matches!(
        request.command.as_str(),
        "run_pane" | "open_command" | "list_panes"
    ) && request.pane_request.is_some()
    {
        zeroize_control_request_secrets(request);
        return None;
    }
    if request.command != "run_shell_command" && request.shell_command.is_some() {
        zeroize_control_request_secrets(request);
        return None;
    }
    if request.command != "open_remote_session"
        && (request.ssh_target.is_some() || request.ssh_port.is_some())
    {
        zeroize_control_request_secrets(request);
        return None;
    }
    if !matches!(
        request.command.as_str(),
        "reload_configuration"
            | "open_project"
            | "resume_disk_session"
            | "open_command"
            | "run_wait"
            | "run_complete"
            | "new_window"
    ) && request.config_path.is_some()
    {
        zeroize_control_request_secrets(request);
        return None;
    }
    if request.command != "open_project" && request.working_directory.is_some() {
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
    if request.command != "set_theme" && request.scope.is_some() {
        zeroize_control_request_secrets(request);
        return None;
    }
    if !matches!(request.command.as_str(), "get_pane_theme" | "run_wait")
        && request.pane_id.is_some()
    {
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
            | "list_panes"
            | "reconnect_session"
            | "run_wait"
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
        "new_window" if control_request_has_new_window_payload(request) => {
            Some(ControlRequestCommand::OpenNewWindow {
                profile: request.profile.take(),
                activation_token: request.config_path.take().filter(|token| !token.is_empty()),
            })
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
                })
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
        "run_wait"
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
                && request.working_directory.is_none()
                && request.split.is_none()
                && request.profile.is_none()
                && request.theme.is_none()
                && request.scope.is_none()
                && request.pane_request.is_none()
                && request.attention_id.is_some()
                && request.pane_id.is_some() =>
        {
            let owner = RunPaneIdentity::new(request.attention_id.take()?, request.pane_id.take()?);
            if owner.attention_id == 0 || owner.routing_id == 0 {
                zeroize_control_request_secrets(request);
                return None;
            }
            let mut encoded_payload = request.config_path.take()?;
            let payload = serde_json::from_str::<RunWaitPayload>(&encoded_payload);
            encoded_payload.zeroize();
            let Ok(payload) = payload else {
                zeroize_control_request_secrets(request);
                return None;
            };
            if payload.dependencies.is_empty()
                || payload.dependencies.iter().any(String::is_empty)
                || payload.command.is_empty()
                || payload.command.first().is_some_and(String::is_empty)
                || pane_command_byte_len(&payload.command) > MAX_PANE_COMMAND_BYTES
            {
                zeroize_control_request_secrets(request);
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
        "run_complete"
            if request.runner_id.is_none()
                && request.session_id.is_some()
                && request.secret.is_none()
                && request.icon.is_none()
                && request.pane_theme.is_none()
                && request.pane_id.is_none()
                && request.pane_overlay.is_none()
                && request.pane_overlay_font_size.is_none()
                && request.pane_overlay_opacity.is_none()
                && request.pane_overlay_color.is_none()
                && request.attention_id.is_none()
                && request.attention_summary.is_none()
                && request.attention_body.is_none()
                && request.tab_name.is_none()
                && request.worktree_name.is_none()
                && request.working_directory.is_none()
                && request.split.is_none()
                && request.profile.is_none()
                && request.theme.is_none()
                && request.scope.is_none()
                && request.pane_request.is_none() =>
        {
            let id = request.session_id.take()?;
            if id == 0 {
                zeroize_control_request_secrets(request);
                return None;
            }
            let mut encoded_exit_code = request.config_path.take()?;
            let exit_code = serde_json::from_str::<Option<i32>>(&encoded_exit_code).ok();
            encoded_exit_code.zeroize();
            let exit_code = exit_code?;
            Some(ControlRequestCommand::RunComplete { id, exit_code })
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
        "run_shell_command"
            if request.runner_id.is_none()
                && request.session_id.is_none()
                && request.secret.is_none()
                && request.icon.is_none()
                && request.pane_theme.is_none()
                && request.pane_id.is_none()
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
                && request.working_directory.is_none()
                && request.split.is_none()
                && request.profile.is_none()
                && request.theme.is_none()
                && request.scope.is_none()
                && request.pane_request.is_none()
                && request.shell_command.is_some() =>
        {
            request
                .shell_command
                .take()
                .and_then(ShellCommandControlRequest::into_request)
                .map(|request| ControlRequestCommand::RunShellCommand { request })
        }
        "open_command"
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
                && request.scope.is_none() =>
        {
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
                && request
                    .attention_id
                    .is_none_or(|attention_id| attention_id != 0)
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
            Some(ControlRequestCommand::ListPaneLabels {
                attention_id: request.attention_id.take(),
            })
        }
        "open_remote_session" if control_request_has_remote_session_payload(request) => {
            Some(ControlRequestCommand::OpenRemoteSession {
                target: request.ssh_target.take()?,
                port: request.ssh_port.take(),
                session_id: request.session_id.take()?,
                secret: request.secret.take().map(SessionSecret::new),
            })
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
        "set_theme"
            if request.runner_id.is_none()
                && request.session_id.is_none()
                && request.secret.is_none() =>
        {
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
        "list_themes"
            if request.runner_id.is_none()
                && request.session_id.is_none()
                && request.secret.is_none()
                && request.icon.is_none()
                && request.pane_theme.is_none()
                && request.theme.is_none() =>
        {
            Some(ControlRequestCommand::ListThemes)
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

#[cfg(test)]
#[path = "../tests/process_control/decode.rs"]
mod tests;
