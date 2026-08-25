use super::*;
use busy_v::Editor;
use gpui::{HighlightStyle as ZedHighlightStyle, blue, red};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn grammar_set(capture_names: &[&str]) -> Arc<GrammarSet> {
    GrammarSet::new(SyntaxThemeHandle::ready(syntax_theme(capture_names)))
        .expect("load Zed grammars")
}

fn highlighter(capture_names: &[&str]) -> ZedSyntaxHighlighter {
    ZedSyntaxHighlighter::new(grammar_set(capture_names))
}

fn syntax_theme(capture_names: &[&str]) -> Arc<SyntaxTheme> {
    Arc::new(SyntaxTheme::new(capture_names.iter().map(|capture_name| {
        (
            (*capture_name).to_owned(),
            ZedHighlightStyle {
                color: Some(red()),
                ..Default::default()
            },
        )
    })))
}

#[test]
fn one_dark_uses_the_readable_dark_syntax_palette() {
    let dark = active_syntax_theme_for(Some("One Dark"));
    let light = active_syntax_theme_for(Some("One Light"));

    assert_eq!(
        style_for_capture(&dark, "variable").and_then(|style| style.foreground),
        Some(HighlightColor::Rgb {
            red: 0xac,
            green: 0xb2,
            blue: 0xbe,
        })
    );
    assert_eq!(
        style_for_capture(&light, "variable").and_then(|style| style.foreground),
        Some(HighlightColor::Rgb {
            red: 0x24,
            green: 0x25,
            blue: 0x29,
        })
    );
}

#[test]
fn vi_theme_prefers_the_live_pane_then_the_spawned_terminal_theme() {
    assert_eq!(
        selected_vi_theme(
            Some("One Light".to_owned()),
            Some("Dracula".to_owned()),
            Some("One Dark".to_owned()),
        )
        .as_deref(),
        Some("Dracula")
    );
    assert_eq!(
        selected_vi_theme(
            Some("One Light".to_owned()),
            None,
            Some("One Dark".to_owned()),
        )
        .as_deref(),
        Some("One Dark")
    );
}

#[test]
fn highlights_a_rust_file_with_zed_queries_and_theme_styles() {
    let mut highlighter = highlighter(&["keyword", "function"]);

    let spans = highlighter.highlight_path(Some(Path::new("main.rs")), b"fn main() {}");

    assert!(spans.iter().any(|span| span.start == 0 && span.end == 2));
    assert!(spans.iter().any(|span| span.start == 3 && span.end == 7));
}

#[test]
fn visible_preview_remaps_and_clips_spans_to_the_viewport() {
    let mut highlighter = highlighter(&["keyword", "function"]);
    let source = b"let prefix = 1;\nfn main() {}\nlet suffix = 2;\n";
    let visible_start = source
        .windows(b"fn main() {}".len())
        .position(|line| line == b"fn main() {}")
        .expect("find visible Rust function");
    let visible_end = visible_start + b"fn main() {}".len();

    let spans = highlighter.highlight_visible_path(
        Some(Path::new("main.rs")),
        source,
        visible_start..visible_end,
    );

    assert!(!spans.is_empty());
    assert!(spans.iter().all(|span| visible_start <= span.start
        && span.start < span.end
        && span.end <= visible_end));
    assert!(
        spans
            .iter()
            .any(|span| &source[span.start..span.end] == b"fn")
    );
    assert!(
        spans
            .iter()
            .all(|span| &source[span.start..span.end] != b"let")
    );
}

#[test]
fn visible_preview_uses_bounded_line_aligned_context() {
    let mut source = Vec::new();
    for _ in 0..(SYNTAX_PREVIEW_CONTEXT_BYTES + 1024) {
        source.extend_from_slice(b"x\n");
    }
    let visible_start = source.len() / 2;
    let visible_range = visible_start..visible_start + 1;
    let context = syntax_preview_context_range(&source, visible_range.clone());

    assert!(context.start <= visible_range.start);
    assert!(context.end >= visible_range.end);
    assert!(context.start == 0 || source[context.start - 1] == b'\n');
    assert!(context.end == source.len() || source[context.end - 1] == b'\n');
    assert!(visible_range.start - context.start <= SYNTAX_PREVIEW_CONTEXT_BYTES + 1);
    assert!(context.end - visible_range.end <= SYNTAX_PREVIEW_CONTEXT_BYTES + 1);
    assert!(context.len() < source.len());
}

#[test]
fn compiles_queries_only_for_the_selected_language() {
    let grammars = grammar_set(&["keyword", "function"]);
    let mut highlighter = ZedSyntaxHighlighter::new(Arc::clone(&grammars));

    assert_eq!(grammars.loaded_configuration_count(), 0);

    let _ = highlighter.highlight_path(Some(Path::new("main.rs")), b"fn main() {}\n");
    let rust_index = grammars
        .language_index_for_name("rust")
        .expect("Rust is a native grammar");

    assert_eq!(grammars.loaded_configuration_count(), 1);
    assert!(grammars.has_configuration(rust_index));
}

#[test]
fn background_highlighter_returns_the_current_revision() {
    let mut highlighter = BackgroundZedSyntaxHighlighter::new(
        Some(PathBuf::from("main.rs")),
        grammar_set(&["keyword", "function"]),
    )
    .expect("start syntax worker");
    let source = b"fn main() {}\n";

    assert!(highlighter.highlight(source).is_empty());
    let preview = highlighter
        .highlight_visible(source, 0..source.len())
        .expect("create visible syntax preview");
    assert!(preview.iter().any(|span| span.start == 0 && span.end == 2));
    let deadline = Instant::now() + Duration::from_secs(5);
    let spans = loop {
        if let Some(spans) = highlighter.poll() {
            break spans;
        }
        assert!(Instant::now() < deadline, "syntax worker did not respond");
        std::thread::sleep(Duration::from_millis(10));
    };

    assert!(spans.iter().any(|span| span.start == 0 && span.end == 2));
}

#[test]
fn background_highlighter_recolors_an_unchanged_buffer_after_a_theme_update() {
    let (updates, receiver) = mpsc::channel();
    let watcher = SyntaxThemeWatcher::from_receiver(receiver);
    let grammars = GrammarSet::new_with_theme_watcher(
        SyntaxThemeHandle::ready(syntax_theme(&["keyword", "function"])),
        Some(watcher),
    )
    .expect("load Zed grammars");
    let mut highlighter =
        BackgroundZedSyntaxHighlighter::new(Some(PathBuf::from("main.rs")), Arc::clone(&grammars))
            .expect("start syntax worker");
    let source = b"fn main() {}\n";

    assert!(highlighter.highlight(source).is_empty());
    let initial = wait_for_syntax_result(&mut highlighter);
    assert_eq!(
        foreground_for_range(&initial, 0..2),
        Some(to_terminal_color(red()))
    );

    let blue_theme = Arc::new(SyntaxTheme::new([
        (
            "keyword".to_owned(),
            ZedHighlightStyle {
                color: Some(blue()),
                ..Default::default()
            },
        ),
        (
            "function".to_owned(),
            ZedHighlightStyle {
                color: Some(blue()),
                ..Default::default()
            },
        ),
    ]));
    updates.send(blue_theme).expect("send changed syntax theme");

    let recolored = wait_for_syntax_result(&mut highlighter);
    assert_eq!(
        foreground_for_range(&recolored, 0..2),
        Some(to_terminal_color(blue()))
    );
}

fn wait_for_syntax_result(highlighter: &mut BackgroundZedSyntaxHighlighter) -> Vec<HighlightSpan> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(spans) = highlighter.poll() {
            return spans;
        }
        assert!(Instant::now() < deadline, "syntax worker did not respond");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn foreground_for_range(spans: &[HighlightSpan], range: Range<usize>) -> Option<HighlightColor> {
    spans
        .iter()
        .find(|span| span.start == range.start && span.end == range.end)
        .and_then(|span| span.style.foreground)
}

#[test]
fn background_highlighter_highlights_batch_files() {
    let mut highlighter = BackgroundZedSyntaxHighlighter::new(
        Some(PathBuf::from("script.cmd")),
        grammar_set(&["keyword", "function", "string", "property"]),
    )
    .expect("start syntax worker");
    let source = b"@echo off\nset VAR=value\n";

    assert!(highlighter.highlight(source).is_empty());
    let deadline = Instant::now() + Duration::from_secs(5);
    let spans = loop {
        if let Some(spans) = highlighter.poll() {
            break spans;
        }
        assert!(Instant::now() < deadline, "syntax worker did not respond");
        std::thread::sleep(Duration::from_millis(10));
    };

    // Should highlight @echo off as keyword
    assert!(
        spans.iter().any(|span| span.start == 0 && span.end >= 3),
        "expected @echo to be highlighted, got spans: {:?}",
        spans
    );
}

#[test]
fn selects_shell_syntax_from_a_shebang_without_a_suffix() {
    let grammars = grammar_set(&["keyword"]);
    let source = b"#!/bin/bash\nif true; then\n";

    assert_eq!(
        grammars.language_index(Some(Path::new("script")), source),
        grammars.language_index_for_name("bash")
    );
}

#[test]
fn only_considers_the_first_line_for_shebang_detection() {
    let grammars = grammar_set(&["keyword"]);

    assert_eq!(
        grammars.language_index(
            Some(Path::new("script")),
            b"plain text\n#!/bin/bash\nif true; then\n",
        ),
        None
    );
}

#[test]
fn prefers_jsonc_for_zeds_special_jsonc_file_names() {
    let grammars = grammar_set(&["comment"]);

    assert_eq!(
        grammars.language_index(
            Some(Path::new("tsconfig.json")),
            b"{ // comments are valid here\n}\n",
        ),
        grammars.language_index_for_name("jsonc")
    );
}

#[test]
fn highlights_markdown_fenced_code() {
    let mut highlighter = highlighter(&["title", "keyword"]);
    let source = b"# Heading\n\n```rust\nfn main() {}\n```\n";

    let spans = highlighter.highlight_path(Some(Path::new("README.md")), source);

    assert!(spans.iter().any(|span| span.start == 0 && span.end >= 2));
    assert!(spans.iter().any(|span| span.start == 19 && span.end >= 21));
}

#[test]
fn highlights_jsonc_tsx_and_git_commits() {
    let mut highlighter = highlighter(&["comment", "keyword", "markup"]);

    for (path, source, token) in [
        (
            "tsconfig.json",
            b"{ // comment\n}\n".as_slice(),
            b"// comment".as_slice(),
        ),
        (
            "component.tsx",
            b"const Component = () => <main />;\n".as_slice(),
            b"const".as_slice(),
        ),
        (
            "COMMIT_EDITMSG",
            b"feat: add syntax highlighting\n".as_slice(),
            b"feat: add syntax highlighting".as_slice(),
        ),
    ] {
        let spans = highlighter.highlight_path(Some(Path::new(path)), source);
        assert!(
            spans
                .iter()
                .any(|span| source[span.start..span.end] == *token),
            "expected {path} to highlight {token:?}; got {spans:?}",
        );
    }
}

#[test]
fn highlights_toml_and_makefiles_from_extension_grammars() {
    let mut highlighter = highlighter(&[
        "comment",
        "function",
        "keyword",
        "number",
        "operator",
        "property",
        "string",
        "string.special.path",
        "string.special.symbol",
        "type",
    ]);

    for (path, source, token) in [
        (
            "Cargo.toml",
            b"[package]\nname = \"zetta\"\nversion = 1\n".as_slice(),
            b"package".as_slice(),
        ),
        (
            "Makefile",
            b"CC := cc\nall: app\n\t$(CC) main.c -o app\n".as_slice(),
            b"CC".as_slice(),
        ),
        (
            "script.cmd",
            b"@echo off\nset VAR=value\n".as_slice(),
            b"echo".as_slice(),
        ),
    ] {
        eprintln!("DEBUG test: highlighting {path}");
        let spans = highlighter.highlight_path(Some(Path::new(path)), source);
        eprintln!("DEBUG test: got spans = {spans:?}");
        assert!(
            spans.iter().any(|span| {
                let span_text = &source[span.start..span.end];
                span_text.windows(token.len()).any(|window| window == token)
            }),
            "expected {path} to highlight {token:?}; got {spans:?}",
        );
    }
}

#[test]
fn recognizes_common_makefile_and_toml_paths() {
    let grammars = grammar_set(&[]);

    for path in ["Makefile", "GNUmakefile", "build.mk"] {
        assert_eq!(
            grammars.language_index(Some(Path::new(path)), b"all:\n"),
            grammars.language_index_for_name("makefile"),
            "expected {path} to use Makefile syntax",
        );
    }
    assert_eq!(
        grammars.language_index(Some(Path::new("Cargo.toml")), b"[package]\n"),
        grammars.language_index_for_name("toml"),
    );
    // Test batch file detection
    for path in ["script.bat", "script.cmd", "build-windows.cmd"] {
        assert_eq!(
            grammars.language_index(Some(Path::new(path)), b"@echo off\n"),
            grammars.language_index_for_name("batch"),
            "expected {path} to use Batch syntax",
        );
    }
}

#[test]
fn resolves_capture_styles_with_zeds_prefix_rules() {
    let theme = SyntaxTheme::new([(
        "operator".to_owned(),
        ZedHighlightStyle {
            color: Some(red()),
            ..Default::default()
        },
    )]);

    assert!(style_for_capture(&theme, "operator.assignment").is_some());
    assert!(style_for_capture(&theme, "keyword.operator.regex").is_none());
}

#[test]
fn installs_zed_highlighting_in_the_bundled_editor() {
    let mut editor = Editor::from_bytes(b"fn main() {}\n", Some(PathBuf::from("main.rs")), false);
    install(
        &mut editor,
        Some(new_shared(syntax_theme(&["keyword", "function"])).expect("load Zed grammars")),
    );

    assert!(
        editor
            .syntax_highlights()
            .expect("syntax highlighter installed")
            .iter()
            .any(|span| span.start == 0 && span.end == 2)
    );
}

#[test]
fn embeds_the_native_grammar_configs_and_queries_without_zeds_rust_source() {
    assert!(GrammarAssets::get("rust/config.toml").is_some());
    assert!(GrammarAssets::get("rust/highlights.scm").is_some());
    assert!(GrammarAssets::get("grammars.rs").is_none());

    // Check batch grammar extension assets
    assert!(
        ExtensionGrammarAssets::get("batch/config.toml").is_some(),
        "batch config.toml not embedded"
    );
    assert!(
        ExtensionGrammarAssets::get("batch/highlights.scm").is_some(),
        "batch highlights.scm not embedded"
    );
}

#[test]
fn scanned_capture_names_cover_every_compiled_grammar() {
    // Configurations are shared across threads and so are configured exactly
    // once, against a table scanned out of the query text. Any capture the
    // scanner misses would silently lose its theme style.
    let grammars = grammar_set(&[]);
    let known: HashSet<&str> = grammars.capture_names.iter().map(String::as_str).collect();

    for language_index in 0..grammars.languages.len() {
        let name = grammars.languages[language_index].name;
        let configuration = grammars
            .configuration(language_index)
            .unwrap_or_else(|error| panic!("compiling {name:?}: {error:#}"));
        for capture_name in configuration.names() {
            assert!(
                known.contains(capture_name),
                "{name:?} declares capture {capture_name:?}, which the scanner missed",
            );
        }
    }
}

#[test]
fn every_embedded_first_line_pattern_compiles() {
    // The patterns are compiled lazily off the startup path, so a malformed one
    // would otherwise only surface as a silently dropped shebang at runtime.
    let grammars = grammar_set(&[]);
    let declared = grammars
        .languages
        .iter()
        .filter(|language| language.first_line_pattern.is_some())
        .count();

    assert!(declared > 0, "expected embedded first-line patterns");
    assert_eq!(grammars.first_line_patterns().len(), declared);
}

#[test]
fn query_capture_scanning_skips_comments_and_anonymous_nodes() {
    let query = r#"
; a comment with @not.a.capture
("@" @operator)
((identifier) @variable.special
  (#match? @variable.special "^@[a-z]+$"))
"#;

    assert_eq!(
        query_capture_names(query),
        ["operator", "variable.special", "variable.special"]
    );
}

#[test]
fn keeps_the_exact_zed_native_grammar_registry() {
    let grammar_ids: Vec<_> = native_grammars()
        .into_iter()
        .map(|(grammar_id, _)| grammar_id)
        .collect();
    assert_eq!(
        grammar_ids,
        [
            "bash",
            "c",
            "cpp",
            "css",
            "diff",
            "go",
            "gomod",
            "gowork",
            "jsdoc",
            "json",
            "jsonc",
            "markdown",
            "markdown-inline",
            "python",
            "regex",
            "rust",
            "tsx",
            "typescript",
            "yaml",
            "gitcommit",
        ]
    );
}

#[test]
fn registers_extension_grammars_separately_from_zeds_native_registry() {
    let grammar_ids: Vec<_> = extension_grammars()
        .into_iter()
        .map(|(grammar_id, _)| grammar_id)
        .collect();
    assert_eq!(grammar_ids, ["makefile", "toml", "powershell", "batch"]);
    assert!(
        native_grammars()
            .into_iter()
            .all(|(grammar_id, _)| !grammar_ids.contains(&grammar_id))
    );
}
