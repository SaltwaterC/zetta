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
    };

    assert!(prefetch.user_themes().unwrap().is_empty());
    assert!(prefetch.bundled_themes().is_none());
}

#[test]
fn a_panicked_prefetch_leaves_the_work_to_the_caller() {
    let handle = spawn("test panicking prefetch", || -> Vec<ThemeFamily> {
        panic!("prefetch failed")
    })
    .unwrap();

    assert!(join(handle).is_none());
}
