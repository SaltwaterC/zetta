use super::*;
use theme::ThemeRegistry;

#[test]
fn bundled_solarized_themes_load_from_embedded_assets() {
    let registry = ThemeRegistry::new(Box::new(ZettaAssets));
    theme_settings::load_bundled_themes(&registry);

    assert!(registry.get("Solarized Dark").is_ok());
    assert!(registry.get("Solarized Light").is_ok());
}
