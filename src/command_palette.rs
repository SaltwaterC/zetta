use gpui::Action;

pub struct PaletteCommand {
    pub name: String,
    pub shortcut: Option<String>,
    pub action: Box<dyn Action>,
}

pub struct CommandPalette {
    pub query: String,
    pub cursor: usize,
    pub select_all: bool,
    pub selected: usize,
    pub commands: Vec<PaletteCommand>,
    normalized_names: Vec<String>,
    matches: Vec<usize>,
}

impl CommandPalette {
    pub fn new(mut commands: Vec<PaletteCommand>) -> Self {
        commands.sort_by(|a, b| a.name.cmp(&b.name));
        commands.dedup_by(|a, b| a.name == b.name);
        let normalized_names = commands
            .iter()
            .map(|command| command.name.to_lowercase())
            .collect();
        let matches = (0..commands.len()).collect();
        Self {
            select_all: false,
            query: String::new(),
            cursor: 0,
            selected: 0,
            commands,
            normalized_names,
            matches,
        }
    }

    pub fn matches(&self) -> &[usize] {
        &self.matches
    }

    pub fn refresh_matches(&mut self) {
        let query = self.query.trim().to_lowercase();
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
mod tests {
    use super::*;
    gpui::actions!(command_palette_test, [First, Second]);

    #[test]
    fn humanizes_action_names() {
        assert_eq!(humanize_action_name("zetta::NewTab"), "zetta: new tab");
        assert_eq!(
            humanize_action_name("editor::OpenURLParser"),
            "editor: open URL parser"
        );
        assert_eq!(
            humanize_action_name("go_to_line::Deploy"),
            "go to line: deploy"
        );
    }

    #[test]
    fn fuzzy_matching_finds_subsequences() {
        assert!(fuzzy_score("terminal: paste trimmed", "paste trim").is_some());
        assert!(fuzzy_score("terminal: paste", "missing").is_none());
    }

    #[test]
    fn matches_are_cached_until_the_query_changes() {
        let mut palette = CommandPalette::new(vec![
            PaletteCommand {
                name: "terminal: paste".into(),
                shortcut: None,
                action: Box::new(First),
            },
            PaletteCommand {
                name: "window: new tab".into(),
                shortcut: None,
                action: Box::new(Second),
            },
        ]);
        assert_eq!(palette.matches(), &[0, 1]);

        palette.query = "paste".into();
        palette.refresh_matches();
        assert_eq!(palette.matches(), &[0]);
        assert_eq!(
            palette.commands[palette.matches()[0]].name,
            "terminal: paste"
        );
    }
}
