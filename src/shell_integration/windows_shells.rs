//! Where the Windows shells keep their startup files: the home directory an
//! MSYS2 or Cygwin shell uses, the links those installations leave in place of
//! a file, and the profile path PowerShell itself reports.

use super::*;

pub(super) fn cygwin_tool_from_active_shell(tool: &str) -> PathBuf {
    active_shell_path().map_or_else(
        || PathBuf::from(tool),
        |path| cygwin_tool_for_path(&path, tool),
    )
}

pub(super) fn cygwin_tool_for_path(path: &Path, tool: &str) -> PathBuf {
    cygwin_root_for_path(path)
        .map(|root| root.join("bin").join(tool))
        .filter(|tool_path| tool_path.is_file())
        .unwrap_or_else(|| PathBuf::from(tool))
}

pub(super) fn cygwin_tool_for_path_or_active_shell(path: &Path, tool: &str) -> PathBuf {
    active_shell_path()
        .map(|shell| cygwin_tool_for_path(&shell, tool))
        .filter(|tool_path| tool_path.is_file())
        .unwrap_or_else(|| cygwin_tool_for_path(path, tool))
}

pub(super) fn cygwin_root_for_path(path: &Path) -> Option<PathBuf> {
    let mut candidate = path;
    loop {
        if candidate.join("bin").join("cygwin1.dll").is_file() {
            return Some(candidate.to_path_buf());
        }
        candidate = candidate.parent()?;
    }
}

pub(super) fn resolve_windows_posix_shell_home(
    home: Option<PathBuf>,
    user_profile: Option<PathBuf>,
    convert_unix_path: impl FnOnce(&Path) -> Result<PathBuf>,
) -> Result<PathBuf> {
    match home {
        Some(home) if home.is_absolute() => Ok(home),
        Some(home) => convert_unix_path(&home),
        None => user_profile
            .context("could not locate the home directory: HOME and USERPROFILE are not set"),
    }
}

pub(super) fn resolve_msys2_link_startup_file(
    path: &Path,
    resolve_link: impl FnOnce(&Path) -> Result<PathBuf>,
) -> Result<PathBuf> {
    if path.exists() {
        return Ok(path.to_path_buf());
    }
    let mut link = path.as_os_str().to_os_string();
    link.push(".lnk");
    let link = PathBuf::from(link);
    if !link.is_file() {
        return Ok(path.to_path_buf());
    }
    resolve_link(&link)
}

pub(super) fn resolve_cygwin_link_startup_file(path: &Path) -> Result<PathBuf> {
    resolve_cygwin_link_startup_file_with(
        path,
        |path| path.exists(),
        |path| fs::symlink_metadata(path).is_ok(),
        resolve_cygwin_link,
    )
}

pub(super) fn resolve_cygwin_link_startup_file_with(
    path: &Path,
    is_accessible: impl Fn(&Path) -> bool,
    has_metadata: impl Fn(&Path) -> bool,
    resolve_link: impl FnOnce(&Path) -> Result<Option<PathBuf>>,
) -> Result<PathBuf> {
    // Cygwin stores POSIX symlinks as a reparse-point format that native
    // Windows file APIs cannot follow. `exists` is false for those paths even
    // though Cygwin can read them, while symlink_metadata still exposes the
    // reparse point without following it.
    if is_accessible(path) || !has_metadata(path) {
        return Ok(path.to_path_buf());
    }

    match resolve_link(path)? {
        Some(path) => Ok(path),
        None => Ok(path.to_path_buf()),
    }
}

pub(super) fn resolve_cygwin_link(path: &Path) -> Result<Option<PathBuf>> {
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let resolved = match Command::new(cygwin_tool_for_path_or_active_shell(path, "readlink.exe"))
        .args(["-f", "--"])
        .arg(path)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to resolve the Cygwin startup-file link {}",
                    path.display()
                )
            });
        }
    };
    if !resolved.status.success() {
        return Ok(None);
    }
    let resolved = String::from_utf8(resolved.stdout)
        .context("readlink.exe returned a Cygwin path that was not UTF-8")?;
    let resolved = resolved.trim().trim_start_matches('\u{feff}');
    anyhow::ensure!(
        !resolved.is_empty(),
        "readlink.exe returned an empty path for {}",
        path.display()
    );

    let native = Command::new(cygwin_tool_for_path_or_active_shell(path, "cygpath.exe"))
        .args(["-w", "--", resolved])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .with_context(|| {
            format!(
                "failed to convert the Cygwin startup-file link {}",
                path.display()
            )
        })?;
    anyhow::ensure!(
        native.status.success(),
        "cygpath.exe could not convert {}: {}",
        resolved,
        String::from_utf8_lossy(&native.stderr).trim()
    );
    let native = String::from_utf8(native.stdout)
        .context("cygpath.exe returned a path that was not UTF-8")?;
    let native = native.trim().trim_start_matches('\u{feff}');
    anyhow::ensure!(
        !native.is_empty(),
        "cygpath.exe returned an empty path for {}",
        path.display()
    );
    Ok(Some(PathBuf::from(native)))
}

pub(super) fn resolve_msys2_link(link: &Path) -> Result<PathBuf> {
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let resolved = Command::new("readlink.exe")
        .args(["-f", "--"])
        .arg(link)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .with_context(|| format!("failed to resolve the MSYS2 link {}", link.display()))?;
    anyhow::ensure!(
        resolved.status.success(),
        "readlink.exe could not resolve {}: {}",
        link.display(),
        String::from_utf8_lossy(&resolved.stderr).trim()
    );
    let resolved = String::from_utf8(resolved.stdout)
        .context("readlink.exe returned a path that was not UTF-8")?;
    let resolved = resolved.trim().trim_start_matches('\u{feff}');
    anyhow::ensure!(
        !resolved.is_empty(),
        "readlink.exe returned an empty path for {}",
        link.display()
    );

    let native = Command::new("cygpath.exe")
        .args(["-w", "--", resolved])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .with_context(|| format!("failed to convert the MSYS2 link {}", link.display()))?;
    anyhow::ensure!(
        native.status.success(),
        "cygpath.exe could not convert {}: {}",
        link.display(),
        String::from_utf8_lossy(&native.stderr).trim()
    );
    let native = String::from_utf8(native.stdout)
        .context("cygpath.exe returned a path that was not UTF-8")?;
    let native = native.trim().trim_start_matches('\u{feff}');
    anyhow::ensure!(
        !native.is_empty(),
        "cygpath.exe returned an empty path for {}",
        link.display()
    );
    Ok(PathBuf::from(native))
}

pub(super) fn current_powershell_profile() -> Result<PathBuf> {
    let executable =
        parent_powershell_executable().unwrap_or_else(|| PathBuf::from("powershell.exe"));
    query_powershell_profile(&executable)
}

pub(super) fn parent_powershell_executable() -> Option<PathBuf> {
    let mut system = sysinfo::System::new();
    let mut pid = sysinfo::get_current_pid().ok()?;

    loop {
        system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        let parent_pid = system.process(pid)?.parent()?;
        system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[parent_pid]), true);
        let parent = system.process(parent_pid)?;
        if parent.exe().is_some_and(|executable| {
            executable
                .file_stem()
                .and_then(OsStr::to_str)
                .is_some_and(|name| {
                    name.eq_ignore_ascii_case("powershell") || name.eq_ignore_ascii_case("pwsh")
                })
        }) {
            return parent.exe().map(Path::to_path_buf);
        }
        pid = parent_pid;
    }
}

pub(super) fn query_powershell_profile(executable: &Path) -> Result<PathBuf> {
    let output = Command::new(executable)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::OutputEncoding = New-Object Text.UTF8Encoding; \
             [Console]::Out.Write($PROFILE.CurrentUserCurrentHost)",
        ])
        .output()
        .with_context(|| {
            format!(
                "failed to query the PowerShell profile using {}",
                executable.display()
            )
        })?;
    anyhow::ensure!(
        output.status.success(),
        "failed to query the PowerShell profile using {}: {}",
        executable.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    );

    let profile = String::from_utf8(output.stdout)
        .context("PowerShell returned a profile path that was not UTF-8")?;
    let profile = profile.trim().trim_start_matches('\u{feff}');
    anyhow::ensure!(
        !profile.is_empty(),
        "{} returned an empty PowerShell profile path",
        executable.display()
    );
    Ok(PathBuf::from(profile))
}

#[cfg(test)]
#[path = "../tests/shell_integration/windows_shells.rs"]
mod tests;
