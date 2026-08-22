use super::*;
use terminal::console_palette_for_theme;
use theme::ThemeRegistry;

#[test]
fn bundled_solarized_themes_load_from_embedded_assets() {
    let registry = ThemeRegistry::new(Box::new(ZettaAssets));
    theme_settings::load_bundled_themes(&registry);

    assert!(registry.get("Solarized Dark").is_ok());
    assert!(registry.get("Solarized Light").is_ok());
}

#[test]
fn bundled_light_and_dark_terminal_themes_map_to_win32_palettes() {
    let registry = ThemeRegistry::new(Box::new(ZettaAssets));
    theme_settings::load_bundled_themes(&registry);

    let dark = console_palette_for_theme(&registry.get("Solarized Dark").unwrap());
    // These bytes intentionally include the same floating-point truncation as
    // the terminal's OSC color replies.
    assert_eq!(dark.colors[0], [0x06, 0x35, 0x42]);
    assert_eq!(dark.colors[8], [0x00, 0x2b, 0x35]);
    assert_eq!(dark.foreground_index, 12);
    assert_eq!(dark.background_index, 8);

    let light = console_palette_for_theme(&registry.get("Solarized Light").unwrap());
    assert_eq!(light.colors[7], [0xee, 0xe8, 0xd5]);
    assert_eq!(light.colors[15], [0xfd, 0xf6, 0xe3]);
    assert_eq!(light.foreground_index, 11);
    assert_eq!(light.background_index, 15);
}

#[test]
fn bundled_profile_icons_load_from_embedded_assets() {
    for icon in ["zetta", "bash", "zsh", "fish"] {
        let path = format!("icons/profile/{icon}.svg");
        assert!(ZettaAssets.load(&path).unwrap().is_some(), "missing {path}");
    }
    assert!(
        ZettaAssets.load("icons/profile/tux.png").unwrap().is_some(),
        "missing icons/profile/tux.png"
    );
}
