use super::*;
use busy_v::Editor;
use gpui::{HighlightStyle as ZedHighlightStyle, red};
use std::path::PathBuf;
use std::time::{Duration, Instant};

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
fn highlights_a_rust_file_with_zed_queries_and_theme_styles() {
    let theme = syntax_theme(&["keyword", "function"]);
    let mut highlighter = ZedSyntaxHighlighter::new(theme).expect("load Zed grammars");

    let spans = highlighter.highlight_path(Some(Path::new("main.rs")), b"fn main() {}");

    assert!(spans.iter().any(|span| span.start == 0 && span.end == 2));
    assert!(spans.iter().any(|span| span.start == 3 && span.end == 7));
}

#[test]
fn compiles_queries_only_for_the_selected_language() {
    let theme = syntax_theme(&["keyword", "function"]);
    let mut highlighter = ZedSyntaxHighlighter::new(theme).expect("load grammar metadata");

    assert!(
        highlighter
            .languages
            .iter()
            .all(|language| language.configuration.is_none())
    );

    let _ = highlighter.highlight_path(Some(Path::new("main.rs")), b"fn main() {}\n");
    let loaded_configurations = highlighter
        .languages
        .iter()
        .filter(|language| language.configuration.is_some())
        .count();
    let rust_index = *highlighter
        .language_names
        .get("rust")
        .expect("Rust is a native grammar");

    assert_eq!(loaded_configurations, 1);
    assert!(highlighter.languages[rust_index].configuration.is_some());
}

#[test]
fn background_highlighter_returns_the_current_revision() {
    let mut highlighter = BackgroundZedSyntaxHighlighter::new(Some(PathBuf::from("main.rs")), None)
        .expect("start syntax worker");
    let source = b"fn main() {}\n";

    assert!(highlighter.highlight(source).is_empty());
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
fn selects_shell_syntax_from_a_shebang_without_a_suffix() {
    let theme = syntax_theme(&["keyword"]);
    let highlighter = ZedSyntaxHighlighter::new(theme).expect("load Zed grammars");
    let source = b"#!/bin/bash\nif true; then\n";

    assert_eq!(
        highlighter.language_index(Some(Path::new("script")), source),
        highlighter.language_names.get("bash").copied()
    );
}

#[test]
fn only_considers_the_first_line_for_shebang_detection() {
    let highlighter =
        ZedSyntaxHighlighter::new(syntax_theme(&["keyword"])).expect("load Zed grammars");

    assert_eq!(
        highlighter.language_index(
            Some(Path::new("script")),
            b"plain text\n#!/bin/bash\nif true; then\n",
        ),
        None
    );
}

#[test]
fn prefers_jsonc_for_zeds_special_jsonc_file_names() {
    let highlighter =
        ZedSyntaxHighlighter::new(syntax_theme(&["comment"])).expect("load Zed grammars");

    assert_eq!(
        highlighter.language_index(
            Some(Path::new("tsconfig.json")),
            b"{ // comments are valid here\n}\n",
        ),
        highlighter.language_names.get("jsonc").copied()
    );
}

#[test]
fn highlights_markdown_fenced_code() {
    let theme = syntax_theme(&["title", "keyword"]);
    let mut highlighter = ZedSyntaxHighlighter::new(theme).expect("load Zed grammars");
    let source = b"# Heading\n\n```rust\nfn main() {}\n```\n";

    let spans = highlighter.highlight_path(Some(Path::new("README.md")), source);

    assert!(spans.iter().any(|span| span.start == 0 && span.end >= 2));
    assert!(spans.iter().any(|span| span.start == 19 && span.end >= 21));
}

#[test]
fn highlights_jsonc_tsx_and_git_commits() {
    let mut highlighter =
        ZedSyntaxHighlighter::new(syntax_theme(&["comment", "keyword", "markup"]))
            .expect("load Zed grammars");

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
    let mut highlighter = ZedSyntaxHighlighter::new(syntax_theme(&[
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
    ]))
    .expect("load Zed and extension grammars");

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
fn recognizes_common_makefile_and_toml_paths() {
    let highlighter =
        ZedSyntaxHighlighter::new(syntax_theme(&[])).expect("load extension grammars");

    for path in ["Makefile", "GNUmakefile", "build.mk"] {
        assert_eq!(
            highlighter.language_index(Some(Path::new(path)), b"all:\n"),
            highlighter.language_names.get("makefile").copied(),
            "expected {path} to use Makefile syntax",
        );
    }
    assert_eq!(
        highlighter.language_index(Some(Path::new("Cargo.toml")), b"[package]\n"),
        highlighter.language_names.get("toml").copied(),
    );
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
    assert_eq!(grammar_ids, ["makefile", "toml"]);
    assert!(
        native_grammars()
            .into_iter()
            .all(|(grammar_id, _)| !grammar_ids.contains(&grammar_id))
    );
}
