use super::*;

const MAX_FILESYSTEM_COMPLETION_ENTRIES: usize = 16_384;
const MAX_FILESYSTEM_COMPLETION_CANDIDATES: usize = 1_024;
const MAX_EXECUTABLE_SCAN_ENTRIES: usize = 65_536;
const MAX_EXECUTABLE_CATALOG_ENTRIES: usize = 16_384;
const MAX_CATALOG_COMPLETION_CANDIDATES: usize = 1_024;
const MAX_MULTI_COMMAND_TEMPLATE_BYTES: usize = 65_536;

#[derive(Clone, Default)]
pub(crate) struct CompletionCancellation(Arc<AtomicBool>);

impl CompletionCancellation {
    fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Clone)]
pub(crate) struct CompletionEntry {
    display: SharedString,
    normalized: SharedString,
}

#[derive(Clone, Default)]
pub(crate) struct CompletionCatalog {
    commands: Arc<[CompletionEntry]>,
    ssh_hosts: Arc<[CompletionEntry]>,
}

impl CompletionCatalog {
    fn new(commands: Vec<String>, ssh_hosts: Vec<String>) -> Self {
        let entries = |values: Vec<String>| {
            values
                .into_iter()
                .map(|display| CompletionEntry {
                    normalized: SharedString::from(display.to_lowercase()),
                    display: SharedString::from(display),
                })
                .collect::<Vec<_>>()
                .into()
        };
        Self {
            commands: entries(commands),
            ssh_hosts: entries(ssh_hosts),
        }
    }
}

pub(crate) struct CompletionRequest {
    pub(crate) generation: u64,
    pub(crate) query: String,
    pub(crate) cursor: usize,
    pub(crate) start: usize,
    pub(crate) reverse: bool,
    pub(crate) source: CompletionSource,
    pub(crate) cancellation: CompletionCancellation,
}

impl CompletionRequest {
    pub(crate) fn take_source(&mut self) -> CompletionSource {
        std::mem::replace(&mut self.source, CompletionSource::Ready(Vec::new()))
    }
}

pub(crate) enum CompletionSource {
    Ready(Vec<SharedString>),
    Filesystem {
        prefix: String,
        home: PathBuf,
        working_directory: PathBuf,
    },
}

pub(crate) enum MultiCommandExecution<T> {
    Single(T),
    Tiled(Vec<T>),
}

impl<T> MultiCommandExecution<T> {
    pub(crate) fn new(mut commands: Vec<T>) -> Self {
        assert!(
            !commands.is_empty(),
            "a multi-command execution must contain a command"
        );
        if commands.len() == 1 {
            Self::Single(
                commands
                    .pop()
                    .expect("a single-command execution contains one command"),
            )
        } else {
            Self::Tiled(commands)
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct MultiCommandExpansion {
    pub(crate) command: String,
    pub(crate) label: String,
}

pub(crate) struct MultiCommandPrompt {
    pub(crate) query: String,
    pub(crate) cursor: usize,
    pub(crate) select_all: bool,
    pub(crate) error: Option<String>,
    pub(crate) completion_candidates: Vec<SharedString>,
    pub(crate) completion_selected: Option<usize>,
    completion_start: usize,
    completion_end: usize,
    completion_add_space: bool,
    completion_generation: u64,
    pub(crate) completion_loading: bool,
    completion_task: Option<Task<()>>,
    completion_cancellation: Option<CompletionCancellation>,
    catalog: CompletionCatalog,
    home: PathBuf,
    query_generation: u64,
    rendered_query_generation: u64,
    rendered_query_before: SharedString,
    rendered_query_after: SharedString,
}

impl MultiCommandPrompt {
    pub(crate) fn new(catalog: CompletionCatalog) -> Self {
        let home = util::paths::home_dir().clone();
        Self {
            query: String::new(),
            cursor: 0,
            select_all: false,
            error: None,
            completion_candidates: Vec::new(),
            completion_selected: None,
            completion_start: 0,
            completion_end: 0,
            completion_add_space: false,
            completion_generation: 0,
            completion_loading: false,
            completion_task: None,
            completion_cancellation: None,
            catalog,
            home,
            query_generation: 0,
            rendered_query_generation: u64::MAX,
            rendered_query_before: SharedString::default(),
            rendered_query_after: SharedString::default(),
        }
    }

    pub(crate) fn set_catalog(&mut self, catalog: CompletionCatalog) {
        self.catalog = catalog;
    }

    pub(crate) fn clear_completion(&mut self) {
        if let Some(cancellation) = self.completion_cancellation.take() {
            cancellation.cancel();
        }
        cancel_completion_task(&mut self.completion_task);
        self.completion_generation = self.completion_generation.wrapping_add(1);
        self.completion_loading = false;
        self.completion_candidates.clear();
        self.completion_selected = None;
    }

    pub(crate) fn accept_completion(&mut self) -> bool {
        if self.completion_candidates.is_empty() {
            return false;
        }
        let completed_directory =
            self.query[self.completion_start..self.completion_end].ends_with(['/', '\\']);
        let followed_by_whitespace = self.query[self.completion_end..]
            .chars()
            .next()
            .is_some_and(char::is_whitespace);
        let add_space =
            self.completion_add_space && !completed_directory && !followed_by_whitespace;
        self.clear_completion();
        if add_space {
            self.query.insert(self.completion_end, ' ');
            self.completion_end += 1;
            self.cursor = self.completion_end;
            self.mark_query_changed();
        }
        true
    }

    pub(crate) fn cycle_existing_completion(&mut self, reverse: bool) -> bool {
        if self.completion_candidates.is_empty() {
            return false;
        }
        self.navigate_completion(reverse);
        true
    }

    pub(crate) fn begin_completion_request(
        &mut self,
        working_directory: PathBuf,
        reverse: bool,
    ) -> CompletionRequest {
        self.completion_generation = self.completion_generation.wrapping_add(1);
        self.completion_loading = true;
        let generation = self.completion_generation;
        let cancellation = CompletionCancellation::default();
        let query = self.query.clone();
        let cursor = self.cursor;
        let (start, kind) = completion_context(&query, cursor);
        let source =
            match kind {
                CompletionKind::Command { prefix } => CompletionSource::Ready(
                    prefix_matches_entries(&self.catalog.commands, &prefix, cfg!(windows)),
                ),
                CompletionKind::Ssh { user, host_prefix } => {
                    let hosts = prefix_matches_entries(&self.catalog.ssh_hosts, &host_prefix, true)
                        .into_iter()
                        .map(|host| match user.as_deref() {
                            Some(user) => SharedString::from(format!("{user}@{host}")),
                            None => host,
                        })
                        .collect();
                    CompletionSource::Ready(hosts)
                }
                CompletionKind::Filesystem { prefix } => CompletionSource::Filesystem {
                    prefix,
                    home: self.home.clone(),
                    working_directory,
                },
            };
        CompletionRequest {
            generation,
            query,
            cursor,
            start,
            reverse,
            source,
            cancellation,
        }
    }

    pub(crate) fn apply_completion_result(
        &mut self,
        request: &CompletionRequest,
        candidates: Vec<SharedString>,
    ) -> bool {
        if !completion_request_is_current(Some(self), request) {
            return false;
        }
        self.completion_loading = false;
        if candidates.is_empty() {
            return true;
        }
        self.completion_start = request.start;
        self.completion_end = request.cursor;
        self.completion_add_space = !inside_double_brace_group(&self.query, request.start);
        self.completion_candidates = candidates;

        let current = &self.query[self.completion_start..self.completion_end];
        let common_prefix = longest_common_prefix(&self.completion_candidates).to_owned();
        if common_prefix.len() > current.len() {
            self.replace_completion(&common_prefix);
            if self.completion_candidates.len() == 1 && common_prefix.ends_with(['/', '\\']) {
                self.clear_completion();
            }
            return true;
        }
        self.navigate_completion(request.reverse);
        true
    }

    pub(crate) fn navigate_completion(&mut self, reverse: bool) {
        if self.completion_candidates.is_empty() {
            return;
        }
        let count = self.completion_candidates.len();
        let selected = match (self.completion_selected, reverse) {
            (Some(selected), false) => (selected + 1) % count,
            (Some(selected), true) => (selected + count - 1) % count,
            (None, false) => 0,
            (None, true) => count - 1,
        };
        self.select_completion(selected);
    }

    pub(crate) fn select_completion(&mut self, selected: usize) {
        let Some(replacement) = self.completion_candidates.get(selected).cloned() else {
            return;
        };
        self.completion_selected = Some(selected);
        self.replace_completion(replacement.as_ref());
    }

    pub(crate) fn set_completion_task(
        &mut self,
        task: Task<()>,
        cancellation: CompletionCancellation,
    ) {
        self.completion_task = Some(task);
        self.completion_cancellation = Some(cancellation);
    }

    pub(crate) fn mark_query_changed(&mut self) {
        self.query_generation = self.query_generation.wrapping_add(1);
    }

    pub(crate) fn delete_previous_word(&mut self) {
        if self.select_all {
            self.query.clear();
            self.cursor = 0;
        } else {
            let end = self.cursor.min(self.query.len());
            let mut start = end;
            while start > 0 {
                let previous = previous_char_boundary(&self.query, start);
                let character = self.query[previous..start]
                    .chars()
                    .next()
                    .expect("character boundaries contain one character");
                if !character.is_whitespace() {
                    break;
                }
                start = previous;
            }
            while start > 0 {
                let previous = previous_char_boundary(&self.query, start);
                let character = self.query[previous..start]
                    .chars()
                    .next()
                    .expect("character boundaries contain one character");
                if character.is_whitespace() || character == ',' {
                    break;
                }
                start = previous;
            }
            self.query.replace_range(start..end, "");
            self.cursor = start;
        }
        self.select_all = false;
        self.error = None;
        self.mark_query_changed();
    }

    pub(crate) fn rendered_query_parts(&mut self) -> (SharedString, SharedString) {
        if self.rendered_query_generation != self.query_generation {
            let cursor = self.cursor.min(self.query.len());
            let (before, after) = self.query.split_at(cursor);
            self.rendered_query_before = SharedString::from(before.to_owned());
            self.rendered_query_after = SharedString::from(after.to_owned());
            self.rendered_query_generation = self.query_generation;
        }
        (
            self.rendered_query_before.clone(),
            self.rendered_query_after.clone(),
        )
    }

    fn replace_completion(&mut self, replacement: &str) {
        self.query
            .replace_range(self.completion_start..self.completion_end, replacement);
        self.completion_end = self.completion_start + replacement.len();
        self.cursor = self.completion_end;
        self.select_all = false;
        self.error = None;
        self.mark_query_changed();
    }
}

fn cancel_completion_task<T>(task: &mut Option<T>) {
    task.take();
}

fn inside_double_brace_group(text: &str, cursor: usize) -> bool {
    let bytes = &text.as_bytes()[..cursor];
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if escaped {
            escaped = false;
            index += 1;
            continue;
        }
        if byte == b'\\' && quote != Some(b'\'') {
            escaped = true;
            index += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            if quote == Some(byte) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(byte);
            }
            index += 1;
            continue;
        }
        if quote.is_none() && index + 1 < bytes.len() {
            match &bytes[index..index + 2] {
                b"{{" => {
                    depth += 1;
                    index += 2;
                    continue;
                }
                b"}}" => {
                    depth = depth.saturating_sub(1);
                    index += 2;
                    continue;
                }
                _ => {}
            }
        }
        index += 1;
    }
    depth > 0
}

enum CompletionKind {
    Command {
        prefix: String,
    },
    Ssh {
        user: Option<String>,
        host_prefix: String,
    },
    Filesystem {
        prefix: String,
    },
}

fn completion_context(query: &str, cursor: usize) -> (usize, CompletionKind) {
    let before_cursor = &query[..cursor];
    let start = before_cursor
        .char_indices()
        .rev()
        .find_map(|(index, character)| {
            (character.is_whitespace() || matches!(character, '{' | ',' | '\'' | '"'))
                .then_some(index + character.len_utf8())
        })
        .unwrap_or(0);
    let prefix = query[start..cursor].to_owned();
    let command_position = query[..start].trim().is_empty();
    let command = before_cursor
        .split_whitespace()
        .next()
        .and_then(|word| Path::new(word).file_name())
        .and_then(|word| word.to_str());

    let kind = if command_position {
        CompletionKind::Command { prefix }
    } else if command.is_some_and(|command| command.eq_ignore_ascii_case("ssh"))
        && !prefix.starts_with('-')
    {
        let (user, host_prefix) = prefix
            .rsplit_once('@')
            .map(|(user, host)| (Some(user.to_owned()), host.to_owned()))
            .unwrap_or((None, prefix));
        CompletionKind::Ssh { user, host_prefix }
    } else {
        CompletionKind::Filesystem { prefix }
    };
    (start, kind)
}

fn prefix_matches_entries(
    values: &[CompletionEntry],
    prefix: &str,
    case_insensitive: bool,
) -> Vec<SharedString> {
    prefix_matches_entries_with_limit(
        values,
        prefix,
        case_insensitive,
        MAX_CATALOG_COMPLETION_CANDIDATES,
    )
}

fn prefix_matches_entries_with_limit(
    values: &[CompletionEntry],
    prefix: &str,
    case_insensitive: bool,
    limit: usize,
) -> Vec<SharedString> {
    let normalized_prefix = case_insensitive.then(|| prefix.to_lowercase());
    values
        .iter()
        .filter(|value| match normalized_prefix.as_deref() {
            Some(prefix) => value.normalized.starts_with(prefix),
            None => value.display.starts_with(prefix),
        })
        .take(limit)
        .map(|value| value.display.clone())
        .collect()
}

pub(crate) fn completion_request_is_current(
    prompt: Option<&MultiCommandPrompt>,
    request: &CompletionRequest,
) -> bool {
    prompt.is_some_and(|prompt| {
        prompt.completion_generation == request.generation
            && prompt.query == request.query
            && prompt.cursor == request.cursor
    })
}

fn longest_common_prefix(values: &[SharedString]) -> &str {
    let Some(first) = values.first() else {
        return "";
    };
    let first = first.as_ref();
    let mut end = first.len();
    for value in &values[1..] {
        end = first[..end]
            .char_indices()
            .map(|(index, character)| index + character.len_utf8())
            .take_while(|end| value.get(..*end) == first.get(..*end))
            .last()
            .unwrap_or(0);
    }
    &first[..end]
}

fn ssh_config_hosts(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .ok()
        .map(|contents| parse_ssh_config_hosts(&contents))
        .unwrap_or_default()
}

pub(crate) fn load_completion_catalog(path: Option<OsString>, home: &Path) -> CompletionCatalog {
    static CATALOG: OnceLock<CompletionCatalog> = OnceLock::new();
    cached_completion_catalog(&CATALOG, || {
        CompletionCatalog::new(
            executable_names(path.as_deref()),
            ssh_config_hosts(&home.join(".ssh").join("config")),
        )
    })
}

fn cached_completion_catalog(
    cache: &OnceLock<CompletionCatalog>,
    load: impl FnOnce() -> CompletionCatalog,
) -> CompletionCatalog {
    cache.get_or_init(load).clone()
}

fn parse_ssh_config_hosts(contents: &str) -> Vec<String> {
    let mut hosts = contents
        .lines()
        .filter_map(|line| {
            let line = line.split('#').next()?.trim();
            let mut fields = line.split_whitespace();
            fields
                .next()
                .is_some_and(|keyword| keyword.eq_ignore_ascii_case("host"))
                .then_some(fields)
        })
        .flatten()
        .filter(|host| !host.starts_with('!') && !host.contains(['*', '?']))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    hosts.sort_by_cached_key(|host| host.to_lowercase());
    hosts.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    hosts
}

fn executable_names(path: Option<&std::ffi::OsStr>) -> Vec<String> {
    #[cfg(windows)]
    let executable_extensions = env::var_os("PATHEXT")
        .map(|extensions| {
            extensions
                .to_string_lossy()
                .split(';')
                .map(|extension| extension.to_ascii_lowercase())
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec![".exe".to_owned(), ".cmd".to_owned(), ".bat".to_owned()]);
    let names = path
        .into_iter()
        .flat_map(env::split_paths)
        .filter_map(|directory| fs::read_dir(directory).ok())
        .flatten()
        .take(MAX_EXECUTABLE_SCAN_ENTRIES)
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            if !metadata.is_file() {
                return None;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                if metadata.permissions().mode() & 0o111 == 0 {
                    return None;
                }
            }
            let name = entry.file_name().into_string().ok()?;
            #[cfg(windows)]
            {
                let extension = Path::new(&name)
                    .extension()
                    .map(|extension| format!(".{}", extension.to_string_lossy().to_lowercase()))?;
                if !executable_extensions.contains(&extension) {
                    return None;
                }
                return Path::new(&name)
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(str::to_owned);
            }
            #[cfg(not(windows))]
            Some(name)
        });
    bounded_sorted_unique_candidates(names, MAX_EXECUTABLE_CATALOG_ENTRIES)
}

#[cfg(test)]
pub(crate) fn filesystem_candidates(
    prefix: &str,
    home: &Path,
    working_directory: &Path,
) -> Vec<SharedString> {
    filesystem_candidates_cancellable(
        prefix,
        home,
        working_directory,
        &CompletionCancellation::default(),
    )
}

pub(crate) fn filesystem_candidates_cancellable(
    prefix: &str,
    home: &Path,
    working_directory: &Path,
    cancellation: &CompletionCancellation,
) -> Vec<SharedString> {
    let separator = std::path::MAIN_SEPARATOR;
    let display_parent_end = prefix
        .char_indices()
        .rev()
        .find_map(|(index, character)| matches!(character, '/' | '\\').then_some(index + 1))
        .unwrap_or(0);
    let display_parent = &prefix[..display_parent_end];
    let name_prefix = &prefix[display_parent_end..];
    let expanded = if prefix == "~" || prefix.starts_with("~/") || prefix.starts_with("~\\") {
        home.join(
            prefix
                .trim_start_matches('~')
                .trim_start_matches(['/', '\\']),
        )
    } else {
        working_directory.join(prefix)
    };
    let directory = if prefix.is_empty() {
        working_directory.to_path_buf()
    } else if prefix.ends_with(['/', '\\']) {
        expanded
    } else {
        expanded.parent().unwrap_or(working_directory).to_path_buf()
    };
    let show_hidden = name_prefix.starts_with('.');
    let candidates = fs::read_dir(directory)
        .into_iter()
        .flatten()
        .take(MAX_FILESYSTEM_COMPLETION_ENTRIES)
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let prefix_matches = if cfg!(windows) {
                name.get(..name_prefix.len())
                    .is_some_and(|start| start.eq_ignore_ascii_case(name_prefix))
            } else {
                name.starts_with(name_prefix)
            };
            if !prefix_matches || (!show_hidden && name.starts_with('.')) {
                return None;
            }
            let mut candidate = format!("{display_parent}{name}");
            let file_type = entry.file_type().ok()?;
            let is_directory = if file_type.is_symlink() {
                fs::metadata(entry.path()).ok()?.is_dir()
            } else {
                file_type.is_dir()
            };
            if is_directory {
                candidate.push(separator);
            }
            Some(SharedString::from(candidate))
        });
    bounded_sorted_candidates_cancellable(
        candidates,
        MAX_FILESYSTEM_COMPLETION_CANDIDATES,
        cancellation,
    )
}

#[cfg(test)]
fn bounded_sorted_candidates<T: Ord>(
    candidates: impl IntoIterator<Item = T>,
    limit: usize,
) -> Vec<T> {
    bounded_sorted_candidates_cancellable(candidates, limit, &CompletionCancellation::default())
}

fn bounded_sorted_candidates_cancellable<T: Ord>(
    candidates: impl IntoIterator<Item = T>,
    limit: usize,
    cancellation: &CompletionCancellation,
) -> Vec<T> {
    let mut candidates_by_largest = std::collections::BinaryHeap::with_capacity(limit);
    let mut candidates = candidates.into_iter();
    while !cancellation.is_cancelled() {
        let Some(candidate) = candidates.next() else {
            break;
        };
        if candidates_by_largest.len() < limit {
            candidates_by_largest.push(candidate);
        } else if candidates_by_largest
            .peek()
            .is_some_and(|largest| candidate < *largest)
        {
            candidates_by_largest.pop();
            candidates_by_largest.push(candidate);
        }
    }
    let mut candidates = candidates_by_largest.into_vec();
    candidates.sort();
    candidates
}

fn bounded_sorted_unique_candidates<T: Ord>(
    candidates: impl IntoIterator<Item = T>,
    limit: usize,
) -> Vec<T> {
    let mut unique_candidates = std::collections::BTreeSet::new();
    for candidate in candidates {
        unique_candidates.insert(candidate);
        if unique_candidates.len() > limit {
            unique_candidates.pop_last();
        }
    }
    unique_candidates.into_iter().collect()
}

#[cfg(test)]
pub(crate) fn expand_multi_command(template: &str, limit: usize) -> Result<Vec<String>, String> {
    expand_multi_command_with_labels(template, limit).map(|expansions| {
        expansions
            .into_iter()
            .map(|expansion| expansion.command)
            .collect()
    })
}

pub(crate) fn expand_multi_command_with_labels(
    template: &str,
    limit: usize,
) -> Result<Vec<MultiCommandExpansion>, String> {
    if template.trim().is_empty() {
        return Err("Enter a command containing a double-brace list".to_owned());
    }
    if template.len() > MAX_MULTI_COMMAND_TEMPLATE_BYTES {
        return Err(format!(
            "A multi-command template can contain at most {MAX_MULTI_COMMAND_TEMPLATE_BYTES} bytes"
        ));
    }

    let mut expanded = Vec::new();
    let mut pending = vec![(template.to_owned(), Vec::<String>::new())];
    while let Some((command, parameters)) = pending.pop() {
        let Some((start, end, alternatives)) = first_double_brace_list(&command) else {
            if expanded.len() >= limit {
                return Err(format!("A multi-command can create at most {limit} panes"));
            }
            expanded.push(MultiCommandExpansion {
                command,
                label: parameters.join(" · "),
            });
            continue;
        };

        let pending_output_count = expanded
            .len()
            .checked_add(pending.len())
            .ok_or_else(|| format!("A multi-command can create at most {limit} panes"))?;
        let remaining_outputs = limit
            .checked_sub(pending_output_count)
            .ok_or_else(|| format!("A multi-command can create at most {limit} panes"))?;
        let mut expanded_alternatives = Vec::new();
        for alternative in alternatives {
            if expanded_alternatives.len() >= remaining_outputs {
                return Err(format!("A multi-command can create at most {limit} panes"));
            }
            for alternative in expand_multi_command_fragment(alternative, limit)? {
                if expanded_alternatives.len() >= remaining_outputs {
                    return Err(format!("A multi-command can create at most {limit} panes"));
                }
                expanded_alternatives.push(alternative);
            }
        }
        let alternatives = expanded_alternatives;

        let prefix = &command[..start];
        let suffix = &command[end..];
        for alternative in alternatives.into_iter().rev() {
            let mut next = String::with_capacity(prefix.len() + alternative.len() + suffix.len());
            next.push_str(prefix);
            next.push_str(&alternative);
            next.push_str(suffix);
            let mut next_parameters = parameters.clone();
            let parameter = alternative.trim();
            next_parameters.push(if parameter.is_empty() {
                "(empty)".to_owned()
            } else {
                parameter.to_owned()
            });
            pending.push((next, next_parameters));
        }
    }
    if expanded.len() < 2 && has_active_double_brace_opener(template) {
        return Err(
            "Use a comma-separated double-brace list, for example ssh {{a,b}}.example.com"
                .to_owned(),
        );
    }
    Ok(expanded)
}

fn expand_multi_command_fragment(fragment: &str, limit: usize) -> Result<Vec<String>, String> {
    let mut expanded = Vec::new();
    let mut pending = vec![fragment.to_owned()];
    while let Some(value) = pending.pop() {
        let Some((start, end, alternatives)) = first_double_brace_list(&value) else {
            if expanded.len() >= limit {
                return Err(format!("A multi-command can create at most {limit} panes"));
            }
            expanded.push(value);
            continue;
        };
        if expanded
            .len()
            .checked_add(pending.len())
            .and_then(|count| count.checked_add(alternatives.len()))
            .is_none_or(|minimum_outputs| minimum_outputs > limit)
        {
            return Err(format!("A multi-command can create at most {limit} panes"));
        }
        let prefix = &value[..start];
        let suffix = &value[end..];
        for alternative in alternatives.into_iter().rev() {
            let mut next = String::with_capacity(prefix.len() + alternative.len() + suffix.len());
            next.push_str(prefix);
            next.push_str(alternative);
            next.push_str(suffix);
            pending.push(next);
        }
    }
    Ok(expanded)
}

fn has_active_double_brace_opener(command: &str) -> bool {
    let bytes = command.as_bytes();
    let mut quote = None;
    let mut escaped = false;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if escaped {
            escaped = false;
            index += 1;
            continue;
        }
        if byte == b'\\' && quote != Some(b'\'') {
            escaped = true;
            index += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            if quote == Some(byte) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(byte);
            }
            index += 1;
            continue;
        }
        if quote.is_none() && index + 1 < bytes.len() && &bytes[index..index + 2] == b"{{" {
            return true;
        }
        index += 1;
    }
    false
}

fn first_double_brace_list(command: &str) -> Option<(usize, usize, Vec<&str>)> {
    struct OpenGroup {
        start: usize,
        alternative_start: usize,
        alternatives: Vec<(usize, usize)>,
        has_comma: bool,
    }

    struct Candidate {
        start: usize,
        end: usize,
        alternatives: Vec<(usize, usize)>,
    }

    let bytes = command.as_bytes();
    let mut quote = None;
    let mut escaped = false;
    let mut groups = Vec::<OpenGroup>::new();
    let mut candidate: Option<Candidate> = None;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if escaped {
            escaped = false;
            index += 1;
            continue;
        }
        if byte == b'\\' && quote != Some(b'\'') {
            escaped = true;
            index += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            if quote == Some(byte) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(byte);
            }
            index += 1;
            continue;
        }
        if quote.is_some() || index + 1 >= bytes.len() {
            index += 1;
            continue;
        }
        match &bytes[index..index + 2] {
            b"{{" => {
                groups.push(OpenGroup {
                    start: index,
                    alternative_start: index + 2,
                    alternatives: Vec::new(),
                    has_comma: false,
                });
                index += 2;
            }
            b"}}" => {
                if let Some(mut group) = groups.pop()
                    && group.has_comma
                {
                    group.alternatives.push((group.alternative_start, index));
                    let completed = Candidate {
                        start: group.start,
                        end: index + 2,
                        alternatives: group.alternatives,
                    };
                    if candidate
                        .as_ref()
                        .is_none_or(|candidate| completed.start < candidate.start)
                    {
                        candidate = Some(completed);
                    }
                }
                index += 2;
            }
            _ => {
                if byte == b','
                    && let Some(group) = groups.last_mut()
                {
                    group.alternatives.push((group.alternative_start, index));
                    group.alternative_start = index + 1;
                    group.has_comma = true;
                }
                index += 1;
            }
        }
    }

    candidate.map(|candidate| {
        let alternatives = candidate
            .alternatives
            .into_iter()
            .map(|(start, end)| &command[start..end])
            .collect();
        (candidate.start, candidate.end, alternatives)
    })
}

#[cfg(test)]
#[path = "tests/multi_command.rs"]
mod tests;
