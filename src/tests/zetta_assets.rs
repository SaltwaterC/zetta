use super::*;
use theme::ThemeRegistry;

#[test]
fn bundled_solarized_themes_load_from_embedded_assets() {
    let registry = ThemeRegistry::new(Box::new(ZettaAssets));
    theme_settings::load_bundled_themes(&registry);

    assert!(registry.get("Solarized Dark").is_ok());
    assert!(registry.get("Solarized Light").is_ok());
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
