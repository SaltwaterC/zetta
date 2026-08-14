use super::*;

#[test]
fn shell_integration_setup_message_explains_how_to_enable_a_new_configuration() {
    let message = shell_integration_configuration_message(&ShellIntegrationConfiguration::Written(
        PathBuf::from("/home/example/.zshrc"),
    ));

    assert!(message.contains("Start a new shell or reload this file"));
}

#[test]
fn process_quits_only_without_windows_or_dormant_session_runners() {
    assert!(should_quit_after_window_closed(0, 0));
    assert!(!should_quit_after_window_closed(0, 1));
    assert!(!should_quit_after_window_closed(1, 0));
}

#[test]
fn application_shutdown_is_managed_by_the_session_runner() {
    assert_eq!(zetta_quit_mode(), gpui::QuitMode::Explicit);
}

#[test]
fn terminal_rendering_profiler_launches_the_current_executable() {
    let executable = Path::new(if cfg!(windows) {
        r"C:\tools\zetta.exe"
    } else {
        "/usr/local/bin/zetta"
    });
    let config = terminal_rendering_profile_config(executable, PerformanceWorkload::Standard);

    assert_eq!(config.profiles.len(), 1);
    assert_eq!(config.default_profile, 0);
    assert_eq!(
        config.profiles[0].command,
        Shell::WithArguments {
            program: executable.to_string_lossy().into_owned(),
            args: vec![
                "benchmark".to_owned(),
                "--terminal-render-workload".to_owned(),
            ],
            title_override: Some("Terminal rendering profiler".to_owned()),
        }
    );
}

#[test]
fn checkerboard_profiler_launches_the_background_workload() {
    let executable = Path::new("/path/to/zetta");
    let config =
        terminal_rendering_profile_config(executable, PerformanceWorkload::CheckerboardBackground);

    assert_eq!(
        config.profiles[0].command,
        Shell::WithArguments {
            program: executable.to_string_lossy().into_owned(),
            args: vec![
                "benchmark".to_owned(),
                "--terminal-checkerboard-workload".to_owned(),
            ],
            title_override: Some("Terminal rendering profiler".to_owned()),
        }
    );
}

#[test]
fn checkerboard_background_changes_every_cell_on_each_frame() {
    assert_ne!(
        checkerboard_background(0, 0, 0),
        checkerboard_background(0, 0, 1)
    );
    assert_ne!(
        checkerboard_background(0, 0, 0),
        checkerboard_background(0, 1, 0)
    );
    assert_eq!(
        checkerboard_background(0, 0, 0),
        checkerboard_background(0, 0, 2)
    );
}

#[test]
fn sparse_update_profiler_launches_the_sparse_workload() {
    let executable = Path::new("/path/to/zetta");
    let config = terminal_rendering_profile_config(executable, PerformanceWorkload::SparseUpdates);

    assert_eq!(
        config.profiles[0].command,
        Shell::WithArguments {
            program: executable.to_string_lossy().into_owned(),
            args: vec![
                "benchmark".to_owned(),
                "--terminal-sparse-update-workload".to_owned(),
            ],
            title_override: Some("Terminal rendering profiler".to_owned()),
        }
    );
}

#[test]
fn unchanged_user_themes_are_not_reloaded() {
    let themes_dir = env::temp_dir().join(format!(
        "zetta-theme-cache-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&themes_dir).unwrap();
    let theme_path = themes_dir.join("test.json");
    fs::write(&theme_path, "one").unwrap();
    let mut cache = HashMap::new();

    assert_eq!(
        changed_theme_files(&themes_dir, &mut cache).unwrap(),
        std::slice::from_ref(&theme_path)
    );
    assert!(
        changed_theme_files(&themes_dir, &mut cache)
            .unwrap()
            .is_empty()
    );

    fs::write(&theme_path, "a longer theme").unwrap();
    assert_eq!(
        changed_theme_files(&themes_dir, &mut cache).unwrap(),
        [theme_path]
    );
    fs::remove_dir_all(themes_dir).unwrap();
}

#[test]
fn defaults_to_light_theme_without_overriding_configuration() {
    assert_eq!(selected_theme_name(None), "One Light");
    assert_eq!(selected_theme_name(Some("One Dark")), "One Dark");
}

#[test]
fn linux_desktop_entry_matches_app_id() {
    let desktop_entry = include_str!("../../resources/linux/Zetta.desktop");
    assert!(desktop_entry.contains(&format!("\nIcon={ZETTA_APP_ID}\n")));
    assert!(desktop_entry.contains(&format!("\nStartupWMClass={ZETTA_APP_ID}\n")));
}

#[test]
fn normalizes_hyphenated_page_key_names() {
    let keymap = r#"{"ctrl-page-up":"zetta::NextTab","ctrl-page-down":"zetta::PreviousTab"}"#;
    assert_eq!(
        normalize_keymap_key_names(keymap),
        r#"{"ctrl-pageup":"zetta::NextTab","ctrl-pagedown":"zetta::PreviousTab"}"#
    );
}

#[test]
fn normalizes_keymap_aliases_for_runtime_loading() {
    let keymap =
        r#"[{"bindings":{"Ctrl+Shift+1":"zetta::NextTab","Ctrl+Shift+0":"zetta::PreviousTab"}}]"#;
    assert_eq!(
        normalize_keymap_key_names(keymap),
        r#"[{"bindings":{"ctrl-shift-1":"zetta::NextTab","ctrl-shift-0":"zetta::PreviousTab"}}]"#
    );
}

#[test]
fn normalizes_regular_keystrokes_to_lowercase() {
    let keymap =
        r#"[{"bindings":{"Ctrl+Alt+V":"zetta::PasteTrimmed","CTRL-SHIFT-T":"zetta::NewTab"}}]"#;
    assert_eq!(
        normalize_keymap_key_names(keymap),
        r#"[{"bindings":{"ctrl-alt-v":"zetta::PasteTrimmed","ctrl-shift-t":"zetta::NewTab"}}]"#
    );
}

/// The bundled themes the override tests build on. Solarized is Zetta's own
/// (`assets/themes/`), One Dark is Zed's, so between them these cover both
/// asset sources the registry loads from.
fn bundled_theme_registry() -> ThemeRegistry {
    let registry = ThemeRegistry::new(Box::new(ZettaAssets));
    theme_settings::load_bundled_themes(&registry);
    registry
}

#[test]
fn zetta_theme_overrides_restyle_scrollbars_from_text_colors() {
    let registry = bundled_theme_registry();
    let mut theme = registry.get("One Dark").unwrap().as_ref().clone();
    let untouched = theme.styles.colors.scrollbar_thumb_background;

    apply_zetta_theme_overrides(&mut theme);

    let colors = &theme.styles.colors;
    assert_eq!(
        colors.scrollbar_thumb_background,
        colors.text_muted.opacity(0.7)
    );
    assert_eq!(
        colors.scrollbar_thumb_hover_background,
        colors.text.opacity(0.85)
    );
    assert_eq!(
        colors.scrollbar_thumb_active_background,
        colors.text_accent.opacity(0.95)
    );
    assert_ne!(
        colors.scrollbar_thumb_background, untouched,
        "One Dark already ships Zetta's scrollbar color, so this test proves nothing"
    );
}

/// `bake_zetta_theme_overrides` re-sweeps the whole registry on every reload, so
/// running it over themes it has already overridden must not drift them.
#[test]
fn zetta_theme_overrides_are_idempotent() {
    let registry = bundled_theme_registry();
    let mut theme = registry.get("Solarized Light").unwrap().as_ref().clone();

    apply_zetta_theme_overrides(&mut theme);
    let once = theme.styles.colors.scrollbar_thumb_background;
    apply_zetta_theme_overrides(&mut theme);

    assert_eq!(theme.styles.colors.scrollbar_thumb_background, once);
}

#[test]
fn baking_theme_overrides_rewrites_every_registered_theme() {
    let registry = bundled_theme_registry();
    let names = registry.list_names();
    assert!(names.len() > 1, "expected the bundled themes to load");
    assert!(
        names.iter().any(|name| {
            let colors = &registry.get(name).unwrap().styles.colors;
            colors.scrollbar_thumb_background != colors.text_muted.opacity(0.7)
        }),
        "every bundled theme already matches the override; this test would pass vacuously"
    );

    bake_zetta_theme_overrides(&registry);

    for name in &names {
        let theme = registry.get(name).unwrap();
        let colors = &theme.styles.colors;
        assert_eq!(
            colors.scrollbar_thumb_background,
            colors.text_muted.opacity(0.7),
            "{name} kept its own scrollbar color"
        );
    }
}

/// Themes are looked up per tab per frame, so the overrides are baked into the
/// registry instead of being applied to each lookup's result. A lookup must
/// therefore hand back the *same* `Arc`, not a fresh clone.
#[test]
fn baked_theme_lookups_share_one_allocation() {
    let registry = bundled_theme_registry();
    bake_zetta_theme_overrides(&registry);

    assert!(Arc::ptr_eq(
        &registry.get("One Dark").unwrap(),
        &registry.get("One Dark").unwrap()
    ));
}
