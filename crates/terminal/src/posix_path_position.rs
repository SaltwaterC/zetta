//! Reading a `path:row:column` target whose path is POSIX text, on any host.
//!
//! `PathWithPosition::parse_str` strips the position by rebuilding the path
//! with `Path::with_file_name`, which joins the file name with the host's
//! separator. On Windows that turns `/home/user/src/main.rs:12` into
//! `/home/user/src\main.rs`, and the WSL and Cygwin translations rightly refuse
//! a POSIX component that holds a backslash. The path of such a target is a
//! prefix of the text it was written in, so the text is what this keeps.

use std::path::{Path, PathBuf};

use util::paths::PathWithPosition;

/// Parses `text` as [`PathWithPosition::parse_str`] does, keeping the path as
/// it was written rather than as the host joins it.
pub fn parse_posix_path_with_position(text: &str) -> PathWithPosition {
    let mut parsed = PathWithPosition::parse_str(text);
    if let Some(written) = written_path(text, &parsed.path) {
        parsed.path = PathBuf::from(written);
    }
    parsed
}

/// The prefix of `text` that `parsed` was read from, when the two differ at
/// most in a `/` the host rewrote as `\`. Anything else, including a backslash
/// the text really holds, leaves the parsed path alone.
fn written_path<'a>(text: &'a str, parsed: &Path) -> Option<&'a str> {
    let parsed = parsed.to_str()?;
    let written = text.get(..parsed.len())?;
    written
        .bytes()
        .zip(parsed.bytes())
        .all(|(written, parsed)| written == parsed || (written == b'/' && parsed == b'\\'))
        .then_some(written)
}

#[cfg(test)]
#[path = "tests/posix_path_position.rs"]
mod tests;
