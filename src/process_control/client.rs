//! The client half: one function per `zetta` subcommand that has to reach a
//! running window, over the endpoints `endpoint.rs` discovers.
//!
//! Every sender goes through [`send_control_request`] or
//! [`send_control_command`], which own the connection, the timeouts and the
//! endpoint's token, so a request cannot be built against one endpoint and sent
//! to another. `request_process_run_wait` is the exception, and says why.

use super::*;

use super::endpoint::{
    config_path_identity, live_control_endpoint, live_control_endpoints, read_control_endpoint,
};

pub(crate) fn request_existing_process_window() -> Result<bool> {
    request_existing_process_window_with_command("open_window", None, None)
}

pub(crate) fn request_existing_process_new_window(
    profile: Option<&str>,
    activation_token: Option<&str>,
) -> Result<bool> {
    request_existing_process_window_with_command("new_window", profile, activation_token)
}

fn request_existing_process_window_with_command(
    command: &str,
    profile: Option<&str>,
    activation_token: Option<&str>,
) -> Result<bool> {
    for endpoint in live_control_endpoints()? {
        if send_open_window_request_with_command(&endpoint, command, profile, activation_token)
            .unwrap_or(false)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn request_existing_process_project_with_working_directory(
    root: &Path,
    working_directory: Option<&Path>,
) -> Result<bool> {
    for endpoint in live_control_endpoints()? {
        if send_open_project_request_with_working_directory(&endpoint, root, working_directory)
            .unwrap_or(false)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn request_existing_process_projects_reload() -> Result<bool> {
    let mut accepted = false;
    for endpoint in live_control_endpoints()? {
        accepted |= send_reload_projects_request(&endpoint).unwrap_or(false);
    }
    Ok(accepted)
}

pub(crate) fn request_existing_process_replace_pane(request: ReplacePaneRequest) -> Result<bool> {
    for endpoint in live_control_endpoints()? {
        if send_replace_pane_request(&endpoint, &request).unwrap_or(false) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn request_existing_process_pane(request: PaneCommand) -> Result<bool> {
    let mut last_error = None;
    for endpoint in live_control_endpoints()? {
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

pub(crate) fn request_existing_process_shell_command(request: ShellCommandRequest) -> Result<bool> {
    // A command invoked by a pane carries its owning Zetta process ID. Route
    // through that endpoint first so another open Zetta process cannot accept
    // the request merely because its endpoint was enumerated first.
    if let Ok(process_id) = env::var("ZETTA_PROCESS_ID") {
        let process_id = process_id
            .parse::<u32>()
            .context("ZETTA_PROCESS_ID must be a positive process ID")?;
        anyhow::ensure!(
            process_id != 0,
            "ZETTA_PROCESS_ID must be a positive process ID"
        );
        return request_process_shell_command(process_id, &request);
    }

    let mut last_error = None;
    for endpoint in live_control_endpoints()? {
        match send_run_shell_command_request(&endpoint, &request) {
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

fn request_process_shell_command(process_id: u32, request: &ShellCommandRequest) -> Result<bool> {
    let Some(endpoint) = live_control_endpoint(process_id)? else {
        return Ok(false);
    };
    send_run_shell_command_request(&endpoint, request)
}

pub(crate) fn request_existing_process_command(
    request: PaneCommand,
    working_directory: Option<PathBuf>,
) -> Result<bool> {
    let mut last_error = None;
    for endpoint in live_control_endpoints()? {
        match send_open_command_request(&endpoint, &request, working_directory.as_deref()) {
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
    // A completion request originates in a particular pane. Prefer its
    // process identity so another Zetta window cannot supply unrelated labels
    // when several processes are running on the same machine.
    let attention_id = env::var("ZETTA_ATTENTION_ID")
        .ok()
        .map(|attention_id| {
            attention_id
                .parse::<u64>()
                .context("ZETTA_ATTENTION_ID must be a positive attention ID")
                .and_then(|attention_id| {
                    anyhow::ensure!(
                        attention_id != 0,
                        "ZETTA_ATTENTION_ID must be a positive attention ID"
                    );
                    Ok(attention_id)
                })
        })
        .transpose()?;
    if let Ok(process_id) = env::var("ZETTA_PROCESS_ID") {
        let process_id = process_id
            .parse::<u32>()
            .context("ZETTA_PROCESS_ID must be a positive process ID")?;
        anyhow::ensure!(
            process_id != 0,
            "ZETTA_PROCESS_ID must be a positive process ID"
        );
        return request_process_pane_labels(process_id, attention_id);
    }

    let mut last_error = None;
    for endpoint in live_control_endpoints()? {
        match send_list_pane_labels_request(&endpoint, attention_id) {
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

fn request_process_pane_labels(
    process_id: u32,
    attention_id: Option<u64>,
) -> Result<Option<Vec<String>>> {
    let Some(endpoint) = live_control_endpoint(process_id)? else {
        return Ok(None);
    };
    send_list_pane_labels_request(&endpoint, attention_id)
}

pub(crate) fn request_existing_process_tab_icon(icon: Option<IconName>) -> Result<bool> {
    for endpoint in live_control_endpoints()? {
        if send_set_tab_icon_request(&endpoint, icon).unwrap_or(false) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn request_existing_process_theme(
    scope: crate::ThemeScope,
    theme: Option<String>,
) -> Result<bool> {
    for endpoint in live_control_endpoints()? {
        if send_set_theme_request(&endpoint, scope, theme.clone()).unwrap_or(false) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn request_existing_process_theme_list() -> Result<Option<Vec<String>>> {
    for endpoint in live_control_endpoints()? {
        if let Some(themes) = send_list_themes_request(&endpoint).unwrap_or(None) {
            return Ok(Some(themes));
        }
    }
    Ok(None)
}

pub(crate) fn request_existing_process_pane_overlay(request: PaneOverlayRequest) -> Result<bool> {
    for endpoint in live_control_endpoints()? {
        if send_set_overlay_request(&endpoint, &request).unwrap_or(false) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn request_existing_process_configuration_reload(path: &Path) -> Result<bool> {
    let config_path = config_path_identity(path);
    for endpoint in live_control_endpoints()? {
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
    let endpoint = read_control_endpoint(process_id)?;
    send_set_tab_attention_request(&endpoint, &request)
}

pub(crate) struct RunWaitConnection {
    stream: UnixStream,
    id: u64,
    token: String,
    pub(crate) command: Vec<String>,
}

impl RunWaitConnection {
    pub(crate) fn complete(&mut self, exit_code: Option<i32>) -> Result<()> {
        let encoded_exit_code = serde_json::to_string(&exit_code)?;
        write_message(
            &mut self.stream,
            &ControlRequest {
                token: self.token.clone(),
                command: "run_complete".to_owned(),
                session_id: Some(self.id),
                config_path: Some(encoded_exit_code),
                ..Default::default()
            },
        )?;
        let response = read_message::<ControlResponse>(&mut self.stream)?;
        anyhow::ensure!(
            response.status == "ok",
            "the Zetta process did not acknowledge the run result"
        );
        Ok(())
    }
}

pub(crate) fn request_process_run_wait(
    process_id: u32,
    request: RunWaitRequest,
) -> Result<RunWaitConnection> {
    anyhow::ensure!(process_id != 0, "process ID must be positive");
    let endpoint = read_control_endpoint(process_id)?;
    let command = request.command.clone();
    let payload = serde_json::to_string(&RunWaitPayload {
        dependencies: request.dependencies,
        allow_failure: request.allow_failure,
        command: request.command,
    })?;
    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(None)?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: "run_wait".to_owned(),
            pane_id: Some(request.owner.routing_id),
            attention_id: Some(request.owner.attention_id),
            config_path: Some(payload),
            ..Default::default()
        },
    )?;
    let response = read_message::<ControlResponse>(&mut stream)?;
    match response.status.as_str() {
        "ready" => Ok(RunWaitConnection {
            stream,
            id: response
                .run_id
                .context("the Zetta process returned a run without an ID")?,
            token: endpoint.token,
            command,
        }),
        "failed" | "rejected" => {
            let message = response.error.map_or_else(
                || "run dependencies were not satisfied".to_owned(),
                |error| error.message,
            );
            anyhow::bail!("{message}");
        }
        status => anyhow::bail!("unexpected response to run_wait: {status}"),
    }
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
        let endpoint = read_control_endpoint(process_id)?;
        Ok(Self {
            endpoint,
            attention_id,
            pane_id,
        })
    }

    /// The pane's theme, if it has changed since `known_revision`.
    ///
    /// Pass `None` to force a full answer regardless of the revision; the
    /// watcher does that periodically so a revision that was never bumped
    /// cannot leave an editor showing stale colours indefinitely.
    pub(crate) fn theme_name(&self, known_revision: Option<u64>) -> Result<PaneThemeAnswer> {
        send_get_pane_theme_request(
            &self.endpoint,
            self.attention_id,
            self.pane_id,
            known_revision,
        )
    }
}

#[cfg(feature = "notifications")]
pub(crate) fn request_process_silent_mode(
    process_id: u32,
    attention_id: Option<u64>,
) -> Result<bool> {
    anyhow::ensure!(process_id != 0, "process ID must be positive");
    anyhow::ensure!(attention_id != Some(0), "attention ID must be positive");
    let endpoint = read_control_endpoint(process_id)?;
    send_get_silent_mode_request(&endpoint, attention_id)
}

#[cfg(feature = "notifications")]
pub(crate) fn request_process_focus_tab(process_id: u32, attention_id: u64) -> Result<bool> {
    anyhow::ensure!(process_id != 0, "process ID must be positive");
    anyhow::ensure!(attention_id != 0, "attention ID must be positive");
    let endpoint = read_control_endpoint(process_id)?;
    send_focus_tab_request(&endpoint, attention_id)
}

// Only the sidecar reaches this: no Zetta subcommand sets a tab name over the
// socket today. `decode_control_request` is what keeps `set_tab_name` available
// to other process-control clients, and this is what pins the client half of it.
#[cfg(test)]
pub(crate) fn request_process_tab_name(process_id: u32, request: TabNameRequest) -> Result<bool> {
    let endpoint = read_control_endpoint(process_id)?;
    send_set_tab_name_request(&endpoint, &request)
}

#[cfg(test)]
pub(super) fn send_open_window_request(endpoint: &ControlEndpoint) -> Result<bool> {
    send_open_window_request_with_command(endpoint, "open_window", None, None)
}

#[cfg(test)]
fn send_open_new_window_request(endpoint: &ControlEndpoint) -> Result<bool> {
    send_open_window_request_with_command(endpoint, "new_window", None, None)
}

#[cfg(test)]
fn send_open_new_window_request_with_profile_and_token(
    endpoint: &ControlEndpoint,
    profile: &str,
    activation_token: &str,
) -> Result<bool> {
    send_open_window_request_with_command(
        endpoint,
        "new_window",
        Some(profile),
        Some(activation_token),
    )
}

fn send_open_window_request_with_command(
    endpoint: &ControlEndpoint,
    command: &str,
    profile: Option<&str>,
    activation_token: Option<&str>,
) -> Result<bool> {
    send_control_command(
        endpoint,
        command,
        ControlRequest {
            config_path: activation_token.map(str::to_owned),
            profile: profile.map(str::to_owned),
            ..Default::default()
        },
    )
}

fn send_open_project_request_with_working_directory(
    endpoint: &ControlEndpoint,
    root: &Path,
    working_directory: Option<&Path>,
) -> Result<bool> {
    send_control_command(
        endpoint,
        "open_project",
        ControlRequest {
            config_path: Some(root.to_string_lossy().into_owned()),
            working_directory: working_directory.map(|path| path.to_string_lossy().into_owned()),
            ..Default::default()
        },
    )
}

fn send_reload_projects_request(endpoint: &ControlEndpoint) -> Result<bool> {
    send_control_command(endpoint, "reload_projects", ControlRequest::default())
}

#[cfg(feature = "notifications")]
fn send_get_silent_mode_request(
    endpoint: &ControlEndpoint,
    attention_id: Option<u64>,
) -> Result<bool> {
    let response = send_control_request(
        endpoint,
        "get_silent_mode",
        ControlRequest {
            attention_id,
            ..Default::default()
        },
    )?;
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
    send_control_command(
        endpoint,
        "set_tab_attention",
        ControlRequest {
            attention_id: Some(request.attention_id),
            attention_summary: Some(request.summary.clone()),
            attention_body: request.body.clone(),
            ..Default::default()
        },
    )
}

#[cfg(test)]
fn send_set_tab_name_request(endpoint: &ControlEndpoint, request: &TabNameRequest) -> Result<bool> {
    send_control_command(
        endpoint,
        "set_tab_name",
        ControlRequest {
            attention_id: Some(request.attention_id),
            tab_name: request.name.clone(),
            ..Default::default()
        },
    )
}

#[cfg(test)]
fn send_set_worktree_name_request(
    endpoint: &ControlEndpoint,
    request: &WorktreeNameRequest,
) -> Result<bool> {
    send_control_command(
        endpoint,
        "set_worktree_name",
        ControlRequest {
            attention_id: Some(request.attention_id),
            worktree_name: request.name.clone(),
            ..Default::default()
        },
    )
}

#[cfg(feature = "notifications")]
fn send_focus_tab_request(endpoint: &ControlEndpoint, attention_id: u64) -> Result<bool> {
    send_control_command(
        endpoint,
        "focus_tab",
        ControlRequest {
            attention_id: Some(attention_id),
            ..Default::default()
        },
    )
}

fn send_run_pane_request(endpoint: &ControlEndpoint, request: &PaneCommand) -> Result<bool> {
    let response = send_control_request(
        endpoint,
        "run_pane",
        ControlRequest {
            pane_request: Some(request.into()),
            ..Default::default()
        },
    )?;
    if response.status == "ok" {
        return Ok(true);
    }
    if let Some(error) = response.error {
        anyhow::bail!("{}: {}", error.code, error.message);
    }
    Ok(false)
}

fn send_run_shell_command_request(
    endpoint: &ControlEndpoint,
    request: &ShellCommandRequest,
) -> Result<bool> {
    let response = send_control_request(
        endpoint,
        "run_shell_command",
        ControlRequest {
            shell_command: Some(request.into()),
            ..Default::default()
        },
    )?;
    if response.status == "ok" {
        return Ok(true);
    }
    if let Some(error) = response.error {
        anyhow::bail!("{}: {}", error.code, error.message);
    }
    Ok(false)
}

fn send_open_command_request(
    endpoint: &ControlEndpoint,
    request: &PaneCommand,
    working_directory: Option<&Path>,
) -> Result<bool> {
    let response = send_control_request(
        endpoint,
        "open_command",
        ControlRequest {
            config_path: working_directory.map(|path| path.to_string_lossy().into_owned()),
            pane_request: Some(request.into()),
            ..Default::default()
        },
    )?;
    if response.status == "ok" {
        return Ok(true);
    }
    if let Some(error) = response.error {
        anyhow::bail!("{}: {}", error.code, error.message);
    }
    Ok(false)
}

fn send_list_pane_labels_request(
    endpoint: &ControlEndpoint,
    attention_id: Option<u64>,
) -> Result<Option<Vec<String>>> {
    let response = send_control_request(
        endpoint,
        "list_panes",
        ControlRequest {
            attention_id,
            ..Default::default()
        },
    )?;
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
    send_control_command(
        endpoint,
        "replace_pane",
        ControlRequest {
            split: request.split.clone(),
            profile: request.profile.clone(),
            theme: request.theme.clone(),
            ..Default::default()
        },
    )
}

fn send_reload_configuration_request(
    endpoint: &ControlEndpoint,
    config_path: &str,
) -> Result<bool> {
    send_control_command(
        endpoint,
        "reload_configuration",
        ControlRequest {
            config_path: Some(config_path.to_owned()),
            ..Default::default()
        },
    )
}

fn send_set_tab_icon_request(endpoint: &ControlEndpoint, icon: Option<IconName>) -> Result<bool> {
    send_control_command(
        endpoint,
        "set_tab_icon",
        ControlRequest {
            icon: icon.map(|icon| {
                let name: &'static str = icon.into();
                name.to_owned()
            }),
            ..Default::default()
        },
    )
}

fn send_set_theme_request(
    endpoint: &ControlEndpoint,
    scope: crate::ThemeScope,
    theme: Option<String>,
) -> Result<bool> {
    send_control_command(
        endpoint,
        "set_theme",
        ControlRequest {
            theme,
            scope: Some(scope.name().to_owned()),
            ..Default::default()
        },
    )
}

fn send_list_themes_request(endpoint: &ControlEndpoint) -> Result<Option<Vec<String>>> {
    let response = send_control_request(endpoint, "list_themes", ControlRequest::default())?;
    Ok((response.status == "ok").then_some(response.themes))
}

/// What a pane-theme query answered.
#[cfg(feature = "syntax-highlighting")]
pub(crate) enum PaneThemeAnswer {
    /// The revision the client already had is still current, so the theme it is
    /// using is still right. Answered by the connection thread alone.
    Unchanged,
    /// The theme as of `revision`. `name` is `None` when the pane has no theme
    /// of its own.
    Resolved {
        name: Option<String>,
        revision: Option<u64>,
    },
}

#[cfg(feature = "syntax-highlighting")]
fn send_get_pane_theme_request(
    endpoint: &ControlEndpoint,
    attention_id: u64,
    pane_id: Option<u64>,
    known_revision: Option<u64>,
) -> Result<PaneThemeAnswer> {
    let response = send_control_request(
        endpoint,
        "get_pane_theme",
        ControlRequest {
            pane_id,
            attention_id: Some(attention_id),
            pane_theme_revision: known_revision,
            ..Default::default()
        },
    )?;
    if response.status == "unchanged" {
        return Ok(PaneThemeAnswer::Unchanged);
    }
    Ok(PaneThemeAnswer::Resolved {
        name: (response.status == "ok")
            .then_some(response.pane_theme)
            .flatten(),
        revision: response.pane_theme_revision,
    })
}

fn send_set_overlay_request(
    endpoint: &ControlEndpoint,
    request: &PaneOverlayRequest,
) -> Result<bool> {
    send_control_command(
        endpoint,
        "set_overlay",
        ControlRequest {
            pane_overlay: request.text.clone(),
            pane_overlay_font_size: request
                .font_size
                .map(OverlayFontSize::cli_name)
                .map(str::to_owned),
            pane_overlay_opacity: request.opacity,
            pane_overlay_color: request.color.clone(),
            ..Default::default()
        },
    )
}

/// Reads one newline-framed message. A reconnect request carries the session
/// secret in this buffer, so it is zeroized on every exit path rather than left
/// in freed heap memory.
/// Sends one request on a fresh connection and reads the response.
///
/// The endpoint owns both the token and the connection, so callers pass only
/// the payload fields and the command name. Wiring the token in at each call
/// site let a request be built against one endpoint's token and sent to
/// another, and cost every sender three lines of timeout setup that must not
/// drift: `CONTROL_CLIENT_TIMEOUT` is what stops a wedged window from hanging
/// a CLI invocation.
///
/// `request_process_run_wait` connects for itself: it keeps the stream after
/// the first response and reads with no timeout, because a run wrapper blocks
/// for as long as its dependencies take.
fn send_control_request(
    endpoint: &ControlEndpoint,
    command: &str,
    request: ControlRequest,
) -> Result<ControlResponse> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: command.to_owned(),
            ..request
        },
    )?;
    read_message::<ControlResponse>(&mut stream)
}

/// [`send_control_request`] for the senders that only care whether the target
/// process accepted the command.
fn send_control_command(
    endpoint: &ControlEndpoint,
    command: &str,
    request: ControlRequest,
) -> Result<bool> {
    Ok(send_control_request(endpoint, command, request)?.status == "ok")
}

#[cfg(test)]
#[path = "../tests/process_control/client.rs"]
mod tests;
