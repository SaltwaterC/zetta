//! The remote-session picker.
//!
//! SSH discovery is deliberately kept out of the render path. The picker only
//! reads the local SSH config when it opens, and does the endpoint/list request
//! on GPUI's background executor after the user submits a target. The forwarded
//! mux connection is then handed to the ordinary shared-pane attach path.

use super::*;
use crate::background_session_ui::{RemoteAttachOutcome, load_remote_attach};
use crate::config::REMOTE_KEEP_ALIVE_DEFAULT_MS;
use crate::remote_pane_transport::{RemotePaneTransport, parse_keep_alive_interval};
use crate::session_auth_ui::SessionAuthenticationPromptMode;

const REMOTE_SESSION_SUGGESTION_VIEWPORT_ROWS: usize = 6;
const REMOTE_SESSION_SUGGESTION_VIEWPORT_HEIGHT: gpui::Rems =
    gpui::rems(1.75 * REMOTE_SESSION_SUGGESTION_VIEWPORT_ROWS as f32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteSessionField {
    Target,
    Port,
    Protocol,
    KeepAlive,
    Profile,
    Template,
    List,
    Create,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RemoteSessionSuggestionNavigation {
    filter: String,
    selected: usize,
}

/// What pressing Enter in the picker does.
///
/// Deliberately decided from the loaded sessions, or from the focused Create
/// action: every edit to the target or the port runs `invalidate_results`, so a
/// non-empty list always belongs to the target currently in the field and
/// Enter can attach from anywhere in the picker. Explicit re-listing stays on
/// the Load button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemoteSessionEnterAction {
    Attach(usize),
    Load,
    Create,
    Ignore,
}

/// The complete request kept while the optional session-secret prompt is open.
///
/// The picker is dismissed before the prompt is shown, just as it is for a
/// remote attach. Keeping the layout here means a protected create can finish
/// without reconstructing the user's template choices after the prompt.
pub(crate) struct RemoteSessionCreate {
    pub(crate) target: zmux::remote::RemoteTarget,
    pub(crate) transport: RemotePaneTransport,
    pub(crate) spec: zmux::headless::CreateSpec,
}

struct RemoteSessionDiscovery {
    profiles: anyhow::Result<Vec<String>>,
    sessions: anyhow::Result<Vec<zmux::protocol::BackgroundSessionSummary>>,
}

pub(crate) struct RemoteSessionPicker {
    pub(crate) target: TextField,
    pub(crate) port: TextField,
    /// What carries the session's panes. Control traffic is SSH either way,
    /// so this is only ever about the panes themselves.
    pub(crate) transport: RemotePaneTransport,
    /// The keep-alive interval, as typed. Empty means Mosh's own heartbeat;
    /// what it parses to is only asked for when Zosh is the protocol.
    pub(crate) keep_alive: TextField,
    pub(crate) field: RemoteSessionField,
    pub(crate) sessions: Vec<zmux::protocol::BackgroundSessionSummary>,
    pub(crate) selected: usize,
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
    pub(crate) profiles: Vec<String>,
    pub(crate) selected_profile: usize,
    pub(crate) profiles_loading: bool,
    pub(crate) profile_error: Option<String>,
    pub(crate) templates: Vec<String>,
    pub(crate) selected_template: usize,
    pub(crate) creating: bool,
    pub(crate) suggestions: Vec<String>,
    suggestion_navigation: Option<RemoteSessionSuggestionNavigation>,
    pub(crate) suggestion_scroll: UniformListScrollHandle,
    pub(crate) scroll: UniformListScrollHandle,
    pub(crate) generation: u64,
    pub(crate) attach_generation: Option<u64>,
    pub(crate) task: Option<Task<()>>,
}

impl Default for RemoteSessionPicker {
    fn default() -> Self {
        Self {
            target: TextField::default(),
            port: TextField::default(),
            transport: RemotePaneTransport::default(),
            keep_alive: TextField::default(),
            field: RemoteSessionField::Target,
            sessions: Vec::new(),
            selected: 0,
            loading: false,
            error: None,
            profiles: Vec::new(),
            selected_profile: 0,
            profiles_loading: false,
            profile_error: None,
            templates: Vec::new(),
            selected_template: 0,
            creating: false,
            suggestions: Vec::new(),
            suggestion_navigation: None,
            suggestion_scroll: UniformListScrollHandle::new(),
            scroll: UniformListScrollHandle::new(),
            generation: 0,
            attach_generation: None,
            task: None,
        }
    }
}

#[cfg(test)]
#[path = "tests/remote_session_ui.rs"]
mod tests;

impl RemoteSessionPicker {
    fn invalidate_results(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.attach_generation = None;
        self.task.take();
        self.sessions.clear();
        self.selected = 0;
        self.loading = false;
        self.error = None;
        self.profiles.clear();
        self.selected_profile = 0;
        self.profiles_loading = false;
        self.profile_error = None;
        self.creating = false;
    }

    /// The fields in tab order.
    ///
    /// Keep-alive is only reachable while Zosh is the protocol: it holds a
    /// Mosh link open, and an SSH session has no link of that kind to hold.
    fn field_order(&self) -> &'static [RemoteSessionField] {
        const WITH_KEEP_ALIVE: &[RemoteSessionField] = &[
            RemoteSessionField::Target,
            RemoteSessionField::Port,
            RemoteSessionField::Protocol,
            RemoteSessionField::KeepAlive,
            RemoteSessionField::Profile,
            RemoteSessionField::Template,
            RemoteSessionField::List,
            RemoteSessionField::Create,
        ];
        const WITHOUT_KEEP_ALIVE: &[RemoteSessionField] = &[
            RemoteSessionField::Target,
            RemoteSessionField::Port,
            RemoteSessionField::Protocol,
            RemoteSessionField::Profile,
            RemoteSessionField::Template,
            RemoteSessionField::List,
            RemoteSessionField::Create,
        ];
        if self.transport.is_zosh() {
            WITH_KEEP_ALIVE
        } else {
            WITHOUT_KEEP_ALIVE
        }
    }

    fn cycle_field(&mut self, reverse: bool) {
        let order = self.field_order();
        let current = order
            .iter()
            .position(|field| *field == self.field)
            .unwrap_or(0);
        let next = if reverse {
            (current + order.len() - 1) % order.len()
        } else {
            (current + 1) % order.len()
        };
        self.field = order[next];
    }

    /// Switches between carrying the panes over SSH and over Zosh.
    ///
    /// Changing the protocol invalidates nothing that was loaded: the session
    /// list comes from the control connection, which is SSH either way.
    fn toggle_transport(&mut self) {
        self.transport = if self.transport.is_zosh() {
            RemotePaneTransport::Ssh
        } else {
            // Choosing Zosh with nothing in the field means the same thing
            // `-k` does on the command line: hold the link open at the default
            // interval. Emptying the field afterwards is how to ask for Mosh's
            // own heartbeat instead.
            if self.keep_alive.text.trim().is_empty() {
                self.keep_alive = TextField::new(REMOTE_KEEP_ALIVE_DEFAULT_MS.to_string());
            }
            RemotePaneTransport::Zosh {
                keep_alive_ms: None,
            }
        };
        if !self.transport.is_zosh() && self.field == RemoteSessionField::KeepAlive {
            self.field = RemoteSessionField::Protocol;
        }
    }

    fn cycle_profile(&mut self, reverse: bool) {
        if self.profiles.is_empty() {
            return;
        }
        self.selected_profile = cycle_index(self.selected_profile, self.profiles.len(), reverse);
    }

    fn cycle_template(&mut self, reverse: bool) {
        if self.templates.is_empty() {
            return;
        }
        self.selected_template = cycle_index(self.selected_template, self.templates.len(), reverse);
    }

    fn reset_suggestion_navigation(&mut self) {
        self.suggestion_navigation = None;
        self.suggestion_scroll
            .scroll_to_item(0, ScrollStrategy::Top);
    }

    fn visible_suggestions(&self) -> Vec<String> {
        let filter = self
            .suggestion_navigation
            .as_ref()
            .map_or(self.target.text.as_str(), |navigation| {
                navigation.filter.as_str()
            })
            .to_lowercase();
        self.suggestions
            .iter()
            .filter(|suggestion| {
                filter.is_empty() || suggestion.to_lowercase().starts_with(&filter)
            })
            .cloned()
            .collect()
    }

    fn enter_action(&self) -> RemoteSessionEnterAction {
        if self.loading || self.creating || self.profiles_loading {
            return RemoteSessionEnterAction::Ignore;
        }
        if self.field == RemoteSessionField::Create {
            return if self.can_create() {
                RemoteSessionEnterAction::Create
            } else {
                RemoteSessionEnterAction::Ignore
            };
        }
        match self.sessions.len() {
            0 => RemoteSessionEnterAction::Load,
            count => RemoteSessionEnterAction::Attach(self.selected.min(count - 1)),
        }
    }

    fn can_create(&self) -> bool {
        !self.profiles.is_empty()
            && !self.templates.is_empty()
            && self.selected_profile < self.profiles.len()
            && self.selected_template < self.templates.len()
    }

    fn navigate_suggestions(&mut self, reverse: bool) -> bool {
        let suggestions = self.visible_suggestions();
        if suggestions.is_empty() {
            return false;
        }
        let filter = self.suggestion_navigation.as_ref().map_or_else(
            || self.target.text.clone(),
            |navigation| navigation.filter.clone(),
        );
        let selected = match self.suggestion_navigation.as_ref() {
            Some(navigation) => {
                let selected = navigation.selected % suggestions.len();
                if reverse {
                    (selected + suggestions.len() - 1) % suggestions.len()
                } else {
                    (selected + 1) % suggestions.len()
                }
            }
            None if reverse => suggestions.len() - 1,
            None => 0,
        };
        self.target = TextField::new(suggestions[selected].clone());
        self.invalidate_results();
        self.suggestion_navigation = Some(RemoteSessionSuggestionNavigation { filter, selected });
        self.suggestion_scroll
            .scroll_to_item(selected, ScrollStrategy::Nearest);
        true
    }
}

fn cycle_index(current: usize, count: usize, reverse: bool) -> usize {
    if reverse {
        current.saturating_add(count).saturating_sub(1) % count
    } else {
        current.saturating_add(1) % count
    }
}

fn sorted_names(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut values = values.into_iter().collect::<Vec<_>>();
    values.sort_unstable_by_key(|value| value.to_ascii_lowercase());
    values
}

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
        let templates = sorted_names(self.effective_config().pane_split_templates.keys().cloned());
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
        let target = zmux::remote::RemoteTarget::new(destination).with_port(port);
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
        Ok(RemotePaneTransport::Zosh { keep_alive_ms })
    }

    pub(crate) fn create_remote_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (values, can_create) = {
            let Some(picker) = self.remote_session_picker.as_ref() else {
                return;
            };
            let values = Self::remote_target_from_picker(picker).and_then(|target| {
                Ok((
                    target,
                    Self::remote_transport_from_picker(picker)?,
                    picker
                        .templates
                        .get(picker.selected_template)
                        .cloned()
                        .context("select a remote session template")?,
                    picker
                        .profiles
                        .get(picker.selected_profile)
                        .cloned()
                        .context("load remote profiles before creating a session")?,
                    picker.profiles.clone(),
                ))
            });
            (values, picker.can_create())
        };
        let (target, transport, template_name, profile_name, profiles) = match values {
            Ok(values) if can_create => values,
            Ok(_) => {
                if let Some(picker) = self.remote_session_picker.as_mut() {
                    picker.profile_error =
                        Some("Load the remote profiles before creating a session.".to_owned());
                }
                self.remote_session_focus.focus(window, cx);
                cx.notify();
                return;
            }
            Err(error) => {
                if let Some(picker) = self.remote_session_picker.as_mut() {
                    picker.error = Some(format!("{error:#}"));
                }
                self.remote_session_focus.focus(window, cx);
                cx.notify();
                return;
            }
        };
        let spec = match build_remote_create_spec(
            self.effective_config(),
            &template_name,
            &profile_name,
            &profiles,
        ) {
            Ok(spec) => spec,
            Err(error) => {
                if let Some(picker) = self.remote_session_picker.as_mut() {
                    picker.error = Some(format!("{error:#}"));
                }
                self.remote_session_focus.focus(window, cx);
                cx.notify();
                return;
            }
        };
        self.remote_session_target = Some(target.clone());
        self.remote_session_transport = transport;
        self.remote_session_create = Some(RemoteSessionCreate {
            target,
            transport,
            spec,
        });
        self.remote_session_picker = None;
        self.open_session_authentication_prompt(
            SessionAuthenticationPromptMode::RemoteCreate,
            window,
            cx,
        );
    }

    pub(crate) fn start_remote_session_create(
        &mut self,
        authentication: Option<SessionAuthentication>,
        secret: Option<SessionSecret>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(create) = self.remote_session_create.take() else {
            self.show_notice(
                "The remote session create request is no longer available.",
                cx,
            );
            self.remote_session_target = None;
            self.focus_active(window, cx);
            return;
        };
        let operation_generation = self.next_remote_session_operation_generation();
        let RemoteSessionCreate {
            target,
            transport,
            spec,
        } = create;
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let client = zmux::client::Client::connect_remote_for_creation(target.clone())
                        .context("connecting to the remote multiplexer")?;
                    let request = spec.request(
                        client.next_shared_operation_id(),
                        authentication.map(|authentication| {
                            authentication.verifier().to_owned()
                        }),
                    );
                    let created = client
                        .create_shared(request)
                        .context("creating the remote headless session")?;
                    let session_id = created.session_id;
                    let attached = load_remote_attach(
                        target,
                        session_id,
                        secret,
                        transport,
                    )
                    .map_err(|error| {
                        anyhow::anyhow!(
                            "created remote session {session_id}, but could not attach it: {error:#}"
                        )
                    })?;
                    Ok::<_, anyhow::Error>((session_id, attached))
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.apply_remote_create_result(
                    operation_generation,
                    result,
                    window,
                    cx,
                );
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn apply_remote_create_result(
        &mut self,
        operation_generation: u64,
        result: anyhow::Result<(u64, RemoteAttachOutcome)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.remote_session_operation_is_current(operation_generation) {
            return;
        }
        match result {
            Ok((session_id, RemoteAttachOutcome::Attached(data))) => {
                match self.finish_remote_multiplexer_session(*data, window, cx) {
                    Ok(crate::background_session_ui::AttachOutcomeSummary::Attached) => {
                        self.remote_session_target = None;
                        self.focus_active(window, cx);
                    }
                    Ok(_) => self.show_notice(
                        format!(
                            "Created remote session {session_id}, but it could not be shown here."
                        ),
                        cx,
                    ),
                    Err(error) => self.show_notice(
                        format!(
                            "Created remote session {session_id}, but could not show it: {error:#}"
                        ),
                        cx,
                    ),
                }
            }
            Ok((session_id, RemoteAttachOutcome::AuthenticationRequired))
            | Ok((session_id, RemoteAttachOutcome::AuthenticationFailed)) => {
                self.show_notice(
                    format!(
                        "Created remote session {session_id}, but authentication failed while attaching."
                    ),
                    cx,
                );
                self.remote_session_target = None;
            }
            Err(error) => {
                self.show_notice(format!("Could not create a remote session: {error:#}"), cx);
                self.remote_session_target = None;
            }
        }
        cx.notify();
    }

    pub(crate) fn load_remote_sessions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = self.remote_session_picker.as_ref() else {
            return;
        };
        let target = match Self::remote_target_from_picker(picker) {
            Ok(target) => target,
            Err(error) => {
                if let Some(picker) = self.remote_session_picker.as_mut() {
                    picker.error = Some(format!("{error:#}"));
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
        picker.error = None;
        picker.profiles.clear();
        picker.selected_profile = 0;
        picker.profiles_loading = true;
        picker.profile_error = None;
        let generation = picker.generation;
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let profiles = zmux::remote::RemoteTransport::for_creation(target.clone())
                        .and_then(|transport| transport.query_profiles())
                        .context("discovering remote profiles");
                    let sessions = zmux::client::Client::connect_remote(target)
                        .and_then(|client| client.list())
                        .context("listing remote sessions");
                    RemoteSessionDiscovery { profiles, sessions }
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
            Err(error) => picker.profile_error = Some(format!("{error:#}")),
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
            Err(error) => picker.error = Some(format!("{error:#}")),
        }
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
            Err(error) => picker.error = Some(format!("{error:#}")),
        }
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
                    picker.error = Some(format!("{error:#}"));
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
        picker.attach_generation = Some(operation_generation);
        picker.loading = true;
        picker.error = None;
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
                                "Could not show remote session {}: {error:#}",
                                summary.id
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
                        "Could not attach remote session {}: {error:#}",
                        summary.id
                    ));
                }
                self.remote_session_focus.focus(window, cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn remote_session_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.remote_session_picker.is_none() {
            return false;
        }
        if event.keystroke.key == "escape" {
            self.dismiss_remote_session_picker(window, cx);
            cx.stop_propagation();
            return true;
        }
        if event.keystroke.key == "enter" {
            let action = self
                .remote_session_picker
                .as_ref()
                .map(RemoteSessionPicker::enter_action);
            match action {
                Some(RemoteSessionEnterAction::Attach(selected)) => {
                    self.select_remote_session(selected, window, cx);
                }
                Some(RemoteSessionEnterAction::Load) => self.load_remote_sessions(window, cx),
                Some(RemoteSessionEnterAction::Create) => self.create_remote_session(window, cx),
                Some(RemoteSessionEnterAction::Ignore) | None => {}
            }
            cx.stop_propagation();
            return true;
        }
        let Some(picker) = self.remote_session_picker.as_mut() else {
            return false;
        };
        match (event.keystroke.key.as_str(), picker.field) {
            ("tab", _) => {
                picker.cycle_field(event.keystroke.modifiers.shift);
                picker.reset_suggestion_navigation();
                cx.notify();
            }
            ("left" | "right" | "space", RemoteSessionField::Protocol) => {
                picker.toggle_transport();
                cx.notify();
            }
            ("left" | "up" | "right" | "down", RemoteSessionField::Profile) => {
                picker.cycle_profile(matches!(event.keystroke.key.as_str(), "left" | "up"));
                cx.notify();
            }
            ("left" | "up" | "right" | "down", RemoteSessionField::Template) => {
                picker.cycle_template(matches!(event.keystroke.key.as_str(), "left" | "up"));
                cx.notify();
            }
            ("up" | "down", RemoteSessionField::Target) => {
                picker.navigate_suggestions(event.keystroke.key == "up");
                cx.notify();
            }
            ("up" | "down", RemoteSessionField::List) if !picker.sessions.is_empty() => {
                if event.keystroke.key == "up" {
                    picker.selected = picker.selected.saturating_sub(1);
                } else {
                    picker.selected = (picker.selected + 1).min(picker.sessions.len() - 1);
                }
                picker
                    .scroll
                    .scroll_to_item(picker.selected, ScrollStrategy::Nearest);
                cx.notify();
            }
            (
                _,
                RemoteSessionField::Target
                | RemoteSessionField::Port
                | RemoteSessionField::KeepAlive,
            ) => {
                let numeric = matches!(
                    picker.field,
                    RemoteSessionField::Port | RemoteSessionField::KeepAlive
                );
                let field = match picker.field {
                    RemoteSessionField::Target => &mut picker.target,
                    RemoteSessionField::Port => &mut picker.port,
                    RemoteSessionField::KeepAlive => &mut picker.keep_alive,
                    RemoteSessionField::Protocol
                    | RemoteSessionField::Profile
                    | RemoteSessionField::Template
                    | RemoteSessionField::List
                    | RemoteSessionField::Create => unreachable!(),
                };
                match apply_clipboard_shortcut(field, &event.keystroke, cx) {
                    ClipboardOutcome::Unchanged => {
                        cx.notify();
                        cx.stop_propagation();
                        return true;
                    }
                    ClipboardOutcome::Edited => {
                        if picker.field == RemoteSessionField::Target {
                            picker.reset_suggestion_navigation();
                        }
                        picker.invalidate_results();
                        cx.notify();
                        cx.stop_propagation();
                        return true;
                    }
                    ClipboardOutcome::Ignored => {}
                }
                // The port and keep-alive fields take digits only, so a
                // character that is not one is dropped before the field sees
                // it; everything else is the shared editing behaviour.
                let typed_a_rejected_character = numeric
                    && event
                        .keystroke
                        .key_char
                        .as_ref()
                        .is_some_and(|text| !text.chars().all(|c| c.is_ascii_digit()));
                let edit = if typed_a_rejected_character {
                    TextFieldEdit::Ignored
                } else {
                    apply_text_field_key(field, &event.keystroke)
                };
                if picker.field == RemoteSessionField::Target && edit == TextFieldEdit::Edited {
                    picker.reset_suggestion_navigation();
                }
                picker.invalidate_results();
                cx.notify();
            }
            _ => {}
        }
        cx.stop_propagation();
        true
    }

    pub(crate) fn remote_session_key_down_capture(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.remote_session_picker.is_some() && event.keystroke.key == "escape" {
            self.dismiss_remote_session_picker(window, cx);
            cx.stop_propagation();
        }
    }

    pub(crate) fn render_remote_session_overlay(
        &self,
        colors: &ThemeColors,
        error_color: Hsla,
        handle: &WeakEntity<Self>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let picker = self.remote_session_picker.as_ref()?;
        let target = picker.target.clone();
        let port = picker.port.clone();
        let transport = picker.transport;
        let keep_alive = picker.keep_alive.clone();
        let field = picker.field;
        let error = picker.error.clone();
        let loading = picker.loading;
        let session_count = picker.sessions.len();
        let selected = picker.selected.min(session_count.saturating_sub(1));
        let sessions = picker.sessions.clone();
        let suggestions = picker.visible_suggestions();
        let suggestion_selected = picker
            .suggestion_navigation
            .as_ref()
            .map(|navigation| navigation.selected);
        let suggestion_scroll = picker.suggestion_scroll.clone();
        let picker_scroll = picker.scroll.clone();
        let profiles_loading = picker.profiles_loading;
        let profile_value = picker
            .profiles
            .get(picker.selected_profile)
            .cloned()
            .unwrap_or_else(|| {
                if profiles_loading {
                    "Loading…".to_owned()
                } else {
                    "Load remote profiles".to_owned()
                }
            });
        let template_value = picker
            .templates
            .get(picker.selected_template)
            .cloned()
            .unwrap_or_else(|| "No templates configured".to_owned());
        let profile_error = picker.profile_error.clone();
        let can_create = picker.can_create();
        let creating = picker.creating;
        let rows = remote_session_rows(handle, colors, &picker_scroll, sessions, selected);

        let cancel_handle = handle.clone();
        let load_handle = handle.clone();
        let attach_handle = handle.clone();
        let create_handle = handle.clone();
        let has_suggestions = !suggestions.is_empty();
        let suggestion_rows = remote_session_suggestion_rows(
            handle,
            colors,
            &suggestion_scroll,
            suggestions,
            suggestion_selected,
        );
        let suggestion_rows =
            remote_session_suggestion_list(suggestion_rows, &suggestion_scroll, window, cx);

        let session_list = div()
            .id("remote-session-list-panel")
            .w_full()
            .rounded(px(4.))
            .border_1()
            .border_color(if field == RemoteSessionField::List {
                colors.border_focused
            } else {
                transparent_black()
            })
            .when(session_count > 0, |panel| panel.child(rows))
            .when(session_count == 0 && !loading && error.is_none(), |panel| {
                panel.child(
                    div()
                        .h_16()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_sm()
                        .text_color(colors.text_muted)
                        .child("Enter a target and press Enter to load sessions."),
                )
            })
            .when_some(error, |panel, error| {
                panel.child(div().p_2().text_sm().text_color(error_color).child(error))
            });

        let field_widget = |id: &'static str,
                            value: TextField,
                            selected_field: RemoteSessionField,
                            placeholder: &'static str,
                            click_handle: WeakEntity<Self>| {
            remote_session_field(
                id,
                value,
                selected_field,
                placeholder,
                field,
                colors,
                click_handle,
            )
        };
        Some(
            div()
                .id("remote-session-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(transparent_black().opacity(0.24))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .id("remote-session-picker")
                        .track_focus(&self.remote_session_focus)
                        .w_full()
                        .max_w(px(680.))
                        .max_h(gpui::relative(0.9))
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background)
                        .text_color(colors.text)
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(Label::new("Open remote session").size(LabelSize::Large))
                        .child(
                            div()
                                .text_sm()
                                .text_color(colors.text_muted)
                                .child(remote_session_description(transport)),
                        )
                        .child(remote_session_fields(&field_widget, target, port, handle))
                        .child(remote_session_transport_row(RemoteSessionTransportRow {
                            transport,
                            keep_alive,
                            field,
                            colors,
                            field_widget: &field_widget,
                            handle,
                        }))
                        .child(remote_session_create_options(RemoteSessionCreateOptions {
                            profile: profile_value,
                            template: template_value,
                            profiles_loading,
                            profile_error,
                            field,
                            colors,
                            error_color,
                            handle,
                        }))
                        .when(
                            field == RemoteSessionField::Target && has_suggestions,
                            |panel| panel.child(suggestion_rows),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(Label::new("Shared sessions").size(LabelSize::Small))
                                .child(div().text_xs().text_color(colors.text_muted).child(
                                    if loading {
                                        "Loading…".to_owned()
                                    } else {
                                        format!(
                                            "{session_count} session{}",
                                            if session_count == 1 { "" } else { "s" }
                                        )
                                    },
                                )),
                        )
                        .child(session_list)
                        .child(remote_session_actions(RemoteSessionActions {
                            loading,
                            session_count,
                            selected,
                            colors,
                            cancel_handle,
                            load_handle,
                            attach_handle,
                            create_handle,
                            profiles_ready: can_create,
                            creating,
                        })),
                )
                .into_any_element(),
        )
    }
}

fn build_remote_create_spec(
    config: &Config,
    template_name: &str,
    default_profile: &str,
    remote_profiles: &[String],
) -> anyhow::Result<zmux::headless::CreateSpec> {
    let template = config
        .pane_split_templates
        .get(template_name)
        .with_context(|| format!("remote template {template_name:?} is no longer available"))?;
    let mut panes = Vec::with_capacity(template.pane_count());
    let mut next_draft_id = 1;
    let layout = build_remote_create_layout(
        &template.layout,
        &template.env,
        default_profile,
        remote_profiles,
        &mut panes,
        &mut next_draft_id,
    )?;
    let active_pane = panes
        .first()
        .map(|pane| zmux::messages::SharedPaneRef::Draft {
            draft_id: pane.draft_id,
        })
        .context("remote session template has no panes")?;
    Ok(zmux::headless::CreateSpec {
        title: String::new(),
        layout,
        active_pane,
        panes,
    })
}

fn build_remote_create_layout(
    node: &PaneSplitTemplate,
    inherited_env: &HashMap<String, String>,
    default_profile: &str,
    remote_profiles: &[String],
    panes: &mut Vec<zmux::messages::SharedPaneDraft>,
    next_draft_id: &mut u64,
) -> anyhow::Result<zmux::messages::SharedDraftLayout> {
    match node {
        PaneSplitTemplate::Split {
            axis,
            first,
            second,
        } => {
            let first = build_remote_create_layout(
                first,
                inherited_env,
                default_profile,
                remote_profiles,
                panes,
                next_draft_id,
            )?;
            let second = build_remote_create_layout(
                second,
                inherited_env,
                default_profile,
                remote_profiles,
                panes,
                next_draft_id,
            )?;
            Ok(zmux::messages::SharedDraftLayout::Split {
                axis: axis.as_str().to_owned(),
                first_ratio: zmux::protocol::DEFAULT_BACKGROUND_PANE_SPLIT_RATIO,
                first: Box::new(first),
                second: Box::new(second),
            })
        }
        PaneSplitTemplate::Pane(pane) => {
            anyhow::ensure!(
                pane.theme.is_none()
                    && pane.dark_theme.is_none()
                    && pane.overlay.is_none()
                    && pane.stack.is_empty(),
                "remote headless creation does not support pane themes, overlays, or stacked commands"
            );
            let draft_id = *next_draft_id;
            *next_draft_id = next_draft_id.saturating_add(1);
            let profile = pane.profile.as_ref().map_or_else(
                || default_profile.to_owned(),
                |profile| profile.name.clone(),
            );
            let command = pane.command.as_ref().map(|command| {
                zetta_profiles::ProfileCommand::with_args(
                    command.program.clone(),
                    command.args.clone(),
                )
            });
            if command.is_none() {
                anyhow::ensure!(
                    remote_profiles
                        .iter()
                        .any(|remote| remote.eq_ignore_ascii_case(&profile)),
                    "profile {profile:?} is not available on the remote host"
                );
            }
            let mut env = inherited_env.clone();
            env.extend(pane.env.clone());
            let label = pane
                .label
                .clone()
                .unwrap_or_else(|| format!("pane-{draft_id}"));
            let application = command
                .as_ref()
                .and_then(|command| command.program.clone())
                .unwrap_or_else(|| profile.clone());
            panes.push(zmux::messages::SharedPaneDraft {
                draft_id,
                profile: profile.clone(),
                command,
                env,
                working_directory: None,
                inherit_working_directory_from: None,
                load_shell_integration: true,
                size: zmux::messages::TerminalSize {
                    columns: zmux::headless::DEFAULT_COLUMNS,
                    lines: zmux::headless::DEFAULT_LINES,
                    cell_width: 0,
                    cell_height: 0,
                },
                console_palette: terminal::ConsolePalette::default(),
                metadata: zmux::protocol::BackgroundPaneSummary {
                    id: 0,
                    label,
                    profile,
                    configured_command: String::new(),
                    application,
                    foreground_command: None,
                    terminal_title: None,
                    working_directory: None,
                    state: zmux::protocol::BackgroundPaneState::Starting,
                    exit: None,
                },
            });
            Ok(zmux::messages::SharedDraftLayout::Draft { draft_id })
        }
    }
}

/// The session list: one row per session the remote host is holding.
///
/// Virtualized, because a host that has been up for a while can be holding
/// more sessions than the panel shows at once.
fn remote_session_rows(
    handle: &WeakEntity<Zetta>,
    colors: &ThemeColors,
    picker_scroll: &gpui::UniformListScrollHandle,
    sessions: Vec<zmux::protocol::BackgroundSessionSummary>,
    selected: usize,
) -> gpui::UniformList {
    let row_colors = colors.clone();
    let row_handle = handle.clone();
    uniform_list(
        "remote-session-list",
        sessions.len(),
        move |range: std::ops::Range<usize>, _, _| {
            range
                .map(|index| {
                    let session = &sessions[index];
                    let row_handle = row_handle.clone();
                    let title = if session.title.is_empty() {
                        format!("Session {}", session.id)
                    } else {
                        session.title.clone()
                    };
                    let detail = if session.authentication_required {
                        "Protected session · secret required".to_owned()
                    } else {
                        format!(
                            "{} pane{}{}",
                            session.panes.len(),
                            if session.panes.len() == 1 { "" } else { "s" },
                            if session.held {
                                " · already in use"
                            } else {
                                ""
                            }
                        )
                    };
                    div()
                        .id(("remote-session-row", index))
                        .h_12()
                        .w_full()
                        .px_3()
                        .flex()
                        .flex_col()
                        .justify_center()
                        .cursor_pointer()
                        .border_1()
                        .border_color(if index == selected {
                            row_colors.border_focused
                        } else {
                            transparent_black()
                        })
                        .when(index == selected, |row| row.bg(row_colors.element_selected))
                        .hover(|style| style.bg(row_colors.element_hover))
                        .on_click(move |_, window, cx| {
                            row_handle
                                .update(cx, |this, cx| {
                                    this.select_remote_session(index, window, cx);
                                })
                                .ok();
                        })
                        .child(div().text_sm().child(title))
                        .child(
                            div()
                                .text_xs()
                                .text_color(row_colors.text_muted)
                                .child(detail),
                        )
                })
                .collect::<Vec<_>>()
        },
    )
    .with_sizing_behavior(ListSizingBehavior::Infer)
    .max_h(px(280.))
    .track_scroll(picker_scroll)
    .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
}

/// The host suggestions under the target field, from the user's SSH
/// configuration.
fn remote_session_suggestion_rows(
    handle: &WeakEntity<Zetta>,
    colors: &ThemeColors,
    suggestion_scroll: &gpui::UniformListScrollHandle,
    suggestions: Vec<String>,
    selected: Option<usize>,
) -> gpui::UniformList {
    let row_colors = colors.clone();
    let row_handle = handle.clone();
    uniform_list(
        "remote-session-suggestions",
        suggestions.len(),
        move |range: std::ops::Range<usize>, _, _| {
            range
                .map(|index| {
                    let suggestion = suggestions[index].clone();
                    let suggestion_handle = row_handle.clone();
                    let suggestion_label = suggestion.clone();
                    div()
                        .id(("remote-session-suggestion", index))
                        .debug_selector(|| format!("remote-session-suggestion-{index}"))
                        .h_7()
                        .w_full()
                        .px_2()
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .text_xs()
                        .text_color(row_colors.text_muted)
                        .border_1()
                        .border_color(if selected == Some(index) {
                            row_colors.border_focused
                        } else {
                            transparent_black()
                        })
                        .when(selected == Some(index), |row| {
                            row.bg(row_colors.element_selected)
                        })
                        .hover(|style| style.bg(row_colors.element_hover))
                        .on_click(move |_, _, cx| {
                            suggestion_handle
                                .update(cx, |this, cx| {
                                    if let Some(picker) = this.remote_session_picker.as_mut() {
                                        picker.target = TextField::new(suggestion.clone());
                                        picker.field = RemoteSessionField::Port;
                                        picker.reset_suggestion_navigation();
                                        picker.invalidate_results();
                                        cx.notify();
                                    }
                                })
                                .ok();
                        })
                        .child(suggestion_label)
                })
                .collect::<Vec<_>>()
        },
    )
    .with_sizing_behavior(ListSizingBehavior::Infer)
    .max_h(REMOTE_SESSION_SUGGESTION_VIEWPORT_HEIGHT)
    .track_scroll(suggestion_scroll)
    .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
}

fn remote_session_suggestion_list(
    rows: gpui::UniformList,
    suggestion_scroll: &gpui::UniformListScrollHandle,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Div {
    div().relative().w_full().child(rows).child(
        div()
            .id("remote-session-suggestions-scrollbar-layer")
            .debug_selector(|| "remote-session-suggestions-scrollbar".to_owned())
            .absolute()
            .inset_0()
            .custom_scrollbars(
                Scrollbars::always_visible(ScrollAxes::Vertical)
                    .tracked_scroll_handle(suggestion_scroll)
                    .id("remote-session-suggestions-scrollbar-state"),
                window,
                cx,
            ),
    )
}

/// One of the picker's two text fields — the SSH target and the port.
///
/// A shared builder rather than two spellings: they differ only in what they
/// hold and which field selecting them focuses.
fn remote_session_field(
    id: &'static str,
    value: TextField,
    selected_field: RemoteSessionField,
    placeholder: &'static str,
    field: RemoteSessionField,
    colors: &ThemeColors,
    click_handle: WeakEntity<Zetta>,
) -> AnyElement {
    let focused = field == selected_field;
    let (before, after) = value.split_at_cursor();
    field_box(id, focused, colors)
        .flex_1()
        .min_w_0()
        .cursor_text()
        .when(value.select_all && focused, |input| {
            input.bg(colors.element_selection_background)
        })
        .when(focused && !value.select_all, |input| {
            input
                .child(div().whitespace_nowrap().child(before.to_owned()))
                .child(caret(colors))
                .child(div().whitespace_nowrap().child(after.to_owned()))
        })
        .when(focused && value.select_all, |input| {
            input.child(div().whitespace_nowrap().child(value.text.clone()))
        })
        .when(!focused, |input| {
            input.child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(if value.text.is_empty() {
                        colors.text_placeholder
                    } else {
                        colors.text
                    })
                    .child(if value.text.is_empty() {
                        placeholder.to_owned()
                    } else {
                        value.text.clone()
                    }),
            )
        })
        .on_click(move |_, _, cx| {
            click_handle
                .update(cx, |this, cx| {
                    if let Some(picker) = this.remote_session_picker.as_mut() {
                        if selected_field == RemoteSessionField::Target {
                            picker.reset_suggestion_navigation();
                        }
                        picker.field = selected_field;
                        cx.notify();
                    }
                })
                .ok();
        })
        .into_any_element()
}

/// What the picker says it is about to do, which depends on what carries the
/// panes. Control traffic is OpenSSH either way, and that is the part a user
/// has to have configured.
fn remote_session_description(transport: RemotePaneTransport) -> &'static str {
    if transport.is_zosh() {
        "Sessions are found through your normal OpenSSH configuration, and each pane is then \
         carried over Zosh. Remote sessions must be shared."
    } else {
        "Connect through your normal OpenSSH configuration. Remote sessions must be shared."
    }
}

/// What the protocol row needs. A bundle rather than a parameter list: it is
/// the picker's own state plus the two things every row here is built from.
struct RemoteSessionTransportRow<'a, F> {
    transport: RemotePaneTransport,
    keep_alive: TextField,
    field: RemoteSessionField,
    colors: &'a ThemeColors,
    field_widget: &'a F,
    handle: &'a WeakEntity<Zetta>,
}

/// The protocol choice, and the keep-alive interval that only Zosh has.
fn remote_session_transport_row<F>(row: RemoteSessionTransportRow<'_, F>) -> impl IntoElement
where
    F: Fn(
        &'static str,
        TextField,
        RemoteSessionField,
        &'static str,
        WeakEntity<Zetta>,
    ) -> AnyElement,
{
    let RemoteSessionTransportRow {
        transport,
        keep_alive,
        field,
        colors,
        field_widget,
        handle,
    } = row;
    h_flex()
        .w_full()
        .gap_2()
        .items_center()
        .child(
            div()
                .flex_none()
                .text_xs()
                .text_color(colors.text_muted)
                .child("Panes over"),
        )
        .child(remote_session_protocol_control(
            transport,
            field == RemoteSessionField::Protocol,
            colors,
            handle,
        ))
        .when(transport.is_zosh(), |row| {
            row.child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(colors.text_muted)
                    .child("Keep-alive"),
            )
            .child(div().flex_none().w(px(120.)).child(field_widget(
                "remote-session-keep-alive",
                keep_alive,
                RemoteSessionField::KeepAlive,
                KEEP_ALIVE_PLACEHOLDER,
                handle.clone(),
            )))
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(colors.text_muted)
                    .child("ms"),
            )
        })
}

/// The placeholder names what an empty field means, which is not "nothing":
/// Mosh still has its own three-second heartbeat.
const KEEP_ALIVE_PLACEHOLDER: &str = "Off";

/// Two buttons that behave as one control: the protocol the panes take.
fn remote_session_protocol_control(
    transport: RemotePaneTransport,
    focused: bool,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> impl IntoElement {
    h_flex()
        .flex_none()
        .rounded(px(4.))
        .border_1()
        .border_color(if focused {
            colors.border_focused
        } else {
            colors.border
        })
        .children(
            [RemotePaneTransport::SSH, RemotePaneTransport::ZOSH].map(|name| {
                let selected = transport.name() == name;
                let handle = handle.clone();
                div()
                    .id(SharedString::from(format!(
                        "remote-session-protocol-{name}"
                    )))
                    .debug_selector(move || format!("remote-session-protocol-{name}"))
                    .px_3()
                    .py_1()
                    .text_xs()
                    .cursor_pointer()
                    .when(selected, |option| {
                        option.bg(colors.element_selected).text_color(colors.text)
                    })
                    .when(!selected, |option| option.text_color(colors.text_muted))
                    .hover(|style| style.bg(colors.element_hover))
                    .on_click(move |_, _, cx| {
                        handle
                            .update(cx, |this, cx| {
                                let Some(picker) = this.remote_session_picker.as_mut() else {
                                    return;
                                };
                                if picker.transport.name() != name {
                                    picker.toggle_transport();
                                }
                                picker.field = RemoteSessionField::Protocol;
                                cx.notify();
                            })
                            .ok();
                    })
                    .child(if name == RemotePaneTransport::SSH {
                        "SSH"
                    } else {
                        "Zosh"
                    })
            }),
        )
}

/// The SSH target and port fields, side by side.
fn remote_session_fields(
    field_widget: &impl Fn(
        &'static str,
        TextField,
        RemoteSessionField,
        &'static str,
        WeakEntity<Zetta>,
    ) -> AnyElement,
    target: TextField,
    port: TextField,
    handle: &WeakEntity<Zetta>,
) -> impl IntoElement {
    let target_handle = handle.clone();
    let port_handle = handle.clone();
    h_flex()
        .w_full()
        .gap_2()
        .child(field_widget(
            "remote-session-target",
            target,
            RemoteSessionField::Target,
            "SSH target or alias",
            target_handle,
        ))
        .child(div().flex_none().w(px(120.)).child(field_widget(
            "remote-session-port",
            port,
            RemoteSessionField::Port,
            "Port",
            port_handle,
        )))
}

/// The choices that turn the remote-session picker into a create form. They
/// are still picker fields, so the same keyboard tab order and visible focus
/// treatment work for both the list and the create action.
struct RemoteSessionCreateOptions<'a> {
    profile: String,
    template: String,
    profiles_loading: bool,
    profile_error: Option<String>,
    field: RemoteSessionField,
    colors: &'a ThemeColors,
    error_color: Hsla,
    handle: &'a WeakEntity<Zetta>,
}

fn remote_session_create_options(options: RemoteSessionCreateOptions<'_>) -> impl IntoElement {
    let RemoteSessionCreateOptions {
        profile,
        template,
        profiles_loading,
        profile_error,
        field,
        colors,
        error_color,
        handle,
    } = options;
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(colors.text_muted)
                .child("Create a new session")
                .when(profiles_loading, |label| {
                    label.child(" · discovering profiles…")
                }),
        )
        .child(
            h_flex()
                .w_full()
                .gap_2()
                .child(remote_session_choice(RemoteSessionChoice {
                    id: "remote-session-profile",
                    label: "Profile",
                    value: profile,
                    field: RemoteSessionField::Profile,
                    focused: field == RemoteSessionField::Profile,
                    colors,
                    handle: handle.clone(),
                }))
                .child(remote_session_choice(RemoteSessionChoice {
                    id: "remote-session-template",
                    label: "Template",
                    value: template,
                    field: RemoteSessionField::Template,
                    focused: field == RemoteSessionField::Template,
                    colors,
                    handle: handle.clone(),
                })),
        )
        .when_some(profile_error, |panel, error| {
            panel.child(div().text_xs().text_color(error_color).child(error))
        })
}

struct RemoteSessionChoice<'a> {
    id: &'static str,
    label: &'static str,
    value: String,
    field: RemoteSessionField,
    focused: bool,
    colors: &'a ThemeColors,
    handle: WeakEntity<Zetta>,
}

fn remote_session_choice(choice: RemoteSessionChoice<'_>) -> impl IntoElement {
    let RemoteSessionChoice {
        id,
        label,
        value,
        field,
        focused,
        colors,
        handle,
    } = choice;
    div()
        .id(id)
        .flex_1()
        .min_w_0()
        .h_10()
        .px_2()
        .flex()
        .flex_col()
        .justify_center()
        .rounded(px(4.))
        .border_1()
        .border_color(if focused {
            colors.border_focused
        } else {
            colors.border
        })
        .cursor_pointer()
        .hover(|style| style.bg(colors.element_hover))
        .on_click(move |_, _, cx| {
            handle
                .update(cx, |this, cx| {
                    let Some(picker) = this.remote_session_picker.as_mut() else {
                        return;
                    };
                    picker.field = field;
                    match field {
                        RemoteSessionField::Profile => picker.cycle_profile(false),
                        RemoteSessionField::Template => picker.cycle_template(false),
                        _ => {}
                    }
                    cx.notify();
                })
                .ok();
        })
        .child(div().text_xs().text_color(colors.text_muted).child(label))
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_sm()
                .child(value),
        )
}

/// What the picker's action row needs to decide which buttons are live.
struct RemoteSessionActions<'a> {
    loading: bool,
    session_count: usize,
    selected: usize,
    profiles_ready: bool,
    creating: bool,
    colors: &'a ThemeColors,
    cancel_handle: WeakEntity<Zetta>,
    load_handle: WeakEntity<Zetta>,
    attach_handle: WeakEntity<Zetta>,
    create_handle: WeakEntity<Zetta>,
}

/// Cancel, Refresh, Create and Attach, with the two session actions live only
/// while their respective background operations are available.
fn remote_session_actions(actions: RemoteSessionActions<'_>) -> impl IntoElement {
    let RemoteSessionActions {
        loading,
        session_count,
        selected,
        profiles_ready,
        creating,
        colors,
        cancel_handle,
        load_handle,
        attach_handle,
        create_handle,
    } = actions;
    div()
        .flex()
        .justify_between()
        .items_center()
        .child(
            div()
                .text_xs()
                .text_color(colors.text_muted)
                .child("Tab next · ↑↓ choose · ←→ change · Enter load/attach/create · Esc cancel"),
        )
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("cancel-remote-session", "Cancel")
                        .style(ButtonStyle::Outlined)
                        .color(Color::Custom(colors.text))
                        .on_click(move |_, window, cx| {
                            cancel_handle
                                .update(cx, |this, cx| {
                                    this.dismiss_remote_session_picker(window, cx);
                                })
                                .ok();
                        }),
                )
                .child(
                    Button::new(
                        "load-remote-sessions",
                        if loading { "Loading…" } else { "Load" },
                    )
                    .style(ButtonStyle::Outlined)
                    .color(Color::Custom(colors.text))
                    .disabled(loading)
                    .on_click(move |_, window, cx| {
                        load_handle
                            .update(cx, |this, cx| {
                                this.load_remote_sessions(window, cx);
                            })
                            .ok();
                    }),
                )
                .child(
                    Button::new(
                        "create-remote-session",
                        if creating { "Creating…" } else { "Create" },
                    )
                    .style(ButtonStyle::Filled)
                    .color(Color::Custom(colors.text))
                    .disabled(loading || creating || !profiles_ready)
                    .on_click(move |_, window, cx| {
                        create_handle
                            .update(cx, |this, cx| {
                                this.create_remote_session(window, cx);
                            })
                            .ok();
                    }),
                )
                .child(
                    Button::new("attach-remote-session", "Attach")
                        .style(ButtonStyle::Filled)
                        .color(Color::Custom(colors.text))
                        .disabled(loading || session_count == 0)
                        .on_click(move |_, window, cx| {
                            attach_handle
                                .update(cx, |this, cx| {
                                    this.select_remote_session(selected, window, cx);
                                })
                                .ok();
                        }),
                ),
        )
}
