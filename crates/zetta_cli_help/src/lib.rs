//! What every Zetta command-line interface shares: the help-table formatter
//! it prints its `Commands`, `Operations` and `Options` sections with, and the
//! check that refuses an option given twice.
//!
//! The crates under `crates/` are separate Cargo workspaces, so before this
//! crate existed each one carried its own copy of both. A path dependency
//! shares them without publishing anything, and keeps one test for the
//! alignment rules every help screen relies on and one for the wording every
//! parser refuses a repeated option with.

/// Formats `(label, description)` rows as an aligned two-column table.
///
/// Each row is indented two spaces, the description column starts two spaces
/// past the longest label, continuation lines of a multi-line description align
/// with that column, and no line carries trailing whitespace.
pub fn format_help_table<'a>(rows: impl AsRef<[(&'a str, &'a str)]>) -> String {
    let rows = rows
        .as_ref()
        .iter()
        .map(|&(label, description)| (label.trim_end(), description))
        .collect::<Vec<_>>();
    let label_width = rows
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(0);
    rows.into_iter()
        .map(|(label, description)| {
            let mut lines = description.split('\n');
            let first_line = lines.next().unwrap_or("").trim_end();
            let mut formatted = String::new();
            formatted.push_str("  ");
            formatted.push_str(label);
            formatted.push_str(&" ".repeat(label_width - label.chars().count()));
            if !first_line.is_empty() {
                formatted.push_str("  ");
                formatted.push_str(first_line);
            }
            for line in lines {
                formatted.push('\n');
                let line = line.trim_end();
                if !line.is_empty() {
                    formatted.push_str("  ");
                    formatted.push_str(&" ".repeat(label_width));
                    formatted.push_str("  ");
                    formatted.push_str(line);
                }
            }
            formatted
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Sets the switch `flag`, refusing `option` given twice.
pub fn set_flag(flag: &mut bool, option: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!*flag, "{option} may only be specified once");
    *flag = true;
    Ok(())
}

/// Stores the value `value` reads in `slot`, refusing `option` given twice.
/// That is checked first, so a repeated option is reported as one even when
/// its value is missing or invalid too.
pub fn set_once<T>(
    slot: &mut Option<T>,
    option: &str,
    value: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<()> {
    anyhow::ensure!(slot.is_none(), "{option} may only be specified once");
    *slot = Some(value()?);
    Ok(())
}

#[cfg(test)]
#[path = "tests/lib.rs"]
mod tests;
