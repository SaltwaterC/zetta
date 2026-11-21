//! The remote-session picker.
//!
//! SSH discovery is deliberately kept out of the render path. The picker only
//! reads the local SSH config when it opens, and does the endpoint/list request
//! on GPUI's background executor after the user submits a target. The forwarded
//! mux connection is then handed to the ordinary shared-pane attach path.

use super::*;
use crate::background_session_ui::{RemoteAttachOutcome, load_remote_attach};
use crate::config::REMOTE_KEEP_ALIVE_DEFAULT_MS;
use crate::overlay_frame::{ALT_MODIFIER, PRIMARY_MODIFIER};
use crate::remote_pane_transport::{RemotePaneTransport, parse_keep_alive_interval};
use crate::session_auth_ui::SessionAuthenticationPromptMode;

mod picker;
pub(crate) use picker::*;
mod create;
#[cfg(test)]
use create::*;
mod keys;
mod render;
#[cfg(test)]
use render::*;

const REMOTE_SESSION_SUGGESTION_VIEWPORT_ROWS: usize = 6;
const REMOTE_SESSION_SUGGESTION_VIEWPORT_HEIGHT: gpui::Rems =
    gpui::rems(1.75 * REMOTE_SESSION_SUGGESTION_VIEWPORT_ROWS as f32);
const REMOTE_ERROR_MAX_CHARS: usize = 240;

/// Turn output from a remote login shell into a short message that can safely
/// live in the picker. Login-shell startup scripts often emit ANSI colour
/// sequences and several unrelated diagnostics before `zmux` gets to run;
/// showing that whole stream makes the picker grow past its useful controls.
fn remote_error_message(error: &anyhow::Error) -> String {
    let output = strip_remote_terminal_sequences(&format!("{error:#}"));
    let lower_output = output.to_ascii_lowercase();
    if (lower_output.contains("reading multiplexer endpoint")
        && lower_output.contains("no such file or directory"))
        || lower_output.contains("no multiplexer is running")
        || (lower_output.contains("connecting the ssh stdio proxy to the local daemon")
            && lower_output.contains("connection refused"))
    {
        return "No remote session service is running on this host. Create a remote session to start one."
            .to_owned();
    }
    if lower_output.contains("exit status: 127")
        || lower_output.contains("command not found: zmux")
        || lower_output.contains("zmux: command not found")
    {
        return "Remote zmux was not found. Install it on the remote host or add it to the remote login shell's PATH.".to_owned();
    }

    // The login shell can write prompt-plugin failures before the command's
    // own diagnostic. Prefer the latter when `zmux` identified it explicitly.
    if let Some(message) = output
        .lines()
        .map(str::trim)
        .rev()
        .find(|line| line.to_ascii_lowercase().starts_with("zmux:"))
    {
        return truncate_remote_error(message);
    }

    let mut message = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(3)
        .collect::<Vec<_>>()
        .join(" ");
    if message.is_empty() {
        message = "The remote operation failed.".to_owned();
    }
    truncate_remote_error(&message)
}

/// Remove terminal control sequences before a remote error reaches a regular
/// text element. The UI is not a terminal, so rendering CSI/OSC bytes as
/// printable text is both noisy and liable to produce surprising layout.
fn strip_remote_terminal_sequences(output: &str) -> String {
    let mut cleaned = String::with_capacity(output.len());
    let mut characters = output.chars();
    while let Some(character) = characters.next() {
        if character != '\x1b' {
            match character {
                '\n' => cleaned.push('\n'),
                '\t' => cleaned.push(' '),
                character if character.is_control() => {}
                character => cleaned.push(character),
            }
            continue;
        }

        match characters.next() {
            Some('[') => {
                for character in characters.by_ref() {
                    if ('@'..='~').contains(&character) {
                        break;
                    }
                }
            }
            Some(']') => {
                let mut terminated_by_escape = false;
                for character in characters.by_ref() {
                    if character == '\x07' {
                        break;
                    }
                    if terminated_by_escape {
                        if character == '\\' {
                            break;
                        }
                        terminated_by_escape = false;
                    } else if character == '\x1b' {
                        terminated_by_escape = true;
                    }
                }
            }
            Some(_) | None => {}
        }
    }
    cleaned
}

fn truncate_remote_error(message: &str) -> String {
    if message.chars().count() <= REMOTE_ERROR_MAX_CHARS {
        return message.to_owned();
    }
    let mut truncated = message
        .chars()
        .take(REMOTE_ERROR_MAX_CHARS.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

#[cfg(test)]
#[path = "tests/remote_session_ui.rs"]
mod tests;

impl Zetta {
    pub(crate) fn next_remote_session_operation_generation(&mut self) -> u64 {
        self.remote_session_operation_generation =
            self.remote_session_operation_generation.wrapping_add(1);
        self.remote_session_operation_generation
    }

    pub(crate) fn remote_session_operation_is_current(&self, generation: u64) -> bool {
        self.remote_session_operation_generation == generation
    }

    pub(crate) fn open_remote_session(
        &mut self,
        _: &OpenRemoteSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let operation_generation = self.next_remote_session_operation_generation();
        self.command_palette = None;
        self.multi_command = None;
        self.tab_search = None;
        self.settings_editor = None;
        #[cfg(feature = "serial-console")]
        {
            self.serial_console = None;
        }
        let remote = self.launch_config.sessions.remote.clone();
        let templates = remote_session_templates(&self.launch_config);
        let picker = RemoteSessionPicker {
            suggestions: crate::multi_command::ssh_config_host_suggestions(),
            generation: operation_generation,
            transport: RemotePaneTransport::from_config(&remote),
            templates,
            keep_alive: TextField::new(
                remote
                    .keep_alive_ms
                    .map(|interval| interval.to_string())
                    .unwrap_or_default(),
            ),
            forward_agent: remote.forward_agent,
            ..Default::default()
        };
        self.remote_session_picker = Some(picker);
        #[cfg(feature = "session-persistence")]
        {
            self.remote_session_key_envelope = None;
        }
        self.remote_session_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn dismiss_remote_session_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.next_remote_session_operation_generation();
        self.remote_session_picker = None;
        self.remote_session_target = None;
        #[cfg(feature = "session-persistence")]
        {
            self.remote_session_key_envelope = None;
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    fn remote_target_from_picker(
        picker: &RemoteSessionPicker,
    ) -> anyhow::Result<zmux::remote::RemoteTarget> {
        let destination = picker.target.text.trim();
        anyhow::ensure!(!destination.is_empty(), "enter an SSH target");
        let port =
            if picker.port.text.trim().is_empty() {
                None
            } else {
                let port =
                    picker.port.text.trim().parse::<u16>().map_err(|_| {
                        anyhow::anyhow!("SSH port must be a number from 1 to 65535")
                    })?;
                anyhow::ensure!(port != 0, "SSH port must be between 1 and 65535");
                Some(port)
            };
        let target = zmux::remote::RemoteTarget::new(destination)
            .with_port(port)
            .with_forward_agent(picker.forward_agent && !picker.transport.is_zosh());
        target.validate()?;
        Ok(target)
    }

    /// What the picker's protocol and keep-alive fields amount to.
    ///
    /// The interval is validated here rather than as it is typed: a partially
    /// typed number is not an error yet, and the picker has one error line.
    fn remote_transport_from_picker(
        picker: &RemoteSessionPicker,
    ) -> anyhow::Result<RemotePaneTransport> {
        if !picker.transport.is_zosh() {
            return Ok(RemotePaneTransport::Ssh);
        }
        let keep_alive = picker.keep_alive.text.trim();
        let keep_alive_ms = if keep_alive.is_empty() {
            None
        } else {
            Some(parse_keep_alive_interval(keep_alive)?)
        };
        Ok(RemotePaneTransport::Zosh {
            keep_alive_ms,
            forward_agent: picker.forward_agent,
        })
    }

    pub(crate) fn load_remote_sessions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = self.remote_session_picker.as_ref() else {
            return;
        };
        let target = match Self::remote_target_from_picker(picker) {
            Ok(target) => target,
            Err(error) => {
                if let Some(picker) = self.remote_session_picker.as_mut() {
                    picker.error = Some(remote_error_message(&error));
                }
                self.remote_session_focus.focus(window, cx);
                cx.notify();
                return;
            }
        };
        let operation_generation = self.next_remote_session_operation_generation();
        let picker = self
            .remote_session_picker
            .as_mut()
            .expect("the remote session picker was checked above");
        picker.generation = operation_generation;
        picker.task.take();
        picker.sessions.clear();
        picker.selected = 0;
        picker.loading = true;
        picker.attach_generation = None;
        picker.error = None;
        picker.profiles.clear();
        picker.selected_profile = 0;
        picker.profiles_loading = true;
        picker.profile_error = None;
        picker.move_unavailable_focus_to_cancel();
        let generation = picker.generation;
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    // Both ride the same shared SSH login, and neither needs
                    // the other's answer, so the second costs a session on
                    // that login rather than a wait for the first.
                    let profiles_target = target.clone();
                    std::thread::scope(|scope| {
                        let profiles = scope.spawn(move || {
                            zmux::remote::RemoteTransport::shared(profiles_target)
                                .and_then(|transport| transport.query_profiles())
                                .context("discovering remote profiles")
                        });
                        let sessions = zmux::client::Client::connect_remote(target)
                            .and_then(|client| client.list())
                            .context("listing remote sessions");
                        let profiles = profiles.join().unwrap_or_else(|_| {
                            Err(anyhow::anyhow!("discovering remote profiles panicked"))
                        });
                        RemoteSessionDiscovery { profiles, sessions }
                    })
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.apply_remote_discovery_result(generation, result, window, cx);
            })
            .ok();
        });
        picker.task = Some(task);
        cx.notify();
    }

    fn apply_remote_discovery_result(
        &mut self,
        generation: u64,
        result: RemoteSessionDiscovery,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(picker) = self.remote_session_picker.as_mut() else {
            return;
        };
        if picker.generation != generation {
            return;
        }
        picker.loading = false;
        picker.profiles_loading = false;
        picker.task = None;
        match result.profiles {
            Ok(profiles) => {
                let previous = picker
                    .profiles
                    .get(picker.selected_profile)
                    .cloned()
                    .unwrap_or_else(|| "System".to_owned());
                picker.profiles = sorted_names(profiles);
                picker.selected_profile = picker
                    .profiles
                    .iter()
                    .position(|profile| profile.eq_ignore_ascii_case(&previous))
                    .or_else(|| {
                        picker
                            .profiles
                            .iter()
                            .position(|profile| profile.eq_ignore_ascii_case("System"))
                    })
                    .unwrap_or(0);
                picker.profile_error = None;
            }
            Err(error) => picker.profile_error = Some(remote_error_message(&error)),
        }
        match result.sessions {
            Ok(sessions) => {
                picker.sessions = sessions;
                picker.selected = 0;
                if picker.sessions.is_empty() {
                    picker.error = Some("The remote host has no shared sessions.".into());
                    picker.field = RemoteSessionField::Target;
                } else {
                    // The sessions are what the user asked for, so the list is
                    // what Enter and the arrow keys should act on; leaving the
                    // picker on the target field made Enter open a second SSH
                    // connection instead of attaching.
                    picker.field = RemoteSessionField::List;
                    picker.reset_suggestion_navigation();
                    picker.scroll.scroll_to_item(0, ScrollStrategy::Top);
                }
            }
            Err(error) => picker.error = Some(remote_error_message(&error)),
        }
        picker.move_unavailable_focus_to_cancel();
        self.remote_session_focus.focus(window, cx);
        cx.notify();
    }

    #[cfg(test)]
    fn apply_remote_session_result(
        &mut self,
        generation: u64,
        result: anyhow::Result<Vec<zmux::protocol::BackgroundSessionSummary>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(picker) = self.remote_session_picker.as_mut() else {
            return;
        };
        if picker.generation != generation {
            return;
        }
        picker.loading = false;
        picker.task = None;
        match result {
            Ok(sessions) => {
                picker.sessions = sessions;
                picker.selected = 0;
                if picker.sessions.is_empty() {
                    picker.error = Some("The remote host has no shared sessions.".into());
                } else {
                    // The sessions are what the user asked for, so the list is
                    // what Enter and the arrow keys should act on; leaving the
                    // picker on the target field made Enter open a second SSH
                    // connection instead of attaching.
                    picker.field = RemoteSessionField::List;
                    picker.reset_suggestion_navigation();
                    picker.scroll.scroll_to_item(0, ScrollStrategy::Top);
                }
            }
            Err(error) => picker.error = Some(remote_error_message(&error)),
        }
        picker.move_unavailable_focus_to_cancel();
        self.remote_session_focus.focus(window, cx);
        cx.notify();
    }

    fn select_remote_session(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = self.remote_session_picker.as_ref() else {
            return;
        };
        if picker.loading {
            return;
        }
        let Some(summary) = picker.sessions.get(index).cloned() else {
            return;
        };
        let target = match Self::remote_target_from_picker(picker)
            .and_then(|target| Ok((target, Self::remote_transport_from_picker(picker)?)))
        {
            Ok((target, transport)) => {
                self.remote_session_transport = transport;
                target
            }
            Err(error) => {
                if let Some(picker) = self.remote_session_picker.as_mut() {
                    picker.error = Some(remote_error_message(&error));
                }
                self.remote_session_focus.focus(window, cx);
                cx.notify();
                return;
            }
        };
        let transport = self.remote_session_transport;
        let operation_generation = self.next_remote_session_operation_generation();
        let picker = self
            .remote_session_picker
            .as_mut()
            .expect("the remote session picker was checked above");
        picker.selected = index;
        picker.attach_generation = Some(operation_generation);
        picker.loading = true;
        picker.error = None;
        picker.move_unavailable_focus_to_cancel();
        let background_target = target.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    load_remote_attach(background_target, summary.id, None, transport)
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.apply_remote_attach_result(
                    operation_generation,
                    target,
                    summary,
                    result,
                    window,
                    cx,
                );
            })
            .ok();
        });
        picker.task.take();
        picker.task = Some(task);
        cx.notify();
    }

    fn apply_remote_attach_result(
        &mut self,
        operation_generation: u64,
        target: zmux::remote::RemoteTarget,
        summary: zmux::protocol::BackgroundSessionSummary,
        result: anyhow::Result<RemoteAttachOutcome>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let is_current = self.remote_session_operation_is_current(operation_generation)
            && self
                .remote_session_picker
                .as_ref()
                .is_some_and(|picker| picker.attach_generation == Some(operation_generation));
        if !is_current {
            return;
        }
        if let Some(picker) = self.remote_session_picker.as_mut() {
            picker.loading = false;
            picker.task = None;
            picker.attach_generation = None;
        }
        match result {
            Ok(RemoteAttachOutcome::Attached(data)) => {
                match self.finish_remote_multiplexer_session(*data, window, cx) {
                    Ok(crate::background_session_ui::AttachOutcomeSummary::Attached) => {
                        self.remote_session_picker = None;
                        self.remote_session_target = None;
                        self.focus_active(window, cx);
                    }
                    Ok(
                        crate::background_session_ui::AttachOutcomeSummary::AuthenticationRequired,
                    )
                    | Ok(
                        crate::background_session_ui::AttachOutcomeSummary::AuthenticationFailed,
                    ) => {
                        if let Some(picker) = self.remote_session_picker.as_mut() {
                            picker.error = Some(
                                "The remote session was attached but could not be shown here."
                                    .to_owned(),
                            );
                        }
                        self.remote_session_focus.focus(window, cx);
                    }
                    Err(error) => {
                        if let Some(picker) = self.remote_session_picker.as_mut() {
                            picker.error = Some(format!(
                                "Could not show remote session {}: {}",
                                summary.id,
                                remote_error_message(&error)
                            ));
                        }
                        self.remote_session_focus.focus(window, cx);
                    }
                }
            }
            Ok(RemoteAttachOutcome::AuthenticationRequired)
            | Ok(RemoteAttachOutcome::AuthenticationFailed)
                if summary.authentication_required =>
            {
                self.remote_session_picker = None;
                #[cfg(feature = "session-persistence")]
                if let Some(envelope) = summary.key_envelope.clone() {
                    self.prompt_to_unlock_remote_session(target, summary.id, envelope, window, cx);
                } else {
                    self.prompt_to_attach_remote_session(target, summary.id, window, cx);
                }
                #[cfg(not(feature = "session-persistence"))]
                self.prompt_to_attach_remote_session(target, summary.id, window, cx);
            }
            Ok(RemoteAttachOutcome::AuthenticationRequired) => {
                self.remote_session_picker = None;
                self.prompt_to_attach_remote_session(target, summary.id, window, cx);
            }
            Ok(RemoteAttachOutcome::AuthenticationFailed) => {
                if let Some(picker) = self.remote_session_picker.as_mut() {
                    picker.error = Some("Authentication failed.".to_owned());
                }
                self.remote_session_focus.focus(window, cx);
            }
            Err(error) => {
                if let Some(picker) = self.remote_session_picker.as_mut() {
                    picker.error = Some(format!(
                        "Could not attach remote session {}: {}",
                        summary.id,
                        remote_error_message(&error)
                    ));
                }
                self.remote_session_focus.focus(window, cx);
            }
        }
        cx.notify();
    }
}
