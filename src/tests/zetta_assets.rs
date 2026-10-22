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
    for path in [
        "icons/profile/zetta.svg",
        "icons/profile/tux.png",
        "icons/profile/bash.png",
        "icons/profile/zsh.svg",
        "icons/profile/fish.svg",
    ] {
        assert!(ZettaAssets.load(path).unwrap().is_some(), "missing {path}");
    }
}

/// The derive excludes `assets/fonts`, so a font missing from
/// `UNCOMPRESSED_FONTS` would not be embedded at all.
#[test]
fn every_bundled_font_is_served_uncompressed() {
    fn ttf_files(directory: &std::path::Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                ttf_files(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "ttf") {
                found.push(path.to_str().unwrap().to_owned());
            }
        }
    }
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let mut on_disk = Vec::new();
    ttf_files(&assets.join("fonts"), &mut on_disk);
    let mut on_disk: Vec<String> = on_disk
        .iter()
        .map(|path| {
            std::path::Path::new(path)
                .strip_prefix(&assets)
                .unwrap()
                .to_str()
                .unwrap()
                .replace('\\', "/")
        })
        .collect();
    on_disk.sort();
    let mut served: Vec<&str> = UNCOMPRESSED_FONTS.iter().map(|(path, _)| *path).collect();
    served.sort();
    assert_eq!(served, on_disk);

    for path in served {
        let loaded = ZettaAssets.load(path).unwrap().unwrap();
        assert!(matches!(loaded, Cow::Borrowed(_)), "{path} was copied");
        assert_eq!(
            *loaded,
            std::fs::read(assets.join(path)).unwrap()[..],
            "{path}"
        );
    }
}

#[test]
fn embedded_fonts_include_zeds_and_zettas() {
    let fonts = ZettaAssets.embedded_fonts().unwrap();
    let listed = ZettaAssets
        .list("fonts/")
        .unwrap()
        .into_iter()
        .filter(|path| path.ends_with(".ttf"))
        .count();
    assert_eq!(fonts.len(), listed);
    assert!(listed > UNCOMPRESSED_FONTS.len(), "Zed's fonts are missing");
}
