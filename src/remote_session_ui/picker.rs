//! The picker's own state: which field has focus and in what order, the
//! suggestions under the target field, its dropdowns, and what Enter does.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteSessionField {
    Target,
    Port,
    Protocol,
    KeepAlive,
    ForwardAgent,
    Profile,
    Template,
    List,
    Cancel,
    Load,
    Create,
    Attach,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RemoteSessionDropdown {
    Profile,
    Template,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RemoteSessionTemplate {
    SinglePane,
    Configured(String),
}

impl RemoteSessionTemplate {
    pub(super) fn label(&self) -> &str {
        match self {
            Self::SinglePane => "Single pane",
            Self::Configured(name) => name,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RemoteSessionSuggestionNavigation {
    pub(super) filter: String,
    pub(super) selected: usize,
}

/// What pressing Enter in the picker does.
///
/// Deliberately decided from the loaded sessions, or from the focused action:
/// every edit to the target or the port runs `invalidate_results`, so a
/// non-empty list always belongs to the target currently in the field and
/// Enter can attach from anywhere in the picker. Explicit re-listing stays on
/// the Load button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RemoteSessionEnterAction {
    Attach(usize),
    Cancel,
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

pub(super) struct RemoteSessionDiscovery {
    pub(super) profiles: anyhow::Result<Vec<String>>,
    pub(super) sessions: anyhow::Result<Vec<zmux::protocol::BackgroundSessionSummary>>,
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
    pub(crate) forward_agent: bool,
    pub(crate) field: RemoteSessionField,
    pub(crate) sessions: Vec<zmux::protocol::BackgroundSessionSummary>,
    pub(crate) selected: usize,
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
    pub(crate) profiles: Vec<String>,
    pub(crate) selected_profile: usize,
    pub(crate) profiles_loading: bool,
    pub(crate) profile_error: Option<String>,
    pub(crate) templates: Vec<RemoteSessionTemplate>,
    pub(crate) selected_template: usize,
    pub(super) open_dropdown: Option<RemoteSessionDropdown>,
    pub(super) dropdown: SearchableDropdown,
    pub(crate) creating: bool,
    pub(crate) suggestions: Vec<String>,
    pub(super) suggestion_navigation: Option<RemoteSessionSuggestionNavigation>,
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
            forward_agent: false,
            field: RemoteSessionField::Target,
            sessions: Vec::new(),
            selected: 0,
            loading: false,
            error: None,
            profiles: Vec::new(),
            selected_profile: 0,
            profiles_loading: false,
            profile_error: None,
            templates: vec![RemoteSessionTemplate::SinglePane],
            selected_template: 0,
            open_dropdown: None,
            dropdown: SearchableDropdown::default(),
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

impl RemoteSessionPicker {
    pub(super) fn invalidate_results(&mut self) {
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
        self.close_dropdown();
        self.creating = false;
        self.move_unavailable_focus_to_cancel();
    }

    /// The controls in tab order.
    ///
    /// Keep-alive is only reachable while Zosh is the protocol: it holds a
    /// Mosh link open. Agent forwarding follows the protocol choice: OpenSSH
    /// provides it natively, while Zosh uses its authenticated pane bridge.
    pub(super) fn field_order(&self) -> Vec<RemoteSessionField> {
        let mut order = vec![
            RemoteSessionField::Target,
            RemoteSessionField::Port,
            RemoteSessionField::Protocol,
            RemoteSessionField::ForwardAgent,
        ];
        if self.transport.is_zosh() {
            order.push(RemoteSessionField::KeepAlive);
        }
        order.extend([
            RemoteSessionField::Profile,
            RemoteSessionField::Template,
            RemoteSessionField::List,
            RemoteSessionField::Cancel,
        ]);
        if self.can_load() {
            order.push(RemoteSessionField::Load);
        }
        if self.can_create_action() {
            order.push(RemoteSessionField::Create);
        }
        if self.can_attach() {
            order.push(RemoteSessionField::Attach);
        }
        order
    }

    pub(super) fn cycle_field(&mut self, reverse: bool) {
        let order = self.field_order();
        if !order.contains(&self.field) {
            self.field = RemoteSessionField::Cancel;
        }
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

    /// Keep the manual focus state on a control that is still rendered and
    /// actionable. A load, attach, or profile-discovery transition can make
    /// an action disappear from the tab order while it is focused.
    pub(super) fn move_unavailable_focus_to_cancel(&mut self) {
        if !self.field_order().contains(&self.field) {
            self.field = RemoteSessionField::Cancel;
        }
    }

    /// Switches between carrying the panes over SSH and over Zosh.
    ///
    /// Changing the protocol invalidates nothing that was loaded: the session
    /// list comes from the control connection, which is SSH either way.
    pub(super) fn toggle_transport(&mut self) {
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
                forward_agent: self.forward_agent,
            }
        };
        if !self.transport.is_zosh() && self.field == RemoteSessionField::KeepAlive {
            self.field = RemoteSessionField::Protocol;
        }
        self.move_unavailable_focus_to_cancel();
    }

    pub(super) fn reset_suggestion_navigation(&mut self) {
        self.suggestion_navigation = None;
        self.suggestion_scroll
            .scroll_to_item(0, ScrollStrategy::Top);
    }

    pub(super) fn visible_suggestions(&self) -> Vec<String> {
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

    pub(super) fn enter_action(&self) -> RemoteSessionEnterAction {
        match self.field {
            RemoteSessionField::Cancel => return RemoteSessionEnterAction::Cancel,
            RemoteSessionField::Load => {
                return if self.can_load() {
                    RemoteSessionEnterAction::Load
                } else {
                    RemoteSessionEnterAction::Ignore
                };
            }
            RemoteSessionField::Create => {
                return if self.can_create_action() {
                    RemoteSessionEnterAction::Create
                } else {
                    RemoteSessionEnterAction::Ignore
                };
            }
            RemoteSessionField::Attach => {
                return if self.can_attach() {
                    RemoteSessionEnterAction::Attach(self.selected.min(self.sessions.len() - 1))
                } else {
                    RemoteSessionEnterAction::Ignore
                };
            }
            _ => {}
        }
        if self.loading || self.creating || self.profiles_loading {
            return RemoteSessionEnterAction::Ignore;
        }
        match self.sessions.len() {
            0 => RemoteSessionEnterAction::Load,
            count => RemoteSessionEnterAction::Attach(self.selected.min(count - 1)),
        }
    }

    pub(super) fn shortcut_action(&self, event: &KeyDownEvent) -> Option<RemoteSessionEnterAction> {
        if event.keystroke.key != "enter" {
            return None;
        }
        let modifiers = event.keystroke.modifiers;
        if modifiers.number_of_modifiers() != 1 {
            return None;
        }
        if modifiers.secondary() {
            return Some(if self.can_create_action() {
                RemoteSessionEnterAction::Create
            } else {
                RemoteSessionEnterAction::Ignore
            });
        }
        if modifiers.alt {
            return Some(if self.can_attach() {
                RemoteSessionEnterAction::Attach(self.selected.min(self.sessions.len() - 1))
            } else {
                RemoteSessionEnterAction::Ignore
            });
        }
        None
    }

    pub(super) fn can_load(&self) -> bool {
        !self.loading
    }

    pub(super) fn can_attach(&self) -> bool {
        !self.loading && !self.sessions.is_empty()
    }

    pub(super) fn can_create_action(&self) -> bool {
        !self.loading && !self.creating && !self.profiles_loading && self.can_create()
    }

    pub(super) fn can_create(&self) -> bool {
        !self.profiles.is_empty()
            && self.selected_profile < self.profiles.len()
            && self.selected_template < self.templates.len()
    }

    pub(super) fn navigate_suggestions(&mut self, reverse: bool) -> bool {
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

    pub(super) fn remote_dropdown_options(
        &self,
        dropdown: RemoteSessionDropdown,
    ) -> (Arc<[String]>, usize) {
        match dropdown {
            RemoteSessionDropdown::Profile => (
                self.profiles.clone().into(),
                self.selected_profile
                    .min(self.profiles.len().saturating_sub(1)),
            ),
            RemoteSessionDropdown::Template => (
                self.templates
                    .iter()
                    .map(RemoteSessionTemplate::label)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
                    .into(),
                self.selected_template
                    .min(self.templates.len().saturating_sub(1)),
            ),
        }
    }

    pub(super) fn open_dropdown(
        &mut self,
        dropdown: RemoteSessionDropdown,
        anchor: Point<Pixels>,
    ) -> bool {
        if dropdown == RemoteSessionDropdown::Profile
            && (self.profiles_loading || self.profiles.is_empty())
        {
            return false;
        }
        let (options, selected) = self.remote_dropdown_options(dropdown);
        if !self.dropdown.open(options, selected, anchor) {
            return false;
        }
        self.open_dropdown = Some(dropdown);
        self.field = match dropdown {
            RemoteSessionDropdown::Profile => RemoteSessionField::Profile,
            RemoteSessionDropdown::Template => RemoteSessionField::Template,
        };
        true
    }

    pub(super) fn close_dropdown(&mut self) -> bool {
        let was_open = self.open_dropdown.take().is_some();
        if was_open {
            self.dropdown.close();
        }
        was_open
    }

    pub(super) fn commit_dropdown(&mut self, value: String) -> bool {
        let Some(dropdown) = self.open_dropdown else {
            return false;
        };
        let option_index = match dropdown {
            RemoteSessionDropdown::Profile => {
                self.profiles.iter().position(|profile| profile == &value)
            }
            RemoteSessionDropdown::Template => self
                .templates
                .iter()
                .position(|template| template.label() == value),
        };
        let Some(option_index) = option_index else {
            return false;
        };
        if !self.dropdown.query.is_empty() && !self.dropdown.rows.contains(&option_index) {
            return false;
        }
        match dropdown {
            RemoteSessionDropdown::Profile => self.selected_profile = option_index,
            RemoteSessionDropdown::Template => self.selected_template = option_index,
        }
        self.close_dropdown();
        true
    }
}

pub(super) fn sorted_names(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut values = values.into_iter().collect::<Vec<_>>();
    values.sort_unstable_by_key(|value| value.to_ascii_lowercase());
    values
}

pub(super) fn remote_session_templates(config: &Config) -> Vec<RemoteSessionTemplate> {
    std::iter::once(RemoteSessionTemplate::SinglePane)
        .chain(
            sorted_names(config.pane_split_templates.keys().cloned())
                .into_iter()
                .map(RemoteSessionTemplate::Configured),
        )
        .collect()
}
