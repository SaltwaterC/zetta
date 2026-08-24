use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    env, fs,
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver, Sender, TryRecvError},
    },
    thread,
};

#[cfg(test)]
use std::rc::Rc;

use anyhow::Context as _;
use busy_v::{HighlightColor, HighlightSpan, HighlightStyle, SyntaxHighlighter};
use gpui::{FontStyle, HighlightStyle as ZedHighlightStyle};
use regex::Regex;
use rust_embed::RustEmbed;
use serde::Deserialize;
use theme::{SyntaxTheme, Theme, ThemeRegistry};
use tree_sitter_highlight::{Highlight, HighlightConfiguration, HighlightEvent, Highlighter};

use crate::{
    process_control::request_process_pane_theme_name, startup::selected_theme_name,
    zetta_assets::ZettaAssets,
};

#[derive(RustEmbed)]
#[folder = "zed/crates/grammars/src/"]
#[exclude = "*.rs"]
struct GrammarAssets;

#[derive(RustEmbed)]
#[folder = "src/grammar_extensions/"]
struct ExtensionGrammarAssets;

#[derive(Deserialize)]
struct GrammarConfig {
    name: String,
    grammar: String,
    #[serde(default)]
    code_fence_block_name: Option<String>,
    #[serde(default)]
    path_suffixes: Vec<String>,
    #[serde(default)]
    first_line_pattern: Option<String>,
    #[serde(default)]
    modeline_aliases: Vec<String>,
}

fn load_config(name: &str) -> anyhow::Result<GrammarConfig> {
    let path = format!("{name}/config.toml");
    let bytes = grammar_asset(&path)
        .ok_or_else(|| anyhow::anyhow!("missing grammar config for {name:?}"))?;
    toml::from_str(std::str::from_utf8(&bytes.data)?)
        .with_context(|| format!("parsing embedded grammar config {path:?}"))
}

fn load_query(name: &str, prefix: &str) -> anyhow::Result<String> {
    let path_prefix = format!("{name}/{prefix}");
    let mut paths: Vec<String> = GrammarAssets::iter()
        .chain(ExtensionGrammarAssets::iter())
        .filter(|path| path.starts_with(&path_prefix) && path.ends_with(".scm"))
        .map(|path| path.to_string())
        .collect();
    paths.sort_unstable();

    let mut query = String::new();
    for path in paths {
        let bytes = grammar_asset(&path)
            .ok_or_else(|| anyhow::anyhow!("missing embedded grammar query {path:?}"))?
            .data;
        query.push_str(
            std::str::from_utf8(&bytes)
                .with_context(|| format!("decoding embedded grammar query {path:?}"))?,
        );
    }
    Ok(query)
}

fn grammar_asset(path: &str) -> Option<rust_embed::EmbeddedFile> {
    GrammarAssets::get(path).or_else(|| ExtensionGrammarAssets::get(path))
}

/// Keep this list synchronized with Zed's native grammars registry while
/// avoiding a dependency on Zed's language runtime and its Wasmtime support.
fn native_grammars() -> Vec<(&'static str, tree_sitter::Language)> {
    vec![
        ("bash", tree_sitter_bash::LANGUAGE.into()),
        ("c", tree_sitter_c::LANGUAGE.into()),
        ("cpp", tree_sitter_cpp::LANGUAGE.into()),
        ("css", tree_sitter_css::LANGUAGE.into()),
        ("diff", tree_sitter_diff::LANGUAGE.into()),
        ("go", tree_sitter_go::LANGUAGE.into()),
        ("gomod", tree_sitter_go_mod::LANGUAGE.into()),
        ("gowork", tree_sitter_gowork::LANGUAGE.into()),
        ("jsdoc", tree_sitter_jsdoc::LANGUAGE.into()),
        ("json", tree_sitter_json::LANGUAGE.into()),
        ("jsonc", tree_sitter_json::LANGUAGE.into()),
        ("markdown", tree_sitter_md::LANGUAGE.into()),
        ("markdown-inline", tree_sitter_md::INLINE_LANGUAGE.into()),
        ("python", tree_sitter_python::LANGUAGE.into()),
        ("regex", tree_sitter_regex::LANGUAGE.into()),
        ("rust", tree_sitter_rust::LANGUAGE.into()),
        ("tsx", tree_sitter_typescript::LANGUAGE_TSX.into()),
        (
            "typescript",
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        ),
        ("yaml", tree_sitter_yaml::LANGUAGE.into()),
        ("gitcommit", tree_sitter_gitcommit::LANGUAGE.into()),
    ]
}

/// Grammars maintained outside Zed's upstream grammar bundle. Each entry is
/// paired with config/query assets under `src/grammar_extensions/{name}`.
fn extension_grammars() -> Vec<(&'static str, tree_sitter::Language)> {
    vec![
        ("makefile", tree_sitter_make::LANGUAGE.into()),
        ("toml", tree_sitter_toml_ng::LANGUAGE.into()),
        ("powershell", tree_sitter_powershell::LANGUAGE.into()),
        ("batch", tree_sitter_batch::LANGUAGE.into()),
    ]
}

struct LanguageEntry {
    name: &'static str,
    language: tree_sitter::Language,
    suffixes: Vec<String>,
    first_line_pattern: Option<String>,
}

/// The syntax theme, resolved on a worker thread.
///
/// Building a [`ThemeRegistry`] parses every bundled theme family and reads the
/// user's theme directory, which is the one piece of vi's startup cost that
/// does not depend on the open file's grammar. Resolving it off-thread lets it
/// overlap with the Tree-sitter query compilation that gates the first frame.
struct SyntaxThemeHandle {
    configured_theme: Option<String>,
    pending: Mutex<Option<thread::JoinHandle<Arc<SyntaxTheme>>>>,
    theme: OnceLock<Arc<SyntaxTheme>>,
}

impl SyntaxThemeHandle {
    fn spawn(configured_theme: Option<String>) -> Self {
        let worker_configured_theme = configured_theme.clone();
        let pending = thread::Builder::new()
            .name("zetta-vi-theme".to_owned())
            .spawn(move || active_syntax_theme_for(worker_configured_theme.as_deref()))
            .ok();
        Self {
            configured_theme,
            pending: Mutex::new(pending),
            theme: OnceLock::new(),
        }
    }

    #[cfg(test)]
    fn ready(theme: Arc<SyntaxTheme>) -> Self {
        let resolved = OnceLock::new();
        let _ = resolved.set(theme);
        Self {
            configured_theme: None,
            pending: Mutex::new(None),
            theme: resolved,
        }
    }

    fn get(&self) -> Arc<SyntaxTheme> {
        if let Some(theme) = self.theme.get() {
            return Arc::clone(theme);
        }

        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if self.theme.get().is_none() {
            // Fall back to loading inline when the worker could not be spawned
            // or panicked; vi must never lose highlighting over a thread error.
            let theme = pending
                .take()
                .and_then(|handle| handle.join().ok())
                .unwrap_or_else(|| active_syntax_theme_for(self.configured_theme.as_deref()));
            let _ = self.theme.set(theme);
        }
        drop(pending);

        self.theme
            .get()
            .cloned()
            .unwrap_or_else(|| Arc::new(SyntaxTheme::new([])))
    }
}

/// Grammar metadata and compiled Tree-sitter queries, shared by every
/// highlighter in the process.
///
/// Compiling one grammar's queries costs tens of milliseconds, so the visible
/// preview on the main thread and the background full-buffer parse must reach
/// the same [`HighlightConfiguration`] rather than each compiling their own.
/// Configurations are immutable once compiled, which is what makes sharing them
/// across threads sound.
pub(crate) struct GrammarSet {
    languages: Vec<LanguageEntry>,
    language_names: HashMap<String, usize>,
    capture_names: Vec<String>,
    first_line_patterns: OnceLock<Vec<(usize, Regex)>>,
    theme: SyntaxThemeHandle,
    styles: OnceLock<Vec<Option<HighlightStyle>>>,
    configurations: Mutex<Vec<Option<Arc<HighlightConfiguration>>>>,
}

impl GrammarSet {
    fn new(theme: SyntaxThemeHandle) -> anyhow::Result<Arc<Self>> {
        let mut languages = Vec::new();
        let mut language_names = HashMap::new();
        let mut grammar_ids = Vec::new();

        for (name, language) in native_grammars().into_iter().chain(extension_grammars()) {
            let language_config = load_config(name)?;
            if language_config.grammar != name {
                anyhow::bail!(
                    "embedded grammar config for {name:?} declares {:?}",
                    language_config.grammar
                );
            }

            let index = languages.len();
            add_language_name(&mut language_names, name, index);
            add_language_name(&mut language_names, &language_config.name, index);
            if let Some(code_fence_name) = language_config.code_fence_block_name.as_deref() {
                add_language_name(&mut language_names, code_fence_name, index);
            }
            for alias in &language_config.modeline_aliases {
                add_language_name(&mut language_names, alias, index);
            }
            grammar_ids.push(name);
            languages.push(LanguageEntry {
                name,
                language,
                suffixes: language_config.path_suffixes,
                first_line_pattern: language_config.first_line_pattern,
            });
        }

        let capture_names = collect_capture_names(&grammar_ids)?;
        let configurations = Mutex::new(vec![None; languages.len()]);
        Ok(Arc::new(Self {
            languages,
            language_names,
            capture_names,
            first_line_patterns: OnceLock::new(),
            theme,
            styles: OnceLock::new(),
            configurations,
        }))
    }

    fn language_index(&self, path: Option<&Path>, source: &[u8]) -> Option<usize> {
        path.and_then(|path| self.language_index_from_path(path))
            .or_else(|| self.language_index_from_first_line(source))
    }

    fn language_index_from_path(&self, path: &Path) -> Option<usize> {
        let filename = path.file_name().and_then(|filename| filename.to_str());
        let extension = filename.and_then(|filename| filename.split('.').next_back());
        let path_string = path.to_str();
        let candidates = [extension, filename, path_string];

        self.languages
            .iter()
            .enumerate()
            .filter_map(|(index, language)| {
                language
                    .suffixes
                    .iter()
                    .filter_map(|suffix| {
                        candidates
                            .iter()
                            .flatten()
                            .find(|candidate| matches_suffix(candidate, suffix))
                            .map(|_| suffix.len())
                    })
                    .max()
                    .map(|score| (score, index))
            })
            .max_by_key(|(score, index)| (*score, *index))
            .map(|(_, index)| index)
    }

    fn language_index_from_first_line(&self, source: &[u8]) -> Option<usize> {
        let first_line = source
            .split(|byte| *byte == b'\n')
            .next()
            .unwrap_or_default();
        let first_line = String::from_utf8_lossy(first_line);

        self.first_line_patterns()
            .iter()
            .filter(|(_, pattern)| pattern.is_match(&first_line))
            .map(|(index, _)| *index)
            .max()
    }

    /// Compile the shebang/modeline patterns only when a path suffix did not
    /// already identify the grammar. Building these regexes is a measurable
    /// part of startup and most files are recognized by their suffix.
    fn first_line_patterns(&self) -> &[(usize, Regex)] {
        self.first_line_patterns.get_or_init(|| {
            self.languages
                .iter()
                .enumerate()
                .filter_map(|(index, language)| {
                    let pattern = language.first_line_pattern.as_deref()?;
                    match Regex::new(pattern) {
                        Ok(pattern) => Some((index, pattern)),
                        Err(error) => {
                            eprintln!(
                                "zetta vi: ignoring first-line pattern for {:?}: {error}",
                                language.name
                            );
                            None
                        }
                    }
                })
                .collect()
        })
    }

    #[cfg(test)]
    fn language_index_for_name(&self, name: &str) -> Option<usize> {
        self.language_names.get(&name.to_ascii_lowercase()).copied()
    }

    /// The capture-name to style table, resolved once the theme is available.
    fn styles(&self) -> &[Option<HighlightStyle>] {
        self.styles.get_or_init(|| {
            let theme = self.theme.get();
            self.capture_names
                .iter()
                .map(|capture_name| style_for_capture(&theme, capture_name))
                .collect()
        })
    }

    /// Compile a grammar's Zed queries the first time that grammar is used.
    ///
    /// The lock is deliberately held across the compile: when the preview and
    /// the background parse race for the same grammar, the loser waits for the
    /// winner's result instead of repeating tens of milliseconds of work.
    fn configuration(&self, language_index: usize) -> anyhow::Result<Arc<HighlightConfiguration>> {
        let mut configurations = self
            .configurations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(configuration) = configurations[language_index].as_ref() {
            return Ok(Arc::clone(configuration));
        }

        let language = &self.languages[language_index];
        let name = language.name;
        let highlights_query = load_query(name, "highlights")?;
        if highlights_query.is_empty() {
            anyhow::bail!("missing highlights query for native grammar {name:?}");
        }
        let injections_query = load_query(name, "injections")?;
        let mut configuration = HighlightConfiguration::new(
            language.language.clone(),
            name,
            &highlights_query,
            &injections_query,
            "",
        )?;
        // Every grammar resolves against the same capture table, which is what
        // lets Tree-sitter's numeric highlight ids stay valid when an injection
        // switches from Markdown to Rust, JSONC, and so on.
        configuration.configure(&self.capture_names);

        let configuration = Arc::new(configuration);
        configurations[language_index] = Some(Arc::clone(&configuration));
        Ok(configuration)
    }

    fn configuration_snapshot(&self) -> Vec<Option<Arc<HighlightConfiguration>>> {
        self.configurations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    #[cfg(test)]
    fn loaded_configuration_count(&self) -> usize {
        self.configuration_snapshot()
            .iter()
            .filter(|configuration| configuration.is_some())
            .count()
    }

    #[cfg(test)]
    fn has_configuration(&self, language_index: usize) -> bool {
        self.configuration_snapshot()[language_index].is_some()
    }
}

/// Collect every capture name the embedded queries can produce.
///
/// The table has to cover all grammars before any of them is configured,
/// because a configured [`HighlightConfiguration`] is never mutated again.
/// Scanning the query text avoids compiling grammars the open file never uses.
fn collect_capture_names(grammar_ids: &[&'static str]) -> anyhow::Result<Vec<String>> {
    let mut capture_names = Vec::new();
    let mut seen = HashSet::new();
    for name in grammar_ids {
        for prefix in ["highlights", "injections"] {
            let query = load_query(name, prefix)?;
            for capture_name in query_capture_names(&query) {
                if seen.insert(capture_name.to_owned()) {
                    capture_names.push(capture_name.to_owned());
                }
            }
        }
    }
    Ok(capture_names)
}

/// Yield the `@capture` names a query declares.
///
/// Comments and string literals are skipped so an anonymous node such as Rust's
/// `"@"` token is not mistaken for a capture.
fn query_capture_names(query: &str) -> Vec<&str> {
    let bytes = query.as_bytes();
    let mut names = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b';' => {
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            b'"' => {
                index += 1;
                while index < bytes.len() && bytes[index] != b'"' {
                    index += if bytes[index] == b'\\' { 2 } else { 1 };
                }
                index = index.saturating_add(1).min(bytes.len());
            }
            b'@' => {
                let start = index + 1;
                let mut end = start;
                while end < bytes.len() && is_capture_name_byte(bytes[end]) {
                    end += 1;
                }
                if end > start {
                    names.push(&query[start..end]);
                }
                index = end.max(start);
            }
            _ => index += 1,
        }
    }
    names
}

fn is_capture_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-')
}

/// A small Tree-sitter adapter for the standalone vi editor.
///
/// Grammar functions come directly from Tree-sitter crates, while Zed's
/// upstream grammar configs and Zetta's extension configs/queries are embedded
/// locally. The adapter owns only Tree-sitter's per-thread parser state; the
/// grammars, capture table, and styles live in the shared [`GrammarSet`].
pub(crate) struct ZedSyntaxHighlighter {
    grammars: Arc<GrammarSet>,
    highlighter: Highlighter,
    preview_cache: Option<VisibleSyntaxCache>,
}

struct VisibleSyntaxCache {
    context_range: Range<usize>,
    spans: Vec<HighlightSpan>,
}

impl ZedSyntaxHighlighter {
    pub(crate) fn new(grammars: Arc<GrammarSet>) -> Self {
        Self {
            grammars,
            highlighter: Highlighter::new(),
            preview_cache: None,
        }
    }
}

#[cfg(test)]
impl SyntaxHighlighter for ZedSyntaxHighlighter {
    fn highlight(&mut self, buffer: &[u8]) -> Vec<HighlightSpan> {
        self.highlight_path(None, buffer)
    }
}

impl ZedSyntaxHighlighter {
    #[cfg(test)]
    fn highlight_path(&mut self, path: Option<&Path>, buffer: &[u8]) -> Vec<HighlightSpan> {
        self.highlight_path_with_cancellation(path, buffer, None)
    }

    fn highlight_path_with_cancellation(
        &mut self,
        path: Option<&Path>,
        buffer: &[u8],
        cancellation_flag: Option<&AtomicUsize>,
    ) -> Vec<HighlightSpan> {
        let Some(language_index) = self.grammars.language_index(path, buffer) else {
            return Vec::new();
        };

        self.highlight_language_with_cancellation(language_index, buffer, cancellation_flag)
    }

    fn highlight_visible_path(
        &mut self,
        path: Option<&Path>,
        buffer: &[u8],
        visible_range: Range<usize>,
    ) -> Vec<HighlightSpan> {
        let visible_start = visible_range.start.min(buffer.len());
        let visible_end = visible_range.end.min(buffer.len()).max(visible_start);
        if visible_start == visible_end {
            return Vec::new();
        }

        let Some(language_index) = self.grammars.language_index(path, buffer) else {
            return Vec::new();
        };
        let visible_range = visible_start..visible_end;
        if self.preview_cache.as_ref().is_some_and(|cache| {
            cache.context_range.start <= visible_range.start
                && visible_range.end <= cache.context_range.end
        }) {
            return self
                .preview_cache
                .as_ref()
                .map(|cache| clip_preview_spans(&cache.spans, visible_range))
                .unwrap_or_default();
        }
        let context_range = syntax_preview_context_range(buffer, visible_range.clone());
        let spans = self.highlight_language_with_cancellation(
            language_index,
            &buffer[context_range.clone()],
            None,
        );

        let spans = spans
            .into_iter()
            .map(|span| {
                HighlightSpan::new(
                    span.start.saturating_add(context_range.start),
                    span.end.saturating_add(context_range.start),
                    span.style,
                )
            })
            .collect();
        self.preview_cache = Some(VisibleSyntaxCache {
            context_range,
            spans,
        });
        self.preview_cache
            .as_ref()
            .map(|cache| clip_preview_spans(&cache.spans, visible_range))
            .unwrap_or_default()
    }

    fn invalidate_visible(&mut self) {
        self.preview_cache = None;
    }

    #[cfg(test)]
    fn highlight_language(&mut self, language_index: usize, buffer: &[u8]) -> Vec<HighlightSpan> {
        self.highlight_language_with_cancellation(language_index, buffer, None)
    }

    fn highlight_language_with_cancellation(
        &mut self,
        language_index: usize,
        buffer: &[u8],
        cancellation_flag: Option<&AtomicUsize>,
    ) -> Vec<HighlightSpan> {
        let grammars = Arc::clone(&self.grammars);
        let Ok(root_configuration) = grammars.configuration(language_index) else {
            return Vec::new();
        };
        let styles = grammars.styles();
        let mut configurations = grammars.configuration_snapshot();
        configurations[language_index] = Some(root_configuration);

        loop {
            if cancellation_flag.is_some_and(|flag| flag.load(Ordering::Acquire) != 0) {
                return Vec::new();
            }

            // An injected language can be discovered only while Tree-sitter
            // walks the root grammar. Record missing configurations during a
            // cheap first pass, compile only those grammars, then retry. This
            // preserves Zed's dynamic Markdown fence behavior without eagerly
            // compiling all native grammar queries for every file.
            let (spans, missing_languages) = {
                let missing_languages = RefCell::new(HashSet::new());
                let language_names = &grammars.language_names;
                let loaded = &configurations;
                let configuration = loaded[language_index]
                    .as_deref()
                    .expect("selected grammar configuration is loaded");
                let events =
                    self.highlighter
                        .highlight(configuration, buffer, cancellation_flag, |name| {
                            let &injected_language =
                                language_names.get(&name.to_ascii_lowercase())?;
                            match loaded[injected_language].as_deref() {
                                Some(configuration) => Some(configuration),
                                None => {
                                    missing_languages.borrow_mut().insert(injected_language);
                                    None
                                }
                            }
                        });
                let spans = highlight_spans(events.expect("highlight failed"), styles);
                (spans, missing_languages.into_inner())
            };

            if missing_languages.is_empty() {
                return spans;
            }
            for injected_language in missing_languages {
                let Ok(configuration) = grammars.configuration(injected_language) else {
                    return spans;
                };
                configurations[injected_language] = Some(configuration);
            }
        }
    }
}

fn clip_preview_spans(spans: &[HighlightSpan], visible_range: Range<usize>) -> Vec<HighlightSpan> {
    let first = spans.partition_point(|span| span.end <= visible_range.start);
    let last = first + spans[first..].partition_point(|span| span.start < visible_range.end);
    spans[first..last]
        .iter()
        .filter_map(|span| {
            let start = span.start.max(visible_range.start);
            let end = span.end.min(visible_range.end);
            (start < end).then(|| HighlightSpan::new(start, end, span.style))
        })
        .collect()
}

fn syntax_preview_context_range(buffer: &[u8], visible_range: Range<usize>) -> Range<usize> {
    let visible_start = visible_range.start.min(buffer.len());
    let visible_end = visible_range.end.min(buffer.len()).max(visible_start);

    let mut start = visible_start.saturating_sub(SYNTAX_PREVIEW_CONTEXT_BYTES);
    while start > 0 && buffer[start - 1] != b'\n' {
        start -= 1;
    }

    let mut end = visible_end
        .saturating_add(SYNTAX_PREVIEW_CONTEXT_BYTES)
        .min(buffer.len());
    while end < buffer.len() && buffer[end] != b'\n' {
        end += 1;
    }
    if end < buffer.len() {
        end += 1;
    }

    start..end
}

fn highlight_spans(
    events: impl Iterator<Item = Result<HighlightEvent, tree_sitter_highlight::Error>>,
    styles: &[Option<HighlightStyle>],
) -> Vec<HighlightSpan> {
    let mut active = Vec::<Highlight>::new();
    let mut spans: Vec<HighlightSpan> = Vec::new();
    for event in events {
        let Ok(event) = event else {
            return Vec::new();
        };
        match event {
            HighlightEvent::HighlightStart(highlight) => active.push(highlight),
            HighlightEvent::HighlightEnd => {
                active.pop();
            }
            HighlightEvent::Source { start, end } => {
                let Some(Highlight(style_index)) = active.last().copied() else {
                    continue;
                };
                let Some(Some(style)) = styles.get(style_index) else {
                    continue;
                };
                if start == end {
                    continue;
                }
                if let Some(previous) = spans.last_mut()
                    && previous.end == start
                    && previous.style == *style
                {
                    previous.end = end;
                } else {
                    spans.push(HighlightSpan::new(start, end, *style));
                }
            }
        }
    }
    spans
}

fn add_language_name(language_names: &mut HashMap<String, usize>, name: &str, index: usize) {
    language_names
        .entry(name.to_ascii_lowercase())
        .or_insert(index);
}

/// Resolve a query capture exactly as Zed does: a theme entry only applies to
/// that capture or one of its dotted-name prefixes. Tree-sitter's built-in
/// matcher also accepts unrelated components (for example `operator` for
/// `keyword.operator.regex`), which gives different colors from Zed themes.
fn style_for_capture(syntax_theme: &SyntaxTheme, capture_name: &str) -> Option<HighlightStyle> {
    syntax_theme
        .highlight_id(capture_name)
        .and_then(|index| usize::try_from(index).ok())
        .and_then(|index| syntax_theme.get(index))
        .map(to_terminal_style)
}

pub(crate) fn run(arguments: Vec<String>) -> i32 {
    let (base, _) = crate::startup::load_startup_config(None, None);
    // A registered project can select its own theme, and this runs as its own
    // process, so the project overlay has to be applied here too — otherwise the
    // editor highlights with the application theme inside a project pane whose
    // terminal is showing a different one. Failures fall back to `base` rather
    // than refusing to open the file.
    let config = crate::project_cli::current_project_config(&base)
        .ok()
        .flatten()
        .map(|project| project.effective)
        .unwrap_or(base);
    // The editor runs in a subprocess, so configuration alone cannot tell it
    // which theme its pane is actually showing. Ask the originating window to
    // preserve project/profile, launch, pane-template, transient, and live
    // appearance choices. A standalone invocation keeps the light-theme
    // configuration fallback it had before.
    let configured_theme = config.theme.clone();
    // Built on the first editor rather than up front, so `vi --help` and
    // argument errors do not pay for grammars they never use. Every file in one
    // session then shares the same compiled grammars.
    let grammars: OnceLock<Option<Arc<GrammarSet>>> = OnceLock::new();

    busy_v::run_with_editor_setup(arguments, move |editor| {
        let grammars = grammars.get_or_init(|| {
            // Resolving the theme is independent of the grammar work, so it
            // starts here and is joined only once styles are actually needed.
            let theme = SyntaxThemeHandle::spawn(selected_vi_theme(
                configured_theme.clone(),
                inherited_pane_theme(),
                inherited_terminal_theme(),
            ));
            match GrammarSet::new(theme) {
                Ok(grammars) => Some(grammars),
                Err(error) => {
                    eprintln!("zetta vi: syntax highlighting unavailable: {error:#}");
                    None
                }
            }
        });
        if let Some(grammars) = grammars.as_ref() {
            install_background(editor, Arc::clone(grammars));
        }
    })
}

fn selected_vi_theme(
    configured_theme: Option<String>,
    pane_theme: Option<String>,
    terminal_theme: Option<String>,
) -> Option<String> {
    pane_theme.or(terminal_theme).or(configured_theme)
}

fn inherited_terminal_theme() -> Option<String> {
    env::var("ZETTA_THEME")
        .ok()
        .filter(|theme| !theme.is_empty())
}

fn inherited_pane_theme() -> Option<String> {
    let process_id = env::var("ZETTA_PROCESS_ID").ok()?.parse().ok()?;
    let attention_id = env::var("ZETTA_ATTENTION_ID").ok()?.parse().ok()?;
    let pane_id = env::var("ZETTA_PANE_ID").ok()?.parse().ok()?;
    request_process_pane_theme_name(process_id, attention_id, pane_id)
        .ok()
        .flatten()
}

#[cfg(test)]
pub(crate) fn new_shared(
    syntax_theme: Arc<SyntaxTheme>,
) -> anyhow::Result<Rc<RefCell<ZedSyntaxHighlighter>>> {
    let grammars = GrammarSet::new(SyntaxThemeHandle::ready(syntax_theme))?;
    Ok(Rc::new(RefCell::new(ZedSyntaxHighlighter::new(grammars))))
}

struct SyntaxJob {
    revision: usize,
    buffer: Vec<u8>,
}

const SYNTAX_PREVIEW_CONTEXT_BYTES: usize = 16 * 1024;

struct SyntaxResult {
    revision: usize,
    highlights: Vec<HighlightSpan>,
}

/// The terminal renderer must never wait for Tree-sitter parsing. This small
/// adapter owns one worker per vi buffer, keeps at most one parse in flight,
/// and replaces queued work with the latest coalesced editor snapshot.
struct BackgroundZedSyntaxHighlighter {
    requests: Sender<SyntaxJob>,
    results: Receiver<SyntaxResult>,
    cancellation: Arc<AtomicUsize>,
    latest_revision: Arc<AtomicUsize>,
    path: Option<PathBuf>,
    preview_highlighter: ZedSyntaxHighlighter,
    revision: usize,
    in_flight: bool,
    pending: Option<SyntaxJob>,
}

impl BackgroundZedSyntaxHighlighter {
    fn new(path: Option<std::path::PathBuf>, grammars: Arc<GrammarSet>) -> anyhow::Result<Self> {
        let (requests, worker_requests) = mpsc::channel();
        let (worker_results, results) = mpsc::channel();
        let cancellation = Arc::new(AtomicUsize::new(0));
        let worker_cancellation = Arc::clone(&cancellation);
        let latest_revision = Arc::new(AtomicUsize::new(0));
        let worker_latest_revision = Arc::clone(&latest_revision);
        let worker_path = path.clone();
        let worker_grammars = Arc::clone(&grammars);

        thread::Builder::new()
            .name("zetta-vi-syntax".to_owned())
            .spawn(move || {
                run_syntax_worker(
                    worker_path,
                    worker_grammars,
                    worker_requests,
                    worker_results,
                    worker_cancellation,
                    worker_latest_revision,
                );
            })
            .context("starting the vi syntax-highlighting worker")?;

        Ok(Self {
            requests,
            results,
            cancellation,
            latest_revision,
            path,
            preview_highlighter: ZedSyntaxHighlighter::new(grammars),
            revision: 0,
            in_flight: false,
            pending: None,
        })
    }

    fn request(&mut self, buffer: &[u8]) {
        self.revision = self.revision.wrapping_add(1);
        self.latest_revision.store(self.revision, Ordering::Release);
        // Discard a result for the previous revision before queuing this
        // snapshot. Busy-V will keep rendering plain text until this revision
        // completes, so no stale spans can be applied after an edit.
        let _ = self.drain_results();
        let job = SyntaxJob {
            revision: self.revision,
            buffer: buffer.to_vec(),
        };
        if self.in_flight {
            self.cancellation.store(1, Ordering::Release);
            self.pending = Some(job);
        } else {
            self.dispatch(job);
        }
    }

    fn dispatch(&mut self, job: SyntaxJob) {
        self.in_flight = self.requests.send(job).is_ok();
    }

    fn drain_results(&mut self) -> Option<Vec<HighlightSpan>> {
        let mut completed = None;
        loop {
            match self.results.try_recv() {
                Ok(result) => {
                    self.in_flight = false;
                    if result.revision == self.revision {
                        completed = Some(result.highlights);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.in_flight = false;
                    self.pending = None;
                    break;
                }
            }
        }
        completed
    }

    fn dispatch_pending(&mut self) {
        if self.in_flight {
            return;
        }
        if let Some(job) = self.pending.take() {
            self.dispatch(job);
        }
    }
}

impl Drop for BackgroundZedSyntaxHighlighter {
    fn drop(&mut self) {
        // The detached worker observes this in Tree-sitter's progress callback
        // and exits once the request channel closes during field teardown.
        self.cancellation.store(1, Ordering::Release);
    }
}

impl SyntaxHighlighter for BackgroundZedSyntaxHighlighter {
    fn highlight(&mut self, buffer: &[u8]) -> Vec<HighlightSpan> {
        self.request(buffer);
        Vec::new()
    }

    fn highlight_visible(
        &mut self,
        buffer: &[u8],
        visible_range: Range<usize>,
    ) -> Option<Vec<HighlightSpan>> {
        Some(self.preview_highlighter.highlight_visible_path(
            self.path.as_deref(),
            buffer,
            visible_range,
        ))
    }

    fn invalidate_visible(&mut self) {
        self.preview_highlighter.invalidate_visible();
    }

    fn poll(&mut self) -> Option<Vec<HighlightSpan>> {
        let completed = self.drain_results();
        self.dispatch_pending();
        completed
    }

    fn has_pending_work(&self) -> bool {
        self.in_flight || self.pending.is_some()
    }
}

fn run_syntax_worker(
    path: Option<std::path::PathBuf>,
    grammars: Arc<GrammarSet>,
    requests: Receiver<SyntaxJob>,
    results: Sender<SyntaxResult>,
    cancellation: Arc<AtomicUsize>,
    latest_revision: Arc<AtomicUsize>,
) {
    let mut highlighter = ZedSyntaxHighlighter::new(grammars);

    while let Ok(mut job) = requests.recv() {
        // A completed worker can race with an edit. Prefer the newest queued
        // snapshot even in that case.
        while let Ok(newer_job) = requests.try_recv() {
            job = newer_job;
        }
        // If editing raced with theme or grammar initialization, do not spend
        // a full parse on the superseded snapshot. For an in-progress parse,
        // the same generation change is signalled through `cancellation`.
        let highlights = if latest_revision.load(Ordering::Acquire) != job.revision {
            Vec::new()
        } else {
            cancellation.store(0, Ordering::Release);
            highlighter.highlight_path_with_cancellation(
                path.as_deref(),
                &job.buffer,
                Some(&cancellation),
            )
        };
        if results
            .send(SyntaxResult {
                revision: job.revision,
                highlights,
            })
            .is_err()
        {
            break;
        }
    }
}

fn install_background(editor: &mut busy_v::Editor, grammars: Arc<GrammarSet>) {
    let path = editor.filename().map(Path::to_path_buf);

    // Compile the open file's grammar here, on the thread that paints the first
    // frame, before the background worker exists. Both would otherwise race for
    // the same lock and the foreground frame could end up waiting on a compile
    // owned by a background thread the scheduler is free to deprioritize.
    if let Some(language_index) = path
        .as_deref()
        .and_then(|path| grammars.language_index_from_path(path))
    {
        let _ = grammars.configuration(language_index);
    }

    match BackgroundZedSyntaxHighlighter::new(path, grammars) {
        Ok(highlighter) => editor.set_syntax_highlighter(Box::new(highlighter)),
        Err(error) => eprintln!("zetta vi: syntax highlighting unavailable: {error:#}"),
    }
}

#[cfg(test)]
pub(crate) fn install(
    editor: &mut busy_v::Editor,
    highlighter: Option<Rc<RefCell<ZedSyntaxHighlighter>>>,
) {
    let Some(highlighter) = highlighter else {
        return;
    };
    let path = editor.filename().map(Path::to_path_buf);
    let language_index = path
        .as_deref()
        .and_then(|path| highlighter.borrow().grammars.language_index_from_path(path));
    editor.set_syntax_highlighter(Box::new(move |buffer: &[u8]| {
        let mut highlighter = highlighter.borrow_mut();
        match language_index {
            Some(language_index) => highlighter.highlight_language(language_index, buffer),
            None => highlighter.highlight_path(path.as_deref(), buffer),
        }
    }));
}

fn active_syntax_theme_for(configured_theme: Option<&str>) -> Arc<SyntaxTheme> {
    let registry = ThemeRegistry::new(Box::new(ZettaAssets));
    theme_settings::load_bundled_themes(&registry);

    if let Ok(entries) = fs::read_dir(crate::config::themes_dir()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            if let Ok(bytes) = fs::read(&path) {
                let _ = theme_settings::load_user_theme(&registry, &bytes);
            }
        }
    }

    // No `apply_zetta_theme_overrides` here: it only restyles scrollbars, which
    // a syntax theme has nothing to do with.
    registry
        .get(selected_theme_name(configured_theme))
        .map(|theme: Arc<Theme>| theme.syntax().clone())
        .unwrap_or_else(|_| Arc::new(SyntaxTheme::new([])))
}

fn to_terminal_style(style: &ZedHighlightStyle) -> HighlightStyle {
    HighlightStyle {
        foreground: style.color.map(to_terminal_color),
        background: style.background_color.map(to_terminal_color),
        bold: style.font_weight.is_some_and(|weight| weight.0 >= 700.0),
        italic: style
            .font_style
            .is_some_and(|font_style| matches!(font_style, FontStyle::Italic | FontStyle::Oblique)),
        underline: style.underline.is_some(),
    }
}

fn matches_suffix(candidate: &str, suffix: &str) -> bool {
    candidate.eq_ignore_ascii_case(suffix)
        || (candidate.len() > suffix.len() + 1
            && candidate.as_bytes()[candidate.len() - suffix.len() - 1] == b'.'
            && candidate.as_bytes()[candidate.len() - suffix.len()..]
                .eq_ignore_ascii_case(suffix.as_bytes()))
}

fn to_terminal_color(color: gpui::Hsla) -> HighlightColor {
    let color = color.to_rgb();
    HighlightColor::Rgb {
        red: (color.r.clamp(0.0, 1.0) * 255.0).round() as u8,
        green: (color.g.clamp(0.0, 1.0) * 255.0).round() as u8,
        blue: (color.b.clamp(0.0, 1.0) * 255.0).round() as u8,
    }
}

#[cfg(test)]
#[path = "tests/vi_syntax.rs"]
mod tests;
