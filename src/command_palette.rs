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

pub struct PaletteCommand {
    pub name: String,
    pub shortcut: Option<String>,
    pub action: Box<dyn Action>,
}

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

    pub fn matches(&self) -> &[usize] {
        &self.matches
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
            .filter_map(|(index, name)| fuzzy_score(name, &query).map(|score| (index, score)))
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

fn fuzzy_score(candidate: &str, query: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let mut characters = query.chars();
    let mut wanted = characters.next()?;
    let mut score = 0;
    let mut previous_match = None;
    for (index, character) in candidate.char_indices() {
        if character != wanted {
            continue;
        }
        score += 10;
        if previous_match.is_some_and(|previous| previous + character.len_utf8() == index) {
            score += 8;
        }
        if index == 0
            || candidate[..index]
                .chars()
                .next_back()
                .is_some_and(|previous| matches!(previous, ' ' | ':' | '_' | '-'))
        {
            score += 5;
        }
        previous_match = Some(index);
        match characters.next() {
            Some(next) => wanted = next,
            None => return Some(score - candidate.len() as i32 / 8),
        }
    }
    None
}

#[cfg(test)]
#[path = "tests/command_palette.rs"]
mod tests;
