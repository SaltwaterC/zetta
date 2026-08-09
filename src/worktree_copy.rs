use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context as _, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CopyFileResult {
    Copied,
    Unsupported,
}

trait FileCopyBackend {
    fn copy_file(&self, source: &Path, destination: &Path) -> Result<CopyFileResult>;
}

struct RegularCopyBackend;

impl FileCopyBackend for RegularCopyBackend {
    fn copy_file(&self, source: &Path, destination: &Path) -> Result<CopyFileResult> {
        copy_regular_file(source, destination)?;
        Ok(CopyFileResult::Copied)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CowFilesystem {
    Unsupported,
    #[cfg(target_os = "linux")]
    Btrfs,
    #[cfg(target_os = "linux")]
    Xfs,
    #[cfg(target_os = "macos")]
    Apfs,
    #[cfg(windows)]
    Refs,
}

struct NativeCowBackend {
    filesystem: CowFilesystem,
}

impl NativeCowBackend {
    fn new(source_root: &Path, destination_root: &Path) -> Self {
        Self {
            filesystem: detect_cow_filesystem(source_root, destination_root),
        }
    }
}

impl FileCopyBackend for NativeCowBackend {
    fn copy_file(&self, source: &Path, destination: &Path) -> Result<CopyFileResult> {
        match self.filesystem {
            #[cfg(target_os = "linux")]
            CowFilesystem::Btrfs | CowFilesystem::Xfs => copy_linux_clone(source, destination),
            #[cfg(target_os = "macos")]
            CowFilesystem::Apfs => copy_macos_clone(source, destination),
            #[cfg(windows)]
            CowFilesystem::Refs => copy_windows_clone(source, destination),
            CowFilesystem::Unsupported => Ok(CopyFileResult::Unsupported),
        }
    }
}

pub(crate) fn validate_copy_path(path: &Path) -> Result<PathBuf> {
    let mut normalized = PathBuf::new();
    let mut has_normal_component = false;

    for component in path.components() {
        match component {
            Component::Normal(component) => {
                normalized.push(component);
                has_normal_component = true;
            }
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => {
                anyhow::bail!(
                    "copy path {} must be relative and may not contain absolute or parent-directory traversal",
                    path.display()
                )
            }
        }
    }

    anyhow::ensure!(
        has_normal_component,
        "copy path must name a relative file or directory"
    );
    Ok(normalized)
}

pub(crate) fn validate_copy_paths(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut normalized_paths = Vec::with_capacity(paths.len());
    for path in paths {
        normalized_paths.push(validate_copy_path(path)?);
    }

    for (index, left) in normalized_paths.iter().enumerate() {
        for right in normalized_paths.iter().skip(index + 1) {
            if paths_overlap(left, right) {
                anyhow::bail!(
                    "copy paths {} and {} overlap; specify only one path from an overlapping tree",
                    left.display(),
                    right.display()
                );
            }
        }
    }
    Ok(normalized_paths)
}

pub(crate) fn validate_copy_sources(source_root: &Path, paths: &[PathBuf]) -> Result<()> {
    let paths = validate_copy_paths(paths)?;
    validate_source_root(source_root)?;
    for path in paths {
        validate_source_path(source_root, &path)?;
    }
    Ok(())
}

pub(crate) fn copy_paths(
    source_root: &Path,
    destination_root: &Path,
    paths: &[PathBuf],
) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let backend = NativeCowBackend::new(source_root, destination_root);
    copy_paths_with_backend(source_root, destination_root, paths, &backend)
}

fn copy_paths_with_backend(
    source_root: &Path,
    destination_root: &Path,
    paths: &[PathBuf],
    backend: &dyn FileCopyBackend,
) -> Result<()> {
    let paths = validate_copy_paths(paths)?;
    validate_source_root(source_root)?;
    validate_destination_root(destination_root)?;
    for path in &paths {
        validate_source_path(source_root, path)?;
        validate_destination_path(destination_root, path)?;
    }

    let regular_backend = RegularCopyBackend;
    for path in paths {
        let source = source_root.join(&path);
        let destination = destination_root.join(&path);
        copy_entry(&source, &destination, backend, &regular_backend)
            .with_context(|| format!("copying {}", path.display()))?;
    }
    Ok(())
}

fn validate_source_root(source_root: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source_root).with_context(|| {
        format!(
            "could not inspect source worktree {}",
            source_root.display()
        )
    })?;
    anyhow::ensure!(
        !metadata.file_type().is_symlink() && metadata.is_dir(),
        "source worktree {} is not a real directory",
        source_root.display()
    );
    Ok(())
}

fn validate_destination_root(destination_root: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(destination_root).with_context(|| {
        format!(
            "could not inspect destination worktree {}",
            destination_root.display()
        )
    })?;
    anyhow::ensure!(
        !metadata.file_type().is_symlink() && metadata.is_dir(),
        "destination worktree {} is not a real directory",
        destination_root.display()
    );
    Ok(())
}

fn validate_source_path(source_root: &Path, path: &Path) -> Result<()> {
    let components = path.components().collect::<Vec<_>>();
    let mut current = source_root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(component) = component else {
            unreachable!("validate_copy_path only returns normal components")
        };
        current.push(component);
        let metadata = fs::symlink_metadata(&current).with_context(|| {
            format!(
                "copy source {} does not exist in source worktree",
                path.display()
            )
        })?;
        if index + 1 < components.len() {
            anyhow::ensure!(
                !metadata.file_type().is_symlink(),
                "copy source path {} traverses symlink {}",
                path.display(),
                current.display()
            );
            anyhow::ensure!(
                metadata.is_dir(),
                "copy source path {} traverses non-directory {}",
                path.display(),
                current.display()
            );
        } else {
            let file_type = metadata.file_type();
            anyhow::ensure!(
                file_type.is_file() || file_type.is_dir() || file_type.is_symlink(),
                "copy source {} is not a regular file, directory, or symlink",
                path.display()
            );
        }
    }
    Ok(())
}

fn validate_destination_path(destination_root: &Path, path: &Path) -> Result<()> {
    let components = path.components().collect::<Vec<_>>();
    let mut current = destination_root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(component) = component else {
            unreachable!("validate_copy_path only returns normal components")
        };
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if index + 1 == components.len() {
                    anyhow::bail!("copy destination {} already exists", current.display());
                }
                anyhow::ensure!(
                    !metadata.file_type().is_symlink(),
                    "copy destination path {} traverses symlink {}",
                    path.display(),
                    current.display()
                );
                anyhow::ensure!(
                    metadata.is_dir(),
                    "copy destination path {} traverses non-directory {}",
                    path.display(),
                    current.display()
                );
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("could not inspect copy destination {}", current.display())
                });
            }
        }
    }
    Ok(())
}

fn copy_entry(
    source: &Path,
    destination: &Path,
    backend: &dyn FileCopyBackend,
    regular_backend: &RegularCopyBackend,
) -> Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating copy destination parent {}", parent.display()))?;
    }

    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("could not inspect copy source {}", source.display()))?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        let target = fs::read_link(source)
            .with_context(|| format!("reading symlink {}", source.display()))?;
        create_symlink(&target, destination, source)?;
    } else if metadata.is_dir() {
        fs::create_dir(destination)
            .with_context(|| format!("creating copied directory {}", destination.display()))?;
        for entry in fs::read_dir(source)
            .with_context(|| format!("reading copy source directory {}", source.display()))?
        {
            let entry = entry.with_context(|| format!("reading {}", source.display()))?;
            copy_entry(
                &entry.path(),
                &destination.join(entry.file_name()),
                backend,
                regular_backend,
            )?;
        }
        fs::set_permissions(destination, metadata.permissions()).with_context(|| {
            format!(
                "preserving permissions on copied directory {}",
                destination.display()
            )
        })?;
    } else if file_type.is_file() {
        let result = backend.copy_file(source, destination)?;
        if result == CopyFileResult::Unsupported {
            regular_backend.copy_file(source, destination)?;
        }
        fs::set_permissions(destination, metadata.permissions()).with_context(|| {
            format!(
                "preserving permissions on copied file {}",
                destination.display()
            )
        })?;
    } else {
        anyhow::bail!(
            "copy source {} is not a regular file, directory, or symlink",
            source.display()
        );
    }
    Ok(())
}

fn copy_regular_file(source: &Path, destination: &Path) -> Result<()> {
    let mut source_file = File::open(source)
        .with_context(|| format!("opening copy source file {}", source.display()))?;
    let mut destination_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .with_context(|| format!("creating copied file {}", destination.display()))?;
    if let Err(error) = io::copy(&mut source_file, &mut destination_file) {
        drop(destination_file);
        let _ = fs::remove_file(destination);
        return Err(error).with_context(|| {
            format!(
                "copying file contents from {} to {}",
                source.display(),
                destination.display()
            )
        });
    }
    Ok(())
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    path_is_prefix(left, right) || path_is_prefix(right, left)
}

fn path_is_prefix(prefix: &Path, path: &Path) -> bool {
    let mut prefix_components = prefix.components();
    let mut path_components = path.components();
    loop {
        match (prefix_components.next(), path_components.next()) {
            (None, _) => return true,
            (Some(left), Some(right)) => {
                let (Component::Normal(left), Component::Normal(right)) = (left, right) else {
                    return false;
                };
                if !path_components_equal(left, right) {
                    return false;
                }
            }
            (Some(_), None) => return false,
        }
    }
}

fn path_components_equal(left: &std::ffi::OsStr, right: &std::ffi::OsStr) -> bool {
    if cfg!(windows) {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    }
}

fn create_symlink(target: &Path, destination: &Path, source: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, destination).with_context(|| {
            format!(
                "creating symlink {} copied from {}",
                destination.display(),
                source.display()
            )
        })?;
    }
    #[cfg(windows)]
    {
        let target_is_directory = fs::metadata(source)
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false);
        if target_is_directory {
            std::os::windows::fs::symlink_dir(target, destination)
        } else {
            std::os::windows::fs::symlink_file(target, destination)
        }
        .with_context(|| {
            format!(
                "creating symlink {} copied from {}",
                destination.display(),
                source.display()
            )
        })?;
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, destination, source);
        anyhow::bail!("symlink copying is unsupported on this platform");
    }
    Ok(())
}

fn detect_cow_filesystem(source_root: &Path, destination_root: &Path) -> CowFilesystem {
    #[cfg(target_os = "linux")]
    {
        let Some(source_type) = linux_filesystem_type(source_root) else {
            return CowFilesystem::Unsupported;
        };
        let Some(destination_type) = linux_filesystem_type(destination_root) else {
            return CowFilesystem::Unsupported;
        };
        if source_type != destination_type {
            return CowFilesystem::Unsupported;
        }
        return match source_type {
            LINUX_BTRFS_SUPER_MAGIC => CowFilesystem::Btrfs,
            LINUX_XFS_SUPER_MAGIC => CowFilesystem::Xfs,
            _ => CowFilesystem::Unsupported,
        };
    }

    #[cfg(target_os = "macos")]
    {
        if macos_is_apfs(source_root) && macos_is_apfs(destination_root) {
            CowFilesystem::Apfs
        } else {
            CowFilesystem::Unsupported
        }
    }

    #[cfg(windows)]
    {
        return if windows_refs_volume(source_root, destination_root) {
            CowFilesystem::Refs
        } else {
            CowFilesystem::Unsupported
        };
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = (source_root, destination_root);
        CowFilesystem::Unsupported
    }
}

#[cfg(target_os = "linux")]
const LINUX_BTRFS_SUPER_MAGIC: u64 = 0x9123_683e;
#[cfg(target_os = "linux")]
const LINUX_XFS_SUPER_MAGIC: u64 = 0x5846_5342;

#[cfg(target_os = "linux")]
fn linux_filesystem_type(path: &Path) -> Option<u64> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut statistics = unsafe { std::mem::zeroed::<libc::statfs>() };
    let result = unsafe { libc::statfs(path.as_ptr(), &mut statistics) };
    (result == 0).then_some(statistics.f_type as u64)
}

#[cfg(target_os = "linux")]
fn copy_linux_clone(source: &Path, destination: &Path) -> Result<CopyFileResult> {
    use std::os::fd::AsRawFd as _;

    let source_file = File::open(source)
        .with_context(|| format!("opening copy source file {}", source.display()))?;
    let destination_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .with_context(|| format!("creating copied file {}", destination.display()))?;
    let result = unsafe {
        libc::ioctl(
            destination_file.as_raw_fd(),
            libc::FICLONE,
            source_file.as_raw_fd(),
        )
    };
    if result == 0 {
        return Ok(CopyFileResult::Copied);
    }

    let error = io::Error::last_os_error();
    drop(destination_file);
    let _ = fs::remove_file(destination);
    if clone_error_is_unsupported(&error) {
        Ok(CopyFileResult::Unsupported)
    } else {
        Err(error).with_context(|| {
            format!(
                "cloning file contents from {} to {}",
                source.display(),
                destination.display()
            )
        })
    }
}

#[cfg(target_os = "macos")]
fn macos_is_apfs(path: &Path) -> bool {
    use std::{
        ffi::{CStr, CString},
        os::unix::ffi::OsStrExt,
    };

    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    let mut statistics = unsafe { std::mem::zeroed::<libc::statfs>() };
    if unsafe { libc::statfs(path.as_ptr(), &mut statistics) } != 0 {
        return false;
    }
    unsafe { CStr::from_ptr(statistics.f_fstypename.as_ptr()) }
        .to_bytes()
        .eq_ignore_ascii_case(b"apfs")
}

#[cfg(target_os = "macos")]
fn copy_macos_clone(source: &Path, destination: &Path) -> Result<CopyFileResult> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let source = CString::new(source.as_os_str().as_bytes())
        .context("copy source path contains an embedded NUL byte")?;
    let destination_path = CString::new(destination.as_os_str().as_bytes())
        .context("copy destination path contains an embedded NUL byte")?;
    let result = unsafe { libc::clonefile(source.as_ptr(), destination_path.as_ptr(), 0) };
    if result == 0 {
        return Ok(CopyFileResult::Copied);
    }

    let error = io::Error::last_os_error();
    let _ = fs::remove_file(destination);
    if clone_error_is_unsupported(&error) {
        Ok(CopyFileResult::Unsupported)
    } else {
        Err(error).with_context(|| {
            format!(
                "cloning file contents from {} to {}",
                source.to_string_lossy(),
                destination.display()
            )
        })
    }
}

#[cfg(windows)]
#[derive(Debug)]
struct WindowsVolumeInfo {
    filesystem: String,
    serial_number: u32,
    flags: u32,
}

#[cfg(windows)]
fn windows_refs_volume(source: &Path, destination: &Path) -> bool {
    let Some(source) = windows_volume_info(source) else {
        return false;
    };
    let Some(destination) = windows_volume_info(destination) else {
        return false;
    };
    source.serial_number == destination.serial_number
        && source.filesystem.eq_ignore_ascii_case("ReFS")
        && destination.filesystem.eq_ignore_ascii_case("ReFS")
        && source.flags & windows::Win32::System::SystemServices::FILE_SUPPORTS_BLOCK_REFCOUNTING
            != 0
        && destination.flags
            & windows::Win32::System::SystemServices::FILE_SUPPORTS_BLOCK_REFCOUNTING
            != 0
}

#[cfg(windows)]
fn windows_volume_info(path: &Path) -> Option<WindowsVolumeInfo> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{Win32::Storage::FileSystem::GetVolumeInformationW, core::PCWSTR};

    let path = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut serial_number = 0;
    let mut flags = 0;
    let mut filesystem_name = [0u16; 32];
    unsafe {
        GetVolumeInformationW(
            PCWSTR(path.as_ptr()),
            None,
            Some(&mut serial_number),
            None,
            Some(&mut flags),
            Some(&mut filesystem_name),
        )
        .ok()
        .ok()?;
    }
    let length = filesystem_name
        .iter()
        .position(|character| *character == 0)
        .unwrap_or(filesystem_name.len());
    Some(WindowsVolumeInfo {
        filesystem: String::from_utf16_lossy(&filesystem_name[..length]),
        serial_number,
        flags,
    })
}

#[cfg(windows)]
fn copy_windows_clone(source: &Path, destination: &Path) -> Result<CopyFileResult> {
    use std::os::windows::io::AsRawHandle as _;
    use windows::{
        Win32::{
            Foundation::HANDLE,
            System::{IO::DeviceIoControl, Ioctl::DUPLICATE_EXTENTS_DATA},
        },
        core::Error,
    };

    let source_file = OpenOptions::new()
        .read(true)
        .open(source)
        .with_context(|| format!("opening copy source file {}", source.display()))?;
    let length = source_file
        .metadata()
        .with_context(|| format!("inspecting copy source file {}", source.display()))?
        .len();
    let length = i64::try_from(length)
        .with_context(|| format!("copy source file {} is too large", source.display()))?;
    let destination_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .with_context(|| format!("creating copied file {}", destination.display()))?;
    destination_file
        .set_len(length as u64)
        .with_context(|| format!("sizing copied file {}", destination.display()))?;

    let extents = DUPLICATE_EXTENTS_DATA {
        FileHandle: HANDLE(source_file.as_raw_handle()),
        SourceFileOffset: 0,
        TargetFileOffset: 0,
        ByteCount: length,
    };
    let result = unsafe {
        DeviceIoControl(
            HANDLE(destination_file.as_raw_handle()),
            windows::Win32::System::Ioctl::FSCTL_DUPLICATE_EXTENTS_TO_FILE,
            Some((&extents as *const DUPLICATE_EXTENTS_DATA).cast()),
            std::mem::size_of::<DUPLICATE_EXTENTS_DATA>() as u32,
            None,
            0,
            None,
            None,
        )
    };
    if result.is_ok() {
        return Ok(CopyFileResult::Copied);
    }

    let error = result.unwrap_err();
    drop(destination_file);
    let _ = fs::remove_file(destination);
    if windows_clone_error_is_unsupported(&error) {
        Ok(CopyFileResult::Unsupported)
    } else {
        Err(error).with_context(|| {
            format!(
                "cloning file contents from {} to {}",
                source.display(),
                destination.display()
            )
        })
    }
}

fn clone_error_is_unsupported(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(code)
            if code == libc::EOPNOTSUPP
                || code == libc::ENOTSUP
                || code == libc::ENOTTY
                || code == libc::EXDEV
                || code == libc::EINVAL
                || code == libc::ENOSYS
    )
}

#[cfg(windows)]
fn windows_clone_error_is_unsupported(error: &windows::core::Error) -> bool {
    let code = error.code().0 as u32 & 0xffff;
    matches!(code, 1 | 6 | 17 | 50 | 82 | 87 | 5)
}

#[cfg(test)]
#[path = "tests/worktree_copy.rs"]
mod tests;
