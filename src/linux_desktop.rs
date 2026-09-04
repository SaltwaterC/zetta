use std::{
    collections::HashSet,
    env,
    fs::{self, Metadata, Permissions},
    io::Write as _,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context as _, Result};
use tempfile::NamedTempFile;

use crate::{Profile, profile_is_hidden};

const ACTIONS_BEGIN: &str = "# ZETTA MANAGED PROFILE ACTIONS BEGIN";
const ACTIONS_END: &str = "# ZETTA MANAGED PROFILE ACTIONS END";
const GROUPS_BEGIN: &str = "# ZETTA MANAGED PROFILE GROUPS BEGIN";
const GROUPS_END: &str = "# ZETTA MANAGED PROFILE GROUPS END";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DesktopEntryStamp {
    pub(crate) inode: u64,
    pub(crate) modified: Option<SystemTime>,
    pub(crate) len: u64,
}

pub(crate) fn update_profile_actions(
    profiles: &[Profile],
    hidden_profiles: &HashSet<String>,
) -> Result<bool> {
    let Some(home) = env::var_os("HOME") else {
        return Ok(false);
    };
    let path = user_desktop_entry_path(Path::new(&home));
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Ok(false),
    };
    if !metadata.file_type().is_file() || !is_writable(&metadata) {
        return Ok(false);
    }
    let Ok(contents) = fs::read_to_string(&path) else {
        return Ok(false);
    };
    let Ok(executable) = env::current_exe() else {
        return Ok(false);
    };
    let Some(updated) =
        render_managed_desktop_entry(&contents, &executable, profiles, hidden_profiles)
    else {
        return Ok(false);
    };
    if updated == contents {
        return Ok(false);
    }
    replace_desktop_file(&path, &updated, &metadata.permissions()).map(|()| true)
}

fn user_desktop_entry_path(home: &Path) -> PathBuf {
    home.join(".local/share/applications/Zetta.desktop")
}

pub(crate) fn desktop_entry_stamp() -> DesktopEntryStamp {
    let Some(home) = env::var_os("HOME") else {
        return DesktopEntryStamp {
            inode: 0,
            modified: None,
            len: 0,
        };
    };
    let path = user_desktop_entry_path(Path::new(&home));
    let Ok(metadata) = fs::symlink_metadata(&path) else {
        return DesktopEntryStamp {
            inode: 0,
            modified: None,
            len: 0,
        };
    };
    DesktopEntryStamp {
        inode: metadata.ino(),
        modified: metadata.modified().ok(),
        len: metadata.len(),
    }
}

pub(crate) fn is_managed_user_desktop_entry() -> bool {
    let Some(home) = env::var_os("HOME") else {
        return false;
    };
    let path = user_desktop_entry_path(Path::new(&home));
    let Ok(metadata) = fs::symlink_metadata(&path) else {
        return false;
    };
    if !metadata.file_type().is_file() || !is_writable(&metadata) {
        return false;
    }
    let Ok(contents) = fs::read_to_string(&path) else {
        return false;
    };
    managed_marker_line(&contents, ACTIONS_BEGIN).is_some()
        && managed_marker_line(&contents, ACTIONS_END).is_some()
        && managed_marker_line(&contents, GROUPS_BEGIN).is_some()
        && managed_marker_line(&contents, GROUPS_END).is_some()
}

fn is_writable(metadata: &Metadata) -> bool {
    !metadata.permissions().readonly() && metadata.permissions().mode() & 0o222 != 0
}

fn render_managed_desktop_entry(
    contents: &str,
    executable: &Path,
    profiles: &[Profile],
    hidden_profiles: &HashSet<String>,
) -> Option<String> {
    let visible_profiles = profiles
        .iter()
        .filter(|profile| !profile_is_hidden(profile, hidden_profiles))
        .filter(|profile| valid_profile_name_for_desktop(profile.name.as_str()))
        .collect::<Vec<_>>();
    if !valid_desktop_exec_argument(executable.to_string_lossy().as_ref()) {
        return None;
    }
    let actions = std::iter::once("new-window".to_owned())
        .chain((1..=visible_profiles.len()).map(|index| format!("profile-{index}")))
        .map(|action| format!("{action};"))
        .collect::<String>();
    let actions_block = format!("{ACTIONS_BEGIN}\nActions={actions}\n{ACTIONS_END}");
    let contents = replace_managed_block(contents, ACTIONS_BEGIN, ACTIONS_END, &actions_block)?;

    let executable = quote_exec_argument(executable.to_string_lossy().as_ref());
    let mut groups = String::from(GROUPS_BEGIN);
    for (index, profile) in visible_profiles.iter().enumerate() {
        let action_id = format!("profile-{}", index + 1);
        groups.push_str(&format!(
            "\n\n[Desktop Action {action_id}]\nName={}\nExec={executable} --new-window --profile {}",
            escape_desktop_string(&profile.name),
            quote_exec_argument(&profile.name),
        ));
    }
    groups.push_str(&format!("\n{GROUPS_END}"));
    let contents = replace_managed_block(&contents, GROUPS_BEGIN, GROUPS_END, &groups)?;

    // GNOME Shell caches GDesktopAppInfo and does not compare the Actions key
    // when deciding whether an existing app is stale. Change the main
    // command line with a harmless, private argument so a profile-list change
    // also refreshes the cached action groups. Keep the primary command an
    // ordinary application launch so a stale Dock fallback is handed to the
    // existing process; the launcher update path repairs the window
    // association after the cache refresh instead of creating another window.
    // The profile action commands themselves remain fresh-window commands.
    replace_main_exec(
        &contents,
        &executable,
        profile_actions_generation(&visible_profiles),
    )
}

fn profile_actions_generation(profiles: &[&Profile]) -> u64 {
    let mut hash = 0xcbf29ce484222325;
    for profile in profiles {
        for byte in profile.name.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn replace_main_exec(contents: &str, executable: &str, generation: u64) -> Option<String> {
    let action_group_start = contents.find("\n[Desktop Action ")? + 1;
    let desktop_entry = &contents[..action_group_start];
    let mut exec_line = None;
    let mut line_start = 0;
    for line in desktop_entry.split_inclusive("\n") {
        let line_end = line_start + line.len();
        let line_without_newline = line.strip_suffix("\n").unwrap_or(line);
        if line_without_newline.starts_with("Exec=") {
            if exec_line.is_some() {
                return None;
            }
            exec_line = Some((line_start, line_end));
        }
        line_start = line_end;
    }
    let (line_start, line_end) = exec_line?;
    let replacement =
        format!("Exec={executable} --zetta-profile-actions-generation {generation}\n");
    let mut updated = String::with_capacity(contents.len() + replacement.len());
    updated.push_str(&contents[..line_start]);
    updated.push_str(&replacement);
    updated.push_str(&contents[line_end..]);
    Some(updated)
}

fn replace_managed_block(
    contents: &str,
    begin_marker: &str,
    end_marker: &str,
    replacement: &str,
) -> Option<String> {
    let (begin, begin_end) = managed_marker_line(contents, begin_marker)?;
    let (end, end_end) = managed_marker_line(contents, end_marker)?;
    if begin_end > end {
        return None;
    }
    let mut updated = String::with_capacity(contents.len() + replacement.len());
    updated.push_str(&contents[..begin]);
    updated.push_str(replacement);
    updated.push('\n');
    updated.push_str(&contents[end_end..]);
    Some(updated)
}

fn managed_marker_line(contents: &str, marker: &str) -> Option<(usize, usize)> {
    let mut found = None;
    let mut line_start = 0;
    for line in contents.split_inclusive('\n') {
        let line_end = line_start + line.len();
        let line = line.strip_suffix('\n').unwrap_or(line);
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line == marker {
            if found.is_some() {
                return None;
            }
            found = Some((line_start, line_end));
        }
        line_start = line_end;
    }
    found
}

fn valid_desktop_exec_argument(argument: &str) -> bool {
    !argument.chars().any(char::is_control)
}

fn valid_profile_name_for_desktop(name: &str) -> bool {
    !name.is_empty() && valid_desktop_exec_argument(name)
}

fn escape_desktop_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\t' => escaped.push_str("\\t"),
            '\r' => escaped.push_str("\\r"),
            ';' => escaped.push_str("\\;"),
            character => escaped.push(character),
        }
    }
    escaped
}

fn quote_exec_argument(argument: &str) -> String {
    const RESERVED: &str = " \t\n\"'\\<>~|&;$*?#()`";
    if !argument.is_empty()
        && !argument
            .chars()
            .any(|character| RESERVED.contains(character) || character == '%')
    {
        return argument.to_owned();
    }

    let mut quoted = String::from("\"");
    for character in argument.chars() {
        match character {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '`' => quoted.push_str("\\`"),
            '$' => quoted.push_str("\\$"),
            '%' => quoted.push_str("%%"),
            character => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

fn replace_desktop_file(path: &Path, contents: &str, permissions: &Permissions) -> Result<()> {
    let parent = path
        .parent()
        .context("managed desktop entry has no parent directory")?;
    let mut temporary = NamedTempFile::new_in(parent)
        .with_context(|| format!("creating a temporary desktop entry in {}", parent.display()))?;
    temporary
        .as_file_mut()
        .write_all(contents.as_bytes())
        .context("writing the temporary desktop entry")?;
    temporary
        .as_file()
        .sync_all()
        .context("syncing the temporary desktop entry")?;
    fs::set_permissions(temporary.path(), permissions.clone())
        .context("preserving desktop entry permissions")?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("atomically replacing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/linux_desktop.rs"]
mod tests;
