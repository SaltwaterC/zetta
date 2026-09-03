//! A pane's stack: the command terminals that share its layout region.
//!
//! One entry is the base terminal the pane was created with; the rest are
//! stacked commands. Selection is what the pane shows, and it has to stay
//! valid as entries are pushed, removed, and exit — which is what
//! `repair_selection` and `select_after_base_exit` are for.

use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum PaneStackSelection {
    #[default]
    Base,
    Stacked(u64),
}

/// The command entries associated with one interactive terminal pane.
pub(crate) struct PaneStack {
    pub(crate) entries: Vec<StackedPane>,
    pub(crate) selected: PaneStackSelection,
}

impl Default for PaneStack {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            selected: PaneStackSelection::Base,
        }
    }
}

impl PaneStack {
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn selected_is_base(&self) -> bool {
        matches!(self.selected, PaneStackSelection::Base)
    }

    pub(crate) fn select_after_base_exit(&mut self) {
        let selected_is_valid = match self.selected {
            PaneStackSelection::Base => false,
            PaneStackSelection::Stacked(id) => self.entries.iter().any(|entry| entry.id == id),
        };
        if !selected_is_valid {
            self.selected = self
                .entries
                .first()
                .map(|entry| PaneStackSelection::Stacked(entry.id))
                .unwrap_or(PaneStackSelection::Base);
        }
    }

    pub(crate) fn selected_entry(&self) -> Option<&StackedPane> {
        let PaneStackSelection::Stacked(id) = self.selected else {
            return None;
        };
        self.entries.iter().find(|entry| entry.id == id)
    }

    pub(crate) fn selected_view(
        &self,
        base: Option<&Entity<TerminalView>>,
    ) -> Option<Entity<TerminalView>> {
        self.selected_entry()
            .and_then(|entry| entry.view.clone())
            .or_else(|| self.selected_is_base().then(|| base.cloned()).flatten())
    }

    pub(crate) fn selected_terminal(
        &self,
        base: Option<&Entity<Terminal>>,
    ) -> Option<Entity<Terminal>> {
        self.selected_entry()
            .and_then(|entry| entry.terminal.clone())
            .or_else(|| self.selected_is_base().then(|| base.cloned()).flatten())
    }

    pub(crate) fn select(&mut self, selection: PaneStackSelection) -> bool {
        let valid = match selection {
            PaneStackSelection::Base => true,
            PaneStackSelection::Stacked(id) => self.entries.iter().any(|entry| entry.id == id),
        };
        if valid {
            self.selected = selection;
        }
        valid
    }

    pub(crate) fn cycle(&mut self, forward: bool) -> Option<PaneStackSelection> {
        if self.entries.is_empty() {
            self.selected = PaneStackSelection::Base;
            return None;
        }

        let current = match self.selected {
            PaneStackSelection::Base => 0,
            PaneStackSelection::Stacked(id) => self
                .entries
                .iter()
                .position(|entry| entry.id == id)
                .map(|index| index + 1)
                .unwrap_or(0),
        };
        let count = self.entries.len() + 1;
        let next = if forward {
            (current + 1) % count
        } else {
            current.checked_sub(1).unwrap_or(count - 1)
        };
        self.selected = if next == 0 {
            PaneStackSelection::Base
        } else {
            PaneStackSelection::Stacked(self.entries[next - 1].id)
        };
        Some(self.selected)
    }

    pub(crate) fn cycle_without_base(&mut self, forward: bool) -> Option<PaneStackSelection> {
        if self.entries.is_empty() {
            self.selected = PaneStackSelection::Base;
            return None;
        }

        let current = match self.selected {
            PaneStackSelection::Stacked(id) => self.entries.iter().position(|entry| entry.id == id),
            PaneStackSelection::Base => None,
        };
        let next = match current {
            Some(index) if forward => (index + 1) % self.entries.len(),
            Some(index) => index.checked_sub(1).unwrap_or(self.entries.len() - 1),
            None if forward => 0,
            None => self.entries.len() - 1,
        };

        self.selected = PaneStackSelection::Stacked(self.entries[next].id);
        Some(self.selected)
    }

    pub(crate) fn push(&mut self, entry: StackedPane) -> bool {
        if self.entries.len() >= MAX_PANES_PER_TAB.saturating_sub(1) {
            return false;
        }
        let id = entry.id;
        self.entries.push(entry);
        self.selected = PaneStackSelection::Stacked(id);
        true
    }

    pub(crate) fn remove(&mut self, id: u64) -> Option<StackedPane> {
        let index = self.entries.iter().position(|entry| entry.id == id)?;
        let was_selected = self.selected == PaneStackSelection::Stacked(id);
        let entry = self.entries.remove(index);
        if was_selected {
            self.selected = if let Some(entry) = self.entries.get(index) {
                PaneStackSelection::Stacked(entry.id)
            } else if let Some(entry) = self.entries.get(index.saturating_sub(1)) {
                PaneStackSelection::Stacked(entry.id)
            } else {
                PaneStackSelection::Base
            };
        } else if let PaneStackSelection::Stacked(selected) = self.selected
            && !self.entries.iter().any(|entry| entry.id == selected)
        {
            self.selected = PaneStackSelection::Base;
        }
        Some(entry)
    }

    pub(crate) fn repair_selection(&mut self) {
        if let PaneStackSelection::Stacked(id) = self.selected
            && !self.entries.iter().any(|entry| entry.id == id)
        {
            self.selected = self
                .entries
                .last()
                .map(|entry| PaneStackSelection::Stacked(entry.id))
                .unwrap_or(PaneStackSelection::Base);
        }
    }
}

/// Serialized so a detached session can carry its stacked commands: the name
/// lives with the variants rather than in a separate mapping, so the two cannot
/// drift apart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StackedPaneState {
    #[default]
    Starting,
    Running,
    Completed,
    Failed,
}

pub(crate) struct StackedPane {
    pub(crate) id: u64,
    pub(crate) routing_id: u64,
    pub(crate) command: String,
    pub(crate) profile: Profile,
    pub(crate) theme_override: Option<String>,
    pub(crate) terminal: Option<Entity<Terminal>>,
    pub(crate) view: Option<Entity<TerminalView>>,
    pub(crate) state: StackedPaneState,
    pub(crate) exit_code: Option<i32>,
    pub(crate) error: Option<String>,
    pub(crate) working_directory: Option<PathBuf>,
    pub(crate) wsl_directory: Option<String>,
}

impl StackedPane {
    pub(crate) fn new(
        id: u64,
        command: String,
        profile: Profile,
        working_directory: Option<PathBuf>,
        wsl_directory: Option<String>,
    ) -> Self {
        Self {
            id,
            routing_id: id,
            command,
            profile,
            theme_override: None,
            terminal: None,
            view: None,
            state: StackedPaneState::Starting,
            exit_code: None,
            error: None,
            working_directory,
            wsl_directory,
        }
    }
}
