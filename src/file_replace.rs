//! Replacing a user's file without ever leaving half of it on disk.
//!
//! Every file Zetta rewrites on the user's behalf — the configuration, the
//! keymap, a project's `.zetta/config.json`, the projects registry, the
//! desktop-entry edits — goes through here. The new contents are written to a
//! temporary file beside the target and renamed over it, so a crash, a full
//! disk or a permissions error leaves the old file intact rather than a
//! truncated one.
//!
//! Two details a bare rename gets wrong, and which matter for configuration
//! kept in a dotfiles repository: a symlinked target is replaced where the link
//! points rather than having the link itself replaced by a regular file, and an
//! existing file keeps its permissions instead of taking the temporary file's
//! owner-only mode.

use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result};
use tempfile::NamedTempFile;

/// Replaces `path` with `text`, adding a trailing newline when it has none.
pub(crate) fn replace_file(path: &Path, text: &str) -> Result<()> {
    replace_files(&[(path, text)])
}

/// Replaces every file in `files`, all or none as far as the filesystem allows:
/// every replacement is staged beside its target before the first is renamed
/// into place, so the failures that are actually likely — no space, no
/// permission, a missing directory that cannot be created — happen before
/// anything has changed.
pub(crate) fn replace_files(files: &[(&Path, &str)]) -> Result<()> {
    let staged = files
        .iter()
        .map(|(path, text)| stage(path, text))
        .collect::<Result<Vec<_>>>()?;
    for (target, temporary) in staged {
        temporary
            .persist(&target)
            .map_err(|error| error.error)
            .with_context(|| format!("replacing {}", target.display()))?;
    }
    Ok(())
}

fn stage(path: &Path, text: &str) -> Result<(PathBuf, NamedTempFile)> {
    let target = resolve_symlink(path);
    let parent = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let mut temporary = NamedTempFile::new_in(parent)
        .with_context(|| format!("creating a temporary file in {}", parent.display()))?;
    temporary
        .write_all(text.as_bytes())
        .with_context(|| format!("writing {}", target.display()))?;
    if !text.ends_with('\n') {
        temporary.write_all(b"\n")?;
    }
    if let Ok(metadata) = fs::metadata(&target) {
        temporary
            .as_file()
            .set_permissions(metadata.permissions())
            .with_context(|| format!("keeping the permissions of {}", target.display()))?;
    }
    temporary.as_file().sync_all()?;
    Ok((target, temporary))
}

/// The file a write to `path` should replace: the end of its symlink chain, or
/// `path` itself when it is not a link (or the link is dangling, in which case
/// writing through it is what the user would expect of any editor).
fn resolve_symlink(path: &Path) -> PathBuf {
    if !fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return path.to_path_buf();
    }
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
#[path = "tests/file_replace.rs"]
mod tests;
