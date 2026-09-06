use super::*;

fn environment(
    entries: &[(&str, &str)],
) -> impl Fn(&str) -> Result<String, std::env::VarError> + use<> {
    let entries = entries
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect::<Vec<_>>();
    move |name| {
        entries
            .iter()
            .find(|(entry, _)| entry == name)
            .map(|(_, value)| value.clone())
            .ok_or(std::env::VarError::NotPresent)
    }
}

#[test]
fn common_utf8_locale_spellings_are_accepted() {
    for value in ["en_US.UTF-8", "en_US.utf8", "C.UTF8", "de_DE.UTF-8@euro"] {
        assert!(is_utf8(value), "{value}");
    }
}

#[test]
fn locale_precedence_and_empty_values_match_posix() {
    let selected = current(environment(&[
        ("LC_ALL", ""),
        ("LC_CTYPE", "C"),
        ("LANG", "en_US.UTF-8"),
    ]))
    .unwrap();
    assert_eq!(selected.variable, "LC_CTYPE");
    assert!(!is_utf8(&selected.value));
}
