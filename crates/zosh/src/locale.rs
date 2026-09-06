//! POSIX locale validation for the standalone client.

#[derive(Clone, Debug, PartialEq, Eq)]
struct Locale {
    variable: &'static str,
    value: String,
}

const PRECEDENCE: [&str; 3] = ["LC_ALL", "LC_CTYPE", "LANG"];

pub(crate) fn ensure_utf8() -> Result<(), String> {
    let locale = current(|name| std::env::var(name));
    if locale.as_ref().is_some_and(|locale| is_utf8(&locale.value)) {
        return Ok(());
    }
    Err(match locale {
        Some(locale) => format!(
            "zosh needs a UTF-8 locale to run.\n\nThe environment says {}={}, which does not name UTF-8. Try {}=C.UTF-8, or a locale ending in .UTF-8.",
            locale.variable, locale.value, locale.variable
        ),
        None => {
            "zosh needs a UTF-8 locale to run.\n\nNone of LC_ALL, LC_CTYPE or LANG ".to_owned()
                + "is set, which selects the C locale. Try LANG=C.UTF-8."
        }
    })
}

fn current(lookup: impl Fn(&str) -> Result<String, std::env::VarError>) -> Option<Locale> {
    PRECEDENCE.iter().find_map(|variable| {
        let value = lookup(variable).ok().filter(|value| !value.is_empty())?;
        Some(Locale { variable, value })
    })
}

fn is_utf8(locale: &str) -> bool {
    let charset = locale
        .rsplit_once('.')
        .map_or(locale, |(_, charset)| charset);
    let charset = charset.split('@').next().unwrap_or(charset);
    charset
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .map(|character| character.to_ascii_lowercase())
        .eq("utf8".chars())
}

#[cfg(test)]
#[path = "tests/locale.rs"]
mod tests;
