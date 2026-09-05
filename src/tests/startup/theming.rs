use super::*;

#[test]
fn defaults_to_light_theme_without_overriding_configuration() {
    assert_eq!(selected_theme_name(None), "One Light");
    assert_eq!(selected_theme_name(Some("One Dark")), "One Dark");
    assert_eq!(selected_dark_theme_name(None), "One Dark");
    assert_eq!(selected_dark_theme_name(Some("Dracula")), "Dracula");
}

#[gpui::test]
fn selected_theme_follows_the_system_appearance_without_cross_mode_fallback(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        theme::init(theme::LoadThemes::All(Box::new(ZettaAssets)), cx);
        let mut config = Config::defaults(None, None);
        config.theme = Some("Solarized Light".to_owned());
        config.dark_theme = Some("Solarized Dark".to_owned());

        *SystemAppearance::global_mut(cx) = SystemAppearance(theme::Appearance::Light);
        assert_eq!(
            selected_theme_name_for_appearance(&config, cx),
            "Solarized Light"
        );

        *SystemAppearance::global_mut(cx) = SystemAppearance(theme::Appearance::Dark);
        assert_eq!(
            selected_theme_name_for_appearance(&config, cx),
            "Solarized Dark"
        );

        config.dark_theme = None;
        assert_eq!(selected_theme_name_for_appearance(&config, cx), "One Dark");
    });
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

/// The bake sweep runs at startup before the first frame and again on every
/// configuration reload, over every registered theme, and a `Theme` clone
/// carries its colours and its whole syntax map. This pins that a second sweep
/// finds nothing to do — which is what makes skipping the clone a real saving
/// rather than a hopeful one.
#[gpui::test]
fn baking_theme_overrides_is_a_no_op_the_second_time(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        theme::init(theme::LoadThemes::All(Box::new(ZettaAssets)), cx);
        let registry = ThemeRegistry::global(cx);

        let unbaked = registry
            .list_names()
            .into_iter()
            .filter_map(|name| registry.get(&name).ok())
            .filter(|theme| !zetta_theme_overrides_are_baked(theme))
            .count();
        assert!(
            unbaked > 0,
            "the bundled themes should need baking before the first sweep"
        );

        bake_zetta_theme_overrides(&registry);
        let still_unbaked = registry
            .list_names()
            .into_iter()
            .filter_map(|name| registry.get(&name).ok())
            .filter(|theme| !zetta_theme_overrides_are_baked(theme))
            .count();
        assert_eq!(
            still_unbaked, 0,
            "every theme should read as baked after one sweep"
        );

        // The scrollbar colours a sweep writes must survive a second sweep, or
        // "already baked" would be a lie and every reload would clone again.
        let sampled = registry.get("One Dark").expect("One Dark is bundled");
        let before = sampled.styles.colors.scrollbar_thumb_background;
        bake_zetta_theme_overrides(&registry);
        let after = registry.get("One Dark").expect("One Dark is bundled");
        assert_eq!(after.styles.colors.scrollbar_thumb_background, before);
    });
}
