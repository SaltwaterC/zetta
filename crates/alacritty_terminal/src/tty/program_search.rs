//! Finding the executable a Windows command line names, without the current
//! directory.
//!
//! Zetta-authored; no upstream counterpart. `CreateProcessW` given no
//! application name searches for a bare program in the launcher's directory,
//! then *the calling process's current directory*, then the system directories
//! and `PATH`. The palette bootstrap runs with the pane's directory as its
//! current directory, so a `cmd.exe` planted in a directory a pane opens in
//! would run instead of the system one. This module performs the same search
//! with that step removed, and with relative or empty `PATH` entries skipped
//! because they name the current directory too, so the caller can pass the
//! result as `lpApplicationName` and leave Windows nothing to search.
//!
//! The search itself is plain path logic so it is tested on every host; only
//! the directories it searches come from Windows, in `tty::windows`. `zmux`'s
//! palette bootstrap uses the same entry points, so there is one policy.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The directories a bare program name is looked up in, in order.
#[derive(Clone, Debug, Default)]
pub struct SearchDirectories {
    /// The directory of the executable doing the launching.
    pub application: Option<PathBuf>,
    /// The system directory, the 16-bit system directory and the Windows
    /// directory, in the order `CreateProcessW` searches them.
    pub system: Vec<PathBuf>,
    /// The `PATH` value the child will be started with.
    pub path: Option<OsString>,
}

/// What a command line's program turned out to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// The executable to pass as `lpApplicationName`.
    Found(PathBuf),
    /// The program is spelled as a relative path with a directory in it, so it
    /// is resolved against the launcher's current directory by the user's own
    /// choice; it is left to `CreateProcessW` as written.
    Relative,
    /// A bare name found in none of the searched directories.
    Missing,
}

/// Resolves `program` as `CreateProcessW` would, minus the current directory.
///
/// `is_file` is the filesystem probe, a parameter so the order can be tested
/// without the directories existing.
pub fn resolve_program(
    program: &str,
    directories: &SearchDirectories,
    is_file: impl Fn(&Path) -> bool,
) -> Resolution {
    if names_directory(program) {
        let path = PathBuf::from(program);
        if !path.is_absolute() {
            return Resolution::Relative;
        }
        // `CreateProcessW` appends `.exe` to an extensionless command-line
        // program but takes `lpApplicationName` literally, so keep finding
        // `C:\tools\shell` as `C:\tools\shell.exe` now that it is passed there.
        if !is_file(&path)
            && let Some(with_extension) = with_default_extension(program)
            && is_file(Path::new(&with_extension))
        {
            return Resolution::Found(PathBuf::from(with_extension));
        }
        return Resolution::Found(path);
    }

    let file_name = with_default_extension(program).unwrap_or_else(|| program.to_owned());
    let path_entries = directories
        .path
        .as_deref()
        .map(|path| std::env::split_paths(path).collect::<Vec<_>>())
        .unwrap_or_default();
    directories
        .application
        .iter()
        .chain(&directories.system)
        .chain(&path_entries)
        // An empty or relative entry is resolved against the current
        // directory, which is the one place this search must not look.
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(&file_name))
        .find(|candidate| is_file(candidate))
        .map_or(Resolution::Missing, Resolution::Found)
}

/// Whether `program` is a path rather than a name to search for. Both
/// separators and a drive prefix count, whatever the host's own rules are.
fn names_directory(program: &str) -> bool {
    let drive =
        program.as_bytes().get(1) == Some(&b':') && program.as_bytes()[0].is_ascii_alphabetic();
    drive || program.contains(['/', '\\'])
}

/// `program` with the `.exe` that `CreateProcessW` assumes for a name without
/// an extension, or `None` when it already has one. A trailing period means
/// "no extension, do not add one", as it does to Windows.
fn with_default_extension(program: &str) -> Option<String> {
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    (!name.contains('.')).then(|| format!("{program}.exe"))
}

/// The value of `PATH` in an environment block given as overrides on top of
/// the current process's, matched case-insensitively as Windows does.
pub fn effective_path<'a>(
    overrides: impl IntoIterator<Item = (&'a String, &'a String)>,
    inherited: Option<OsString>,
) -> Option<OsString> {
    overrides
        .into_iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("PATH"))
        .map(|(_, value)| OsString::from(value))
        .or(inherited)
}

/// The program a command line names, as `CreateProcessW` splits it: up to the
/// closing quote if it opens with one, otherwise up to the first space or tab.
/// No backslash escaping applies to the program, unlike to the arguments.
pub fn command_line_program(command_line: &str) -> &str {
    if let Some(quoted) = command_line.strip_prefix('"') {
        return quoted.split('"').next().unwrap_or(quoted);
    }
    command_line.split([' ', '\t']).next().unwrap_or(command_line)
}

/// `program` spelled as the first token of a command line: quoted when it has
/// a space or tab in it, which would otherwise split it (`C:\Program Files\…`
/// is first tried as `C:\Program`). A program cannot contain `"`, which
/// Windows forbids in file names, so no escaping is needed inside the quotes.
pub fn quote_program(program: &str) -> String {
    if program.is_empty() || program.contains([' ', '\t']) {
        format!("\"{program}\"")
    } else {
        program.to_owned()
    }
}

#[cfg(test)]
#[path = "../tests/tty/program_search.rs"]
mod tests;
