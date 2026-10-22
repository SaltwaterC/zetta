use super::*;

fn family_names(families: &[ThemeFamily]) -> Vec<String> {
    families
        .iter()
        .map(|family| family.name.to_string())
        .collect()
}

#[test]
fn the_prefetched_bundled_themes_are_the_ones_parsed_in_place() {
    let mut prefetch = StartupPrefetch {
        bundled_themes: spawn("test theme prefetch", || {
            theme_settings::parse_bundled_themes(&ZettaAssets)
        }),
        user_themes: None,
        fonts: None,
    };

    let prefetched = prefetch.bundled_themes().unwrap();

    assert!(!prefetched.is_empty());
    assert_eq!(
        family_names(&prefetched),
        family_names(&theme_settings::parse_bundled_themes(&ZettaAssets))
    );
    assert!(prefetch.bundled_themes().is_none());
}

#[test]
fn the_prefetched_user_themes_are_handed_over() {
    let mut prefetch = StartupPrefetch {
        bundled_themes: None,
        user_themes: spawn("test user theme prefetch", || Ok(Vec::new())),
        fonts: None,
    };

    assert!(prefetch.user_themes().unwrap().is_empty());
    assert!(prefetch.bundled_themes().is_none());
}

#[test]
fn the_prefetched_fonts_are_the_ones_read_in_place() {
    let mut prefetch = StartupPrefetch {
        bundled_themes: None,
        user_themes: None,
        fonts: spawn("test font prefetch", || ZettaAssets.embedded_fonts()),
    };

    let prefetched = prefetch.fonts().unwrap();

    assert!(!prefetched.is_empty());
    assert_eq!(prefetched, ZettaAssets.embedded_fonts().unwrap());
    // Without the thread, the fonts are read here.
    assert_eq!(prefetch.fonts().unwrap(), prefetched);
}

#[test]
fn a_panicked_prefetch_leaves_the_work_to_the_caller() {
    let handle = spawn("test panicking prefetch", || -> Vec<ThemeFamily> {
        panic!("prefetch failed")
    })
    .unwrap();

    assert!(join(handle).is_none());
}
