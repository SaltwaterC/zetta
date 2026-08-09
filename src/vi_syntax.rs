use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    fs,
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc,
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
    startup::{selected_theme_name, with_zetta_theme_overrides},
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

struct LanguageHighlighter {
    name: &'static str,
    language: tree_sitter::Language,
    suffixes: Vec<String>,
    first_line_pattern: Option<Regex>,
    configuration: Option<HighlightConfiguration>,
}

/// A small Tree-sitter adapter for the standalone vi editor.
///
/// Grammar functions come directly from Tree-sitter crates, while Zed's
/// upstream grammar configs and Zetta's extension configs/queries are embedded
/// locally. The adapter owns only Tree-sitter's parser/highlighter state and
/// language configurations; it does not depend on Zed's language buffer or
/// syntax map.
pub(crate) struct ZedSyntaxHighlighter {
    languages: Vec<LanguageHighlighter>,
    language_names: HashMap<String, usize>,
    capture_names: Vec<String>,
    styles: Vec<Option<HighlightStyle>>,
    syntax_theme: Arc<SyntaxTheme>,
    highlighter: Highlighter,
    preview_cache: Option<VisibleSyntaxCache>,
}

struct VisibleSyntaxCache {
    context_range: Range<usize>,
    spans: Vec<HighlightSpan>,
}

impl ZedSyntaxHighlighter {
    pub(crate) fn new(syntax_theme: Arc<SyntaxTheme>) -> anyhow::Result<Self> {
        let mut languages = Vec::new();
        let mut language_names = HashMap::new();

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
            languages.push(LanguageHighlighter {
                name,
                language,
                suffixes: language_config.path_suffixes,
                first_line_pattern: language_config
                    .first_line_pattern
                    .map(|pattern| Regex::new(&pattern))
                    .transpose()?,
                configuration: None,
            });
        }

        Ok(Self {
            languages,
            language_names,
            capture_names: Vec::new(),
            styles: Vec::new(),
            syntax_theme,
            highlighter: Highlighter::new(),
            preview_cache: None,
        })
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

        self.languages
            .iter()
            .enumerate()
            .filter_map(|(index, language)| {
                language
                    .first_line_pattern
                    .as_ref()
                    .is_some_and(|pattern| pattern.is_match(&first_line))
                    .then_some(index)
            })
            .max()
    }

    /// Compile a grammar's Zed queries only when that grammar is actually
    /// selected. The initial implementation eagerly compiled every native
    /// grammar before vi could render its first frame.
    fn ensure_configuration(&mut self, language_index: usize) -> anyhow::Result<()> {
        if self.languages[language_index].configuration.is_some() {
            return Ok(());
        }

        let name = self.languages[language_index].name;
        let highlights_query = load_query(name, "highlights")?;
        if highlights_query.is_empty() {
            anyhow::bail!("missing highlights query for native grammar {name:?}");
        }
        let injections_query = load_query(name, "injections")?;
        let configuration = HighlightConfiguration::new(
            self.languages[language_index].language.clone(),
            name,
            &highlights_query,
            &injections_query,
            "",
        )?;
        self.languages[language_index].configuration = Some(configuration);

        let mut added_capture_name = false;
        for language in &self.languages {
            let Some(configuration) = language.configuration.as_ref() else {
                continue;
            };
            for capture_name in configuration.names() {
                if self
                    .capture_names
                    .iter()
                    .all(|known_name| known_name != capture_name)
                {
                    self.capture_names.push((*capture_name).to_owned());
                    self.styles
                        .push(style_for_capture(&self.syntax_theme, capture_name));
                    added_capture_name = true;
                }
            }
        }

        // Every loaded grammar must use the same capture table. This is what
        // lets Tree-sitter's numeric highlight ids remain valid when a Zed
        // injection switches from Markdown to Rust, JSONC, and so on.
        if added_capture_name {
            for language in &mut self.languages {
                if let Some(configuration) = language.configuration.as_mut() {
                    configuration.configure(&self.capture_names);
                }
            }
        } else if let Some(configuration) = self.languages[language_index].configuration.as_mut() {
            configuration.configure(&self.capture_names);
        }

        Ok(())
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
        let Some(language_index) = self.language_index(path, buffer) else {
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

        let Some(language_index) = self.language_index(path, buffer) else {
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
        if self.ensure_configuration(language_index).is_err() {
            return Vec::new();
        }

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
                let languages = &self.languages;
                let language_names = &self.language_names;
                let styles = &self.styles;
                let configuration = languages[language_index]
                    .configuration
                    .as_ref()
                    .expect("selected grammar configuration is loaded");
                let events =
                    self.highlighter
                        .highlight(configuration, buffer, cancellation_flag, |name| {
                            let &injected_language =
                                language_names.get(&name.to_ascii_lowercase())?;
                            let Some(configuration) =
                                languages[injected_language].configuration.as_ref()
                            else {
                                missing_languages.borrow_mut().insert(injected_language);
                                return None;
                            };
                            Some(configuration)
                        });
                let events_vec: Vec<_> = events.expect("highlight failed").collect();
                let spans = highlight_spans(events_vec.into_iter(), styles);
                (spans, missing_languages.into_inner())
            };

            if missing_languages.is_empty() {
                return spans;
            }
            for injected_language in missing_languages {
                if self.ensure_configuration(injected_language).is_err() {
                    return spans;
                }
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
    let (config, _) = crate::startup::load_startup_config(None, None);
    let configured_theme = config.theme.clone();
    busy_v::run_with_editor_setup(arguments, move |editor| {
        install_background(editor, configured_theme.clone());
    })
}

#[cfg(test)]
pub(crate) fn new_shared(
    syntax_theme: Arc<SyntaxTheme>,
) -> anyhow::Result<Rc<RefCell<ZedSyntaxHighlighter>>> {
    ZedSyntaxHighlighter::new(syntax_theme).map(|highlighter| Rc::new(RefCell::new(highlighter)))
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
    configured_theme: Option<String>,
    preview_highlighter: Option<ZedSyntaxHighlighter>,
    preview_initialization_attempted: bool,
    revision: usize,
    in_flight: bool,
    pending: Option<SyntaxJob>,
}

impl BackgroundZedSyntaxHighlighter {
    fn new(
        path: Option<std::path::PathBuf>,
        configured_theme: Option<String>,
    ) -> anyhow::Result<Self> {
        let (requests, worker_requests) = mpsc::channel();
        let (worker_results, results) = mpsc::channel();
        let cancellation = Arc::new(AtomicUsize::new(0));
        let worker_cancellation = Arc::clone(&cancellation);
        let latest_revision = Arc::new(AtomicUsize::new(0));
        let worker_latest_revision = Arc::clone(&latest_revision);
        let worker_path = path.clone();
        let worker_configured_theme = configured_theme.clone();

        thread::Builder::new()
            .name("zetta-vi-syntax".to_owned())
            .spawn(move || {
                run_syntax_worker(
                    worker_path,
                    worker_configured_theme,
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
            configured_theme,
            preview_highlighter: None,
            preview_initialization_attempted: false,
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
        if !self.preview_initialization_attempted {
            self.preview_initialization_attempted = true;
            self.preview_highlighter = match ZedSyntaxHighlighter::new(active_syntax_theme_for(
                self.configured_theme.as_deref(),
            )) {
                Ok(highlighter) => Some(highlighter),
                Err(error) => {
                    eprintln!("zetta vi: syntax preview unavailable: {error:#}");
                    None
                }
            };
        }

        self.preview_highlighter.as_mut().map(|highlighter| {
            highlighter.highlight_visible_path(self.path.as_deref(), buffer, visible_range)
        })
    }

    fn invalidate_visible(&mut self) {
        if let Some(highlighter) = self.preview_highlighter.as_mut() {
            highlighter.invalidate_visible();
        }
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
    configured_theme: Option<String>,
    requests: Receiver<SyntaxJob>,
    results: Sender<SyntaxResult>,
    cancellation: Arc<AtomicUsize>,
    latest_revision: Arc<AtomicUsize>,
) {
    // Both theme loading and grammar-query construction happen here instead
    // of before vi's first alternate-screen frame.
    let mut highlighter =
        match ZedSyntaxHighlighter::new(active_syntax_theme_for(configured_theme.as_deref())) {
            Ok(highlighter) => Some(highlighter),
            Err(error) => {
                eprintln!("zetta vi: syntax highlighting unavailable: {error:#}");
                None
            }
        };

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
            highlighter
                .as_mut()
                .map(|highlighter| {
                    highlighter.highlight_path_with_cancellation(
                        path.as_deref(),
                        &job.buffer,
                        Some(&cancellation),
                    )
                })
                .unwrap_or_default()
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

fn install_background(editor: &mut busy_v::Editor, configured_theme: Option<String>) {
    let path = editor.filename().map(Path::to_path_buf);
    match BackgroundZedSyntaxHighlighter::new(path, configured_theme) {
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
        .and_then(|path| highlighter.borrow().language_index_from_path(path));
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

    registry
        .get(selected_theme_name(configured_theme))
        .map(with_zetta_theme_overrides)
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
