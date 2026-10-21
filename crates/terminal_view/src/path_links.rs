//! Where a path-like link printed in the terminal leads on this machine.
//!
//! A link is only text: relative to the directory the shell was in when it was
//! printed, perhaps `~`-prefixed or a WSL distribution's Linux path, and for a
//! name with spaces in it one of several readings (`terminal::SpacedPathReading`).
//! This resolves the text to a path on this machine and chooses among the
//! readings, the one step that needs the filesystem; callers run
//! [`with_existing_reading`] and [`existing_spaced_reading`] off the foreground
//! thread.

use std::path::PathBuf;

use terminal::{PathLikeTarget, SpacedPathReading};
use util::paths::{PathStyle, PathWithPosition, home_dir};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LocalPathOpenAction {
    OpenDirectory(PathBuf),
    RevealFile(PathBuf),
}

pub(crate) fn local_path_open_action(target: &PathLikeTarget) -> LocalPathOpenAction {
    let path = host_path(target, &target.maybe_path).unwrap_or_else(|| resolve_local_path(target));
    if path.is_dir() {
        LocalPathOpenAction::OpenDirectory(path)
    } else {
        LocalPathOpenAction::RevealFile(path)
    }
}

pub(crate) fn resolve_local_path(target: &PathLikeTarget) -> PathBuf {
    resolve_local_path_text(target, &target.maybe_path)
}

/// The longest of `target`'s spaced readings that names something on disk.
/// Stats up to one path per reading, so keep it off the foreground thread.
pub(crate) fn existing_spaced_reading(target: &PathLikeTarget) -> Option<&SpacedPathReading> {
    target
        .spaced_readings
        .iter()
        .find(|reading| host_path(target, &reading.maybe_path).is_some_and(|path| path.exists()))
}

/// `target`, naming the longest of its spaced readings that exists in place
/// of the fragment that was clicked. Stats, like [`existing_spaced_reading`].
pub(crate) fn with_existing_reading(mut target: PathLikeTarget) -> PathLikeTarget {
    if let Some(reading) = existing_spaced_reading(&target) {
        target.maybe_path = reading.maybe_path.clone();
    }
    target.spaced_readings.clear();
    target
}

/// Where this machine finds `maybe_path`: where it resolves to, or for a shell
/// with a [`PathLikeTarget::host_root`], the same Linux path under that root.
/// `None` when there is nowhere to look, as for a path relative to a WSL
/// directory that was never reported.
fn host_path(target: &PathLikeTarget, maybe_path: &str) -> Option<PathBuf> {
    let path = resolve_local_path_text(target, maybe_path);
    let Some(root) = &target.host_root else {
        return Some(path);
    };
    let mut host = root.clone();
    for component in path.to_str()?.strip_prefix('/')?.split('/') {
        match component {
            "" | "." => {}
            // `..` cannot climb out of the distribution into its siblings.
            ".." => {
                if host != *root {
                    host.pop();
                }
            }
            // A Linux name may hold a backslash, which a Windows path would
            // read as a separator.
            component if component.contains('\\') => return None,
            component => host.push(component),
        }
    }
    Some(host)
}

/// Resolves `maybe_path`, `target`'s text or one of its readings, against
/// `target`'s directory and path style.
fn resolve_local_path_text(target: &PathLikeTarget, maybe_path: &str) -> PathBuf {
    let path = PathWithPosition::parse_str(maybe_path).path;
    let path = if target.path_style == PathStyle::local() {
        expand_local_tilde_path(path)
    } else {
        path
    };
    if target.path_style.is_posix() {
        return if is_posix_absolute_path(&path) || path.is_absolute() {
            path
        } else if let Some(terminal_dir) = target
            .terminal_dir
            .as_deref()
            .filter(|directory| is_posix_absolute_path(directory))
        {
            join_posix_path(terminal_dir, &path)
        } else {
            // Keep the relative path for the WSL editor dispatcher. It
            // resolves it against `pwd` before passing the Windows helper a
            // converted absolute path, so it never falls back to C:\WINDOWS.
            path
        };
    }

    let path = normalize_local_path(path);
    if path.is_absolute() {
        path
    } else if let Some(terminal_dir) = &target.terminal_dir {
        terminal_dir.join(path)
    } else {
        path
    }
}

fn expand_local_tilde_path(path: PathBuf) -> PathBuf {
    let Some(path) = path.to_str() else {
        return path;
    };
    let Some(remainder) = path.strip_prefix('~') else {
        return path.into();
    };
    if remainder.is_empty() {
        return home_dir().clone();
    }

    let Some(separator) = remainder.chars().next() else {
        return path.into();
    };
    if !PathStyle::local().separators_ch().contains(&separator) {
        return path.into();
    }

    home_dir().join(&remainder[separator.len_utf8()..])
}

fn is_posix_absolute_path(path: &std::path::Path) -> bool {
    path.to_str().is_some_and(|path| path.starts_with('/'))
}

fn join_posix_path(directory: &std::path::Path, path: &std::path::Path) -> PathBuf {
    let directory = directory.to_string_lossy();
    let path = path.to_string_lossy();
    PathBuf::from(format!(
        "{}/{}",
        directory.trim_end_matches('/'),
        path.trim_start_matches('/')
    ))
}

#[cfg(target_os = "windows")]
fn normalize_local_path(path: PathBuf) -> PathBuf {
    normalize_windows_path(path)
}

#[cfg(not(target_os = "windows"))]
fn normalize_local_path(path: PathBuf) -> PathBuf {
    path
}

#[cfg(any(target_os = "windows", test))]
fn normalize_windows_path(path: PathBuf) -> PathBuf {
    let mut path = path.to_string_lossy().replace('/', "\\");
    if path.starts_with(r"\\.\") {
        return PathBuf::from(path);
    }
    while path.contains("\\.\\") {
        path = path.replace("\\.\\", "\\");
    }
    while let Some(remainder) = path.strip_prefix(".\\") {
        path = remainder.to_owned();
    }
    if path == "." {
        path.clear();
    } else if path.ends_with("\\.") {
        path.pop();
    }
    PathBuf::from(path)
}

#[cfg(test)]
#[path = "tests/path_links.rs"]
mod tests;
