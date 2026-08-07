use std::{
    collections::HashSet,
    env, fs,
    io::{self, Write as _},
    path::{Path, PathBuf},
    sync::{
        Once,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use sysinfo::{Pid, ProcessesToUpdate, System};

const FILE_PREFIX: &str = "zetta-scrollback-";
const DIRECTORY_NAME: &str = "zetta-scrollback";
const PENDING_FILE_GRACE_PERIOD: Duration = Duration::from_secs(30);
static CLEANUP_MONITOR_RUNNING: AtomicBool = AtomicBool::new(false);
static LEGACY_CLEANUP: Once = Once::new();

/// Creates a private managed scrollback file and returns its persistent path.
/// The caller must promptly pass it to `zetta edit --delete-after`.
pub(crate) fn create(content: &str) -> io::Result<PathBuf> {
    let _ = cleanup_stale();
    #[cfg(target_os = "linux")]
    if Path::new("/dev/shm").is_dir() {
        match private_directory(&linux_shm_root())
            .and_then(|directory| create_in(&directory, content))
        {
            Ok(path) => return Ok(path),
            Err(error) => {
                log::warn!("cannot use /dev/shm for scrollback editor buffer: {error}")
            }
        }
    }
    let directory = private_directory(&fallback_root())?;
    create_in(&directory, content)
}

fn create_in(directory: &Path, content: &str) -> io::Result<PathBuf> {
    let (pid, start_time) = current_process_identity()?;
    let mut file = tempfile::Builder::new()
        .prefix(&format!("{FILE_PREFIX}pending-{pid}-{start_time}-"))
        .suffix(".txt")
        .tempfile_in(directory)?;
    file.write_all(content.as_bytes())?;
    file.flush()?;
    let (_, path) = file.keep().map_err(|error| error.error)?;
    #[cfg(windows)]
    if let Err(error) = mark_temporary(&path) {
        let _ = remove_managed(&path);
        return Err(error);
    }
    Ok(path)
}

/// Transfers a pending managed file to the editor helper's PID. This lets
/// garbage collection distinguish an active editor from a command that was
/// consumed by a foreground terminal application.
pub fn claim_for_editor(path: &Path) -> io::Result<PathBuf> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "scrollback path has no parent")
    })?;
    if !managed_directories()
        .iter()
        .any(|directory| directory == parent)
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "refusing to manage a file outside Zetta's scrollback directories",
        ));
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "invalid scrollback file name")
        })?;
    let owner = parse_owner(file_name).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to manage a file not created by Zetta",
        )
    })?;
    let (pid, start_time) = current_process_identity()?;
    let claimed = parent.join(format!(
        "{FILE_PREFIX}editor-{pid}-{start_time}-{}",
        owner.suffix
    ));
    fs::rename(path, &claimed)?;
    Ok(claimed)
}

pub fn remove_managed(path: &Path) -> bool {
    if let Err(error) = fs::remove_file(path)
        && error.kind() != io::ErrorKind::NotFound
    {
        log::warn!("failed to remove scrollback editor buffer {path:?}: {error}");
        false
    } else {
        true
    }
}

/// Removes files whose owning Zetta/editor process no longer exists, or whose
/// editor handoff was never claimed. Errors are intentionally per-file so one
/// inaccessible entry never stops cleanup.
pub fn cleanup_stale() -> usize {
    cleanup_stale_at(std::time::SystemTime::now())
}

fn cleanup_stale_at(now: std::time::SystemTime) -> usize {
    LEGACY_CLEANUP.call_once(cleanup_legacy_files);
    let mut candidates = Vec::new();
    let mut pids = HashSet::new();
    for directory in managed_directories() {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(owner) = parse_owner(file_name) else {
                continue;
            };
            let FileOwner {
                pid,
                start_time,
                kind,
                ..
            } = owner;
            let modified = fs::metadata(&path)
                .ok()
                .and_then(|metadata| metadata.modified().ok());
            let pid = Pid::from_u32(pid);
            pids.insert(pid);
            candidates.push((path, pid, kind, start_time, modified));
        }
    }

    let current_pid = sysinfo::get_current_pid().ok();
    if candidates.is_empty() {
        return 0;
    }
    let pids = pids.into_iter().collect::<Vec<_>>();
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&pids), true);
    let mut remaining = 0;
    for (path, pid, kind, start_time, modified) in candidates {
        let owner_is_alive = system
            .process(pid)
            .is_some_and(|process| start_time.is_none_or(|start| process.start_time() == start));
        let pending_handoff_expired = handoff_expired(kind, current_pid, pid, modified, now);
        if !owner_is_alive || pending_handoff_expired {
            remaining += usize::from(!remove_managed(&path));
        } else {
            remaining += 1;
        }
    }
    remaining
}

fn handoff_expired(
    kind: FileKind,
    current_pid: Option<Pid>,
    pid: Pid,
    modified: Option<std::time::SystemTime>,
    now: std::time::SystemTime,
) -> bool {
    (kind == FileKind::Pending || (kind == FileKind::Legacy && current_pid == Some(pid)))
        && modified.is_some_and(|modified| {
            now.duration_since(modified)
                .is_ok_and(|age| age >= PENDING_FILE_GRACE_PERIOD)
        })
}

fn cleanup_legacy_files() {
    let Ok(entries) = fs::read_dir(env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if file_name.starts_with(FILE_PREFIX)
            && file_name.ends_with(".txt")
            && parse_owner(file_name).is_none()
        {
            let _ = remove_managed(&path);
        }
    }
}

/// Starts a short-lived background monitor. It exits as soon as no managed
/// files remain and is restarted by the next scrollback edit. This provides
/// prompt crash cleanup without permanent polling or UI-thread work.
pub fn start_cleanup_monitor() {
    if CLEANUP_MONITOR_RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    if let Err(error) = std::thread::Builder::new()
        .name("scrollback-temp-cleanup".to_owned())
        .spawn(|| {
            loop {
                if cleanup_stale() == 0 {
                    CLEANUP_MONITOR_RUNNING.store(false, Ordering::Release);
                    // Close the race with a file created while the flag still
                    // read true: reacquire ownership if work appeared.
                    if cleanup_stale() == 0 || CLEANUP_MONITOR_RUNNING.swap(true, Ordering::AcqRel)
                    {
                        break;
                    }
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        })
    {
        CLEANUP_MONITOR_RUNNING.store(false, Ordering::Release);
        log::warn!("failed to start scrollback temporary-file cleanup: {error}");
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileKind {
    Pending,
    Editor,
    Legacy,
}

struct FileOwner<'a> {
    pid: u32,
    start_time: Option<u64>,
    suffix: &'a str,
    kind: FileKind,
}

fn parse_owner(file_name: &str) -> Option<FileOwner<'_>> {
    let remainder = file_name.strip_prefix(FILE_PREFIX)?;
    let (kind, remainder) = if let Some(remainder) = remainder.strip_prefix("pending-") {
        (FileKind::Pending, remainder)
    } else if let Some(remainder) = remainder.strip_prefix("editor-") {
        (FileKind::Editor, remainder)
    } else {
        (FileKind::Legacy, remainder)
    };
    let (pid, suffix) = remainder.split_once('-')?;
    let pid = pid.parse().ok()?;
    if let Some((start_time, suffix)) = suffix.split_once('-')
        && let Ok(start_time) = start_time.parse()
    {
        return Some(FileOwner {
            pid,
            start_time: Some(start_time),
            suffix,
            kind,
        });
    }
    // Compatibility with files created by the first implementation, which
    // encoded only a PID. They are still collected once that PID is absent.
    Some(FileOwner {
        pid,
        start_time: None,
        suffix,
        kind,
    })
}

fn current_process_identity() -> io::Result<(u32, u64)> {
    let pid = sysinfo::get_current_pid()
        .map_err(|error| io::Error::other(format!("cannot determine process ID: {error}")))?;
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    let start_time = system
        .process(pid)
        .map(|process| process.start_time())
        .ok_or_else(|| io::Error::other("cannot determine process start time"))?;
    Ok((pid.as_u32(), start_time))
}

fn managed_directories() -> Vec<PathBuf> {
    #[cfg(target_os = "linux")]
    return vec![
        linux_shm_root().join(DIRECTORY_NAME),
        fallback_root().join(DIRECTORY_NAME),
    ];

    #[cfg(not(target_os = "linux"))]
    vec![fallback_root().join(DIRECTORY_NAME)]
}

fn fallback_root() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        if let Some(cache) = env::var_os("XDG_CACHE_HOME").filter(|path| !path.is_empty()) {
            return PathBuf::from(cache).join("zetta");
        }
        if let Some(home) = env::var_os("HOME").filter(|path| !path.is_empty()) {
            return PathBuf::from(home).join(".cache").join("zetta");
        }
        return env::temp_dir().join("zetta");
    }
    #[cfg(windows)]
    {
        // MSYS2 can set TMP to its Unix-style temporary directory. The GUI
        // and the `zetta edit` helper must instead agree on one native path.
        return windows_fallback_root(
            env::var_os("LOCALAPPDATA")
                .filter(|path| !path.is_empty())
                .map(PathBuf::from),
            env::temp_dir(),
        );
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    env::temp_dir().join("zetta")
}

#[cfg(windows)]
fn windows_fallback_root(local_app_data: Option<PathBuf>, temporary_directory: PathBuf) -> PathBuf {
    local_app_data
        .map(|path| path.join("Temp"))
        .unwrap_or(temporary_directory)
        .join("zetta")
}

#[cfg(target_os = "linux")]
fn linux_shm_root() -> PathBuf {
    // A UID-specific parent avoids traversing or attempting to collect files
    // belonging to other users in the globally writable /dev/shm directory.
    Path::new("/dev/shm").join(format!("zetta-{}", unsafe { libc::geteuid() }))
}

fn private_directory(root: &Path) -> io::Result<PathBuf> {
    let directory = root.join(DIRECTORY_NAME);
    fs::create_dir_all(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        let metadata = fs::symlink_metadata(&directory)?;
        if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "scrollback directory is not a private directory owned by the current user",
            ));
        }
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    }
    Ok(directory)
}

#[cfg(windows)]
fn mark_temporary(path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows::{
        Win32::Storage::FileSystem::{FILE_ATTRIBUTE_TEMPORARY, SetFileAttributesW},
        core::PCWSTR,
    };

    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    unsafe { SetFileAttributesW(PCWSTR(wide.as_ptr()), FILE_ATTRIBUTE_TEMPORARY) }
        .map_err(|_| io::Error::last_os_error())
}

#[cfg(test)]
#[path = "tests/scrollback_temp.rs"]
mod tests;
