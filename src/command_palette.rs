use std::path::{Path, PathBuf};

use gpui::{Action, ScrollStrategy, UniformListScrollHandle};

use crate::{
    OpenProject, ToggleAutoBackgroundTab, ToggleTabSharing, project::project_display_name,
    text_edit::TextField,
};

/// The two session-lifecycle actions are mode-specific. Keep explicit user
/// keybindings usable, but only expose the action that can operate in this
/// launch mode through built-in UI surfaces.
pub(crate) fn action_available_in_launch_mode(action_name: &str, no_mux: bool) -> bool {
    let unavailable_action = if no_mux {
        ToggleTabSharing.name()
    } else {
        ToggleAutoBackgroundTab.name()
    };
    action_name != unavailable_action
}

/// What [`CommandPalette::apply_key`] did with a key, and what is left for the
/// surface that owns the list.
#[derive(Debug, PartialEq, Eq)]
pub enum PaletteKey {
    /// Not a key the list answers to. The caller carries on with its own
    /// handling — `escape`, and whatever else that surface binds.
    Ignored,
    /// The selection or the query changed. Nothing else to do but redraw.
    Redraw,
    /// `enter` on this command index. The caller runs it, because what running
    /// one means differs per surface.
    Accept(usize),
}

pub struct PaletteCommand {
    pub name: String,
    /// The keys the effective keymap binds the action to, drawn as a key
    /// binding beside the command.
    pub shortcut: Option<std::rc::Rc<[gpui::KeybindingKeystroke]>>,
    pub action: Box<dyn Action>,
}

/// How far PageUp and PageDown move the selection: about a list's height of
/// rows at the palette's maximum height.
const PAGE_ROWS: usize = 8;

pub struct CommandPalette {
    pub query: TextField,
    pub selected: usize,
    pub commands: Vec<PaletteCommand>,
    pub scroll: UniformListScrollHandle,
    normalized_names: Vec<String>,
    matches: Vec<usize>,
}

impl CommandPalette {
    pub fn new(mut commands: Vec<PaletteCommand>) -> Self {
        commands.sort_by(|a, b| a.name.cmp(&b.name));
        commands.dedup_by(|a, b| a.name == b.name);
        Self::from_sorted_commands(commands)
    }

    /// Like [`Self::new`], but `pinned` is kept first instead of being sorted
    /// alphabetically with the rest — for a "reset to default" style entry
    /// that should stay at the top of the list regardless of its name.
    pub fn with_pinned_first(pinned: PaletteCommand, mut rest: Vec<PaletteCommand>) -> Self {
        rest.retain(|command| command.name != pinned.name);
        rest.sort_by(|a, b| a.name.cmp(&b.name));
        rest.dedup_by(|a, b| a.name == b.name);
        let mut commands = Vec::with_capacity(rest.len() + 1);
        commands.push(pinned);
        commands.extend(rest);
        Self::from_sorted_commands(commands)
    }

    fn from_sorted_commands(commands: Vec<PaletteCommand>) -> Self {
        let normalized_names = commands
            .iter()
            .map(|command| command.name.to_lowercase())
            .collect();
        let matches = (0..commands.len()).collect();
        Self {
            query: TextField::default(),
            selected: 0,
            commands,
            scroll: UniformListScrollHandle::new(),
            normalized_names,
            matches,
        }
    }

    /// What changing the query owes the list, whichever way it changed —
    /// typing, or a cut or paste: the matches are rebuilt from it and the
    /// selection returns to the first match rather than pointing into the old
    /// list. The palette's clipboard path used to rebuild the matches but keep
    /// the old selection.
    pub fn query_edited(&mut self) {
        self.refresh_matches();
        self.selected = 0;
        self.scroll_to_selected();
    }

    pub fn matches(&self) -> &[usize] {
        &self.matches
    }

    /// Applies one key to the list and its query, and reports what the caller
    /// still has to do.
    ///
    /// The command palette and the theme picker are both a [`CommandPalette`]
    /// behind a filtered list, and their key handlers were the same code with
    /// the receiver renamed. What differs is only what `escape` dismisses and
    /// what `enter` runs, so both stay at the call site: `escape` is not a key
    /// this answers to, and `enter` comes back as [`PaletteKey::Accept`]
    /// carrying the command rather than running it.
    pub fn apply_key(&mut self, keystroke: &gpui::Keystroke) -> PaletteKey {
        match keystroke.key.as_str() {
            "up" => {
                self.selected = self.selected.saturating_sub(1);
                self.scroll_to_selected();
                PaletteKey::Redraw
            }
            "down" => {
                self.selected = (self.selected + 1).min(self.matches.len().saturating_sub(1));
                self.scroll_to_selected();
                PaletteKey::Redraw
            }
            "pageup" => {
                self.selected = self.selected.saturating_sub(PAGE_ROWS);
                self.scroll_to_selected();
                PaletteKey::Redraw
            }
            "pagedown" => {
                self.selected =
                    (self.selected + PAGE_ROWS).min(self.matches.len().saturating_sub(1));
                self.scroll_to_selected();
                PaletteKey::Redraw
            }
            "enter" => match self.matches.get(self.selected).copied() {
                Some(command) => PaletteKey::Accept(command),
                // An empty list has nothing to run, and the overlay stays open
                // rather than dismissing on a keystroke that did nothing.
                None => PaletteKey::Redraw,
            },
            _ => match crate::text_edit::apply_text_field_key(&mut self.query, keystroke) {
                crate::text_edit::TextFieldEdit::Ignored => PaletteKey::Ignored,
                crate::text_edit::TextFieldEdit::CursorMoved => PaletteKey::Redraw,
                crate::text_edit::TextFieldEdit::Edited => {
                    self.query_edited();
                    PaletteKey::Redraw
                }
            },
        }
    }

    pub fn scroll_to_selected(&self) {
        if !self.matches.is_empty() {
            self.scroll
                .scroll_to_item(self.selected, ScrollStrategy::Nearest);
        }
    }

    pub fn refresh_matches(&mut self) {
        let query = self.query.text.trim().to_lowercase();
        let mut matches = self
            .normalized_names
            .iter()
            .enumerate()
            .filter_map(|(index, name)| {
                crate::fuzzy_match::score(name, &query).map(|score| (index, score))
            })
            .collect::<Vec<_>>();
        matches.sort_by(|(left_index, left_score), (right_index, right_score)| {
            right_score.cmp(left_score).then_with(|| {
                self.commands[*left_index]
                    .name
                    .cmp(&self.commands[*right_index].name)
            })
        });
        self.matches = matches.into_iter().map(|(index, _)| index).collect();
        self.selected = self.selected.min(self.matches.len().saturating_sub(1));
    }
}

/// One entry per registered project root, opening it the way the Projects
/// page's Open button does. The label carries the directory name for searching
/// and the full path for disambiguation: [`CommandPalette::new`] drops
/// duplicate names, and two registered projects can share a directory name.
pub fn project_palette_commands(roots: &[PathBuf]) -> Vec<PaletteCommand> {
    roots
        .iter()
        .map(|root| PaletteCommand {
            name: project_palette_command_name(root),
            // No keybinding can name a project: `OpenProject` carries a
            // machine-specific path and is not keymap-bindable.
            shortcut: None,
            action: Box::new(OpenProject { root: root.clone() }),
        })
        .collect()
}

fn project_palette_command_name(root: &Path) -> String {
    format!(
        "zetta: open project: {} ({})",
        project_display_name(root),
        root.display()
    )
}

pub fn humanize_action_name(name: &str) -> String {
    let chars = name.chars().collect::<Vec<_>>();
    let mut result = String::with_capacity(name.len() + 8);
    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];
        if character == ':' {
            if result.ends_with(':') {
                result.push(' ');
            } else {
                result.push(':');
            }
            index += 1;
        } else if character == '_' {
            result.push(' ');
            index += 1;
        } else if character.is_uppercase() {
            let start = index;
            index += 1;
            while chars.get(index).is_some_and(|next| next.is_uppercase()) {
                index += 1;
            }
            let run = &chars[start..index];
            let split_last =
                run.len() > 1 && chars.get(index).is_some_and(|next| next.is_lowercase());
            let acronym_end = if split_last { run.len() - 1 } else { run.len() };
            if !result.ends_with(' ') {
                result.push(' ');
            }
            if acronym_end > 0 {
                result.extend(&run[..acronym_end]);
            }
            if split_last {
                result.push(' ');
                result.extend(run[acronym_end].to_lowercase());
            } else if run.len() == 1 {
                result.pop();
                result.extend(character.to_lowercase());
            }
        } else {
            result.push(character);
            index += 1;
        }
    }
    result
}

#[cfg(test)]
#[path = "tests/command_palette.rs"]
mod tests;
