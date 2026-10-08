//! Private directories and files: where endpoint tokens are allowed to live.
//!
//! The session directory holds the multiplexer's and every Zetta process's
//! control endpoint, each a socket path plus the token that authenticates a
//! channel to it. Whoever can plant or replace a file there chooses where a
//! client sends its requests — session secrets, resume identities and
//! passphrases among them — and, through stale-endpoint cleanup, what gets
//! unlinked. This module decides whether a directory is private enough to hold
//! them, and is the only way files are written into or read out of one.
//!
//! It is compiled twice: as `zmux::private_fs`, and straight into a Zetta built
//! without the `zmux` feature (see `src/main.rs`), so the checks have exactly
//! one implementation. It may therefore use nothing beyond `std`, `anyhow`,
//! `getrandom`, `libc` and `windows`, which both crates depend on.
//!
//! # Unix
//!
//! A path is resolved one component at a time, as the kernel would, and each
//! step is checked before the next is trusted — checking only the final
//! directory says nothing, because whoever can rename an ancestor can swap the
//! whole subtree beneath it:
//!
//! - every directory on the way is owned by root or by this user, and is
//!   writable by nobody else unless it is sticky (`/tmp`), or it is writable by
//!   this user's own private group (the `user`/`user` scheme Debian, Ubuntu and
//!   Fedora give every account, under which a home directory is `0775`);
//! - every symbolic link on the way is owned by root or by this user;
//! - the private directory itself is not a link, is owned by this user, and is
//!   writable by nobody else on the same terms. Writers also make it `0700`.
//!
//! A missing component is created only once its parent has passed, one level
//! at a time with mode `0700`, so the umask can narrow it but never widen it.
//! A predictable directory somebody else created first — the per-user prefix
//! under the temporary directory is the one that matters — fails that check,
//! and the operation fails with it. Choosing a different directory instead
//! would not help: the daemon and every client have to derive the same path
//! independently, so a location only one of them knows is no location at all.
//!
//! # Windows
//!
//! Missing components are created with a protected DACL granting only this
//! account and SYSTEM, and the private directory must not be a reparse point.
//! The owner and DACL of a directory that already exists are not checked: an
//! elevated token's default owner is the Administrators group, so an owner
//! check would refuse the account's own directories, and validating an
//! inherited DACL properly means evaluating it rather than comparing it. The
//! normal parent, `%APPDATA%`, is private to the account already.

use std::{
    fs,
    io::{self, Read as _, Write as _},
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result};

/// The largest endpoint file a reader accepts. Real ones are a few hundred
/// bytes; the limit only stops a reader from slurping whatever it is given.
pub const MAX_ENDPOINT_BYTES: u64 = 64 * 1024;

/// Creates `path` as a directory only the current user may use, creating each
/// missing ancestor privately below a parent that has already been checked.
///
/// Fails rather than repairing an ancestor that another user could rename or
/// replace — see the module documentation for the policy.
pub fn create_private_dir(path: &Path) -> Result<()> {
    resolve_private_dir(path, true)
        .with_context(|| format!("creating private directory {}", path.display()))
}

/// Checks that an existing `path` is private, without creating or changing
/// anything. A reader calls this before trusting a file inside it.
///
/// A missing directory is reported as [`io::ErrorKind::NotFound`], so a caller
/// that treats absence as "nothing published" can still say so.
pub fn validate_private_dir(path: &Path) -> io::Result<()> {
    resolve_private_dir(path, false)
}

/// Where configuration lives when the platform's per-user location is unknown.
///
/// The current directory is not an acceptable substitute: this directory holds
/// the process control token and the session catalogs, and a working directory
/// can be one another user may write to. A per-user name under the system
/// temporary directory is predictable, so [`create_private_dir`] refuses it
/// if anybody else got there first.
pub fn private_fallback_dir() -> PathBuf {
    #[cfg(unix)]
    {
        // SAFETY: geteuid only reads the calling process's effective user ID
        // and cannot fail.
        std::env::temp_dir().join(format!("zetta-{}", unsafe { libc::geteuid() }))
    }
    #[cfg(not(unix))]
    std::env::temp_dir().join("zetta")
}

/// Writes `contents` to `path`, replacing it atomically.
///
/// The bytes go to a fresh, randomly named file beside `path`, created
/// exclusively with mode `0600`, which is then renamed over `path`. Nothing is
/// ever opened by a name somebody could have prepared in advance, so a
/// symbolic link or an existing file at a predictable temporary name cannot
/// redirect or pre-empt the write.
pub fn write_private_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} has no file name", path.display()),
        )
    })?;
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let (temporary, mut file) = create_temporary_file(directory, name)?;
    file.write_all(contents)
        .and_then(|()| {
            drop(file);
            replace_file(&temporary, path)
        })
        .inspect_err(|_| {
            let _ = fs::remove_file(&temporary);
        })
}

/// Reads a file another process of this user published into a private
/// directory, refusing anything that is not a regular file, that is longer
/// than `limit` bytes, or — on Unix — that this user does not own or somebody
/// else can write to. A symbolic link (on Windows, any reparse point) is
/// refused rather than followed.
pub fn read_private_file(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let file = open_private_file(path)?;
    let metadata = file.metadata()?;
    check_private_file(path, &metadata)?;
    if metadata.len() > limit {
        return Err(refusal(path, "is larger than any file published here"));
    }
    let mut contents = Vec::new();
    file.take(limit + 1).read_to_end(&mut contents)?;
    if contents.len() as u64 > limit {
        return Err(refusal(path, "is larger than any file published here"));
    }
    Ok(contents)
}

/// Removes the socket a dead process left at `path`, but only if what is
/// there is a socket this user owns. A missing path is not an error.
///
/// Callers derive `path` themselves from the endpoint's own location; a path
/// read out of an endpoint file is never handed here.
pub fn remove_stale_socket(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    check_stale_socket(path, &metadata)?;
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn refusal(path: &Path, reason: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("{} {reason}", path.display()),
    )
}

fn random_suffix() -> io::Result<String> {
    let mut bytes = [0_u8; 8];
    getrandom::fill(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Creates a new file beside the one it will replace. The leading dot keeps it
/// out of every directory scan that looks for `zetta-*` or `control-*` files.
fn create_temporary_file(
    directory: &Path,
    name: &std::ffi::OsStr,
) -> io::Result<(PathBuf, fs::File)> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    loop {
        let temporary = directory.join(format!(
            ".{}.{}.tmp",
            name.to_string_lossy(),
            random_suffix()?
        ));
        match options.open(&temporary) {
            Ok(file) => return Ok((temporary, file)),
            // Sixty-four random bits make a collision a curiosity, but one is
            // still not a reason to give up or to reuse the file.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
}

fn replace_file(temporary: &Path, path: &Path) -> io::Result<()> {
    match fs::rename(temporary, path) {
        #[cfg(windows)]
        Err(_) if path.exists() => {
            // A reader holding the old file open without delete sharing makes
            // a replacing rename fail; removing it first is what the
            // publishers here did before this module existed.
            fs::remove_file(path)?;
            fs::rename(temporary, path)
        }
        result => result,
    }
}

#[cfg(unix)]
use unix::{check_private_file, check_stale_socket, open_private_file, resolve_private_dir};
#[cfg(windows)]
pub use windows_impl::{UserOnlySecurity, UserOnlySecurityTarget};
#[cfg(windows)]
use windows_impl::{
    check_private_file, check_stale_socket, open_private_file, resolve_private_dir,
};

#[cfg(unix)]
mod unix {
    use std::{
        collections::VecDeque,
        ffi::{CStr, OsString},
        os::unix::fs::{
            DirBuilderExt as _, FileTypeExt as _, MetadataExt as _, OpenOptionsExt as _,
            PermissionsExt as _,
        },
        path::Component,
        sync::OnceLock,
    };

    use super::*;

    /// How many symbolic links one resolution may follow, as Linux's
    /// `MAXSYMLINKS`.
    const MAX_SYMLINKS: usize = 40;
    const STICKY: u32 = 0o1000;

    enum Step {
        Parent,
        Name(OsString),
    }

    fn steps(path: &Path) -> VecDeque<Step> {
        path.components()
            .filter_map(|component| match component {
                Component::ParentDir => Some(Step::Parent),
                Component::Normal(name) => Some(Step::Name(name.to_owned())),
                Component::RootDir | Component::CurDir | Component::Prefix(_) => None,
            })
            .collect()
    }

    fn euid() -> u32 {
        // SAFETY: geteuid only reads the calling process's effective user ID
        // and cannot fail.
        unsafe { libc::geteuid() }
    }

    pub(super) fn resolve_private_dir(path: &Path, create: bool) -> io::Result<()> {
        let path = std::path::absolute(path)?;
        let mut pending = steps(&path);
        ensure(
            pending.iter().any(|step| matches!(step, Step::Name(_))),
            &path,
            "cannot be a private directory",
        )?;
        let mut current = PathBuf::from("/");
        check_ancestor(&current, &fs::symlink_metadata(&current)?)?;
        let mut links = 0;
        while let Some(step) = pending.pop_front() {
            let name = match step {
                // `current` holds no links, so its parent is the physical one,
                // which is what the kernel resolves `..` to as well.
                Step::Parent => {
                    current.pop();
                    continue;
                }
                Step::Name(name) => name,
            };
            let next = current.join(&name);
            let metadata = match fs::symlink_metadata(&next) {
                Err(error) if error.kind() == io::ErrorKind::NotFound && create => {
                    // `current` has passed, so nobody else can have put
                    // anything here that this would now trust.
                    match fs::DirBuilder::new().mode(0o700).create(&next) {
                        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => {
                            return Err(error);
                        }
                        _ => fs::symlink_metadata(&next)?,
                    }
                }
                result => result?,
            };
            if metadata.file_type().is_symlink() {
                if pending.is_empty() {
                    return Err(refusal(&next, "is a symbolic link"));
                }
                check_owner(&next, &metadata)?;
                links += 1;
                ensure(links <= MAX_SYMLINKS, &path, "has too many symbolic links")?;
                let target = fs::read_link(&next)?;
                if target.is_absolute() {
                    current = PathBuf::from("/");
                }
                let mut expanded = steps(&target);
                expanded.append(&mut pending);
                pending = expanded;
                continue;
            }
            if !metadata.is_dir() {
                return Err(refusal(&next, "is not a directory"));
            }
            if !pending.is_empty() {
                check_ancestor(&next, &metadata)?;
            }
            current = next;
        }
        check_private_dir(&current, create)
    }

    fn ensure(condition: bool, path: &Path, reason: &str) -> io::Result<()> {
        if condition {
            Ok(())
        } else {
            Err(refusal(path, reason))
        }
    }

    fn check_owner(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
        if metadata.uid() == 0 || metadata.uid() == euid() {
            Ok(())
        } else {
            Err(refusal(path, "is owned by another user"))
        }
    }

    /// A directory on the way to a private one: nobody but root or this user
    /// may be able to rename what is inside it.
    fn check_ancestor(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
        check_owner(path, metadata)?;
        if metadata.mode() & STICKY != 0 {
            return Ok(());
        }
        check_not_shared_writable(path, metadata)
    }

    /// Opens the final directory without following a link, so the checks and
    /// any `chmod` apply to the directory that was resolved and nothing else.
    fn check_private_dir(path: &Path, create: bool) -> io::Result<()> {
        let directory = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(path)?;
        let metadata = directory.metadata()?;
        if metadata.uid() != euid() {
            return Err(refusal(path, "is owned by another user"));
        }
        let mode = metadata.mode();
        if create {
            if mode & 0o077 != 0 {
                directory.set_permissions(fs::Permissions::from_mode(0o700))?;
            }
        } else {
            check_not_shared_writable(path, &metadata)?;
        }
        Ok(())
    }

    pub(super) fn open_private_file(path: &Path) -> io::Result<fs::File> {
        // `O_NONBLOCK` so that a FIFO planted in place of the file cannot hang
        // the open; the type check below then refuses it.
        fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
    }

    pub(super) fn check_private_file(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
        if !metadata.is_file() {
            return Err(refusal(path, "is not a regular file"));
        }
        if metadata.uid() != euid() {
            return Err(refusal(path, "is owned by another user"));
        }
        check_not_shared_writable(path, metadata)
    }

    /// Nobody but this user may write it: not every user, and not its group
    /// unless that group is this user's own private one.
    fn check_not_shared_writable(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
        let mode = metadata.mode();
        if mode & 0o002 != 0 {
            return Err(refusal(path, "is writable by every user"));
        }
        if mode & 0o020 != 0 && private_group() != Some(metadata.gid()) {
            return Err(refusal(path, "is writable by its group"));
        }
        Ok(())
    }

    pub(super) fn check_stale_socket(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
        if !metadata.file_type().is_socket() {
            return Err(refusal(path, "is not a socket"));
        }
        if metadata.uid() != euid() {
            return Err(refusal(path, "is owned by another user"));
        }
        Ok(())
    }

    /// This user's primary group, if it is a private one: named after the
    /// user and listing nobody else. Such a group being able to write a
    /// directory gives nobody else anything.
    ///
    /// Looked up once per process: the answer is a property of the account,
    /// and the lookup can go to a directory service.
    fn private_group() -> Option<u32> {
        static GROUP: OnceLock<Option<u32>> = OnceLock::new();
        *GROUP.get_or_init(lookup_private_group)
    }

    fn lookup_private_group() -> Option<u32> {
        let mut passwd_buffer = Vec::new();
        // SAFETY: an all-zero `passwd` is a valid out-parameter.
        let mut passwd: libc::passwd = unsafe { std::mem::zeroed() };
        let found = with_growing_buffer(&mut passwd_buffer, |buffer, result| {
            let mut entry = std::ptr::null_mut();
            // SAFETY: every pointer is valid for the call, and `buffer` is
            // writable for its whole length.
            let status = unsafe {
                libc::getpwuid_r(
                    euid(),
                    &mut passwd,
                    buffer.as_mut_ptr(),
                    buffer.len(),
                    &mut entry,
                )
            };
            *result = !entry.is_null();
            status
        })?;
        if !found || passwd.pw_name.is_null() {
            return None;
        }
        // SAFETY: `pw_name` points into `passwd_buffer`, which is still alive.
        let user = unsafe { CStr::from_ptr(passwd.pw_name) }.to_owned();
        let gid = passwd.pw_gid;

        let mut group_buffer = Vec::new();
        // SAFETY: an all-zero `group` is a valid out-parameter.
        let mut group: libc::group = unsafe { std::mem::zeroed() };
        let found = with_growing_buffer(&mut group_buffer, |buffer, result| {
            let mut entry = std::ptr::null_mut();
            // SAFETY: as for `getpwuid_r` above.
            let status = unsafe {
                libc::getgrgid_r(
                    gid,
                    &mut group,
                    buffer.as_mut_ptr(),
                    buffer.len(),
                    &mut entry,
                )
            };
            *result = !entry.is_null();
            status
        })?;
        if !found || group.gr_name.is_null() {
            return None;
        }
        // SAFETY: the group's strings point into `group_buffer`, which is
        // still alive, and `gr_mem` is a null-terminated array.
        unsafe {
            if CStr::from_ptr(group.gr_name) != user.as_c_str() {
                return None;
            }
            let mut member = group.gr_mem;
            while !member.is_null() && !(*member).is_null() {
                if CStr::from_ptr(*member) != user.as_c_str() {
                    return None;
                }
                member = member.add(1);
            }
        }
        Some(gid)
    }

    /// Runs a reentrant `get*_r` lookup, doubling its buffer on `ERANGE`.
    /// Returns whether an entry was found, or `None` if the lookup failed.
    fn with_growing_buffer(
        buffer: &mut Vec<libc::c_char>,
        mut lookup: impl FnMut(&mut [libc::c_char], &mut bool) -> libc::c_int,
    ) -> Option<bool> {
        let mut size = 4096;
        loop {
            buffer.resize(size, 0);
            let mut found = false;
            match lookup(buffer, &mut found) {
                0 => return Some(found),
                libc::ERANGE if size < 1 << 20 => size *= 2,
                _ => return None,
            }
        }
    }
}

#[cfg(windows)]
mod windows_impl {
    use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};

    use windows::{
        Win32::{
            Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree},
            Security::{
                Authorization::{
                    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                    SDDL_REVISION_1,
                },
                GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
                TOKEN_USER, TokenUser,
            },
            Storage::FileSystem::{
                CreateDirectoryW, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
            },
            System::Threading::{GetCurrentProcess, OpenProcessToken},
        },
        core::{BOOL, HSTRING, PWSTR},
    };

    use super::*;

    /// What a [`UserOnlySecurity`] descriptor is for, which decides whether
    /// its entries are inherited.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum UserOnlySecurityTarget {
        /// A directory: the entries are inherited by everything created in it.
        Directory,
        /// A kernel object with no children, such as a named pipe.
        Object,
    }

    /// Security attributes granting this account and SYSTEM, and nobody else,
    /// by the account's own SID.
    ///
    /// Not `OW` (owner rights): an elevated token's default owner is the
    /// Administrators group, so an object created elevated would refuse the
    /// same account's non-elevated processes.
    pub struct UserOnlySecurity {
        descriptor: PSECURITY_DESCRIPTOR,
        attributes: SECURITY_ATTRIBUTES,
    }

    impl UserOnlySecurity {
        pub fn new(target: UserOnlySecurityTarget) -> io::Result<Self> {
            let user = current_user_sid()?;
            let sddl = match target {
                UserOnlySecurityTarget::Directory => {
                    format!("D:P(A;OICI;FA;;;{user})(A;OICI;FA;;;SY)")
                }
                UserOnlySecurityTarget::Object => format!("D:P(A;;GA;;;{user})(A;;GA;;;SY)"),
            };
            let sddl = HSTRING::from(sddl);
            let mut descriptor = PSECURITY_DESCRIPTOR(std::ptr::null_mut());
            // SAFETY: `sddl` is a valid null-terminated string, and the
            // descriptor it produces is freed in `Drop`.
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    &sddl,
                    SDDL_REVISION_1,
                    &mut descriptor,
                    None,
                )
            }
            .map_err(io::Error::other)?;
            Ok(Self {
                descriptor,
                attributes: SECURITY_ATTRIBUTES {
                    nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                    lpSecurityDescriptor: descriptor.0,
                    bInheritHandle: BOOL(0),
                },
            })
        }

        pub fn attributes(&self) -> &SECURITY_ATTRIBUTES {
            &self.attributes
        }
    }

    impl Drop for UserOnlySecurity {
        fn drop(&mut self) {
            // SAFETY: the descriptor was allocated by
            // ConvertStringSecurityDescriptorToSecurityDescriptorW.
            unsafe { LocalFree(Some(HLOCAL(self.descriptor.0))) };
        }
    }

    fn current_user_sid() -> io::Result<String> {
        // SAFETY: the token handle is closed before returning, the buffer is
        // sized by the first query and aligned for `TOKEN_USER`, and the SID
        // string is freed after it has been copied.
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
                .map_err(io::Error::other)?;
            let mut length = 0_u32;
            let _ = GetTokenInformation(token, TokenUser, None, 0, &mut length);
            let mut buffer = vec![0_u64; (length as usize).div_ceil(8)];
            let queried = GetTokenInformation(
                token,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                length,
                &mut length,
            );
            let _ = CloseHandle(token);
            queried.map_err(io::Error::other)?;
            let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
            let mut sid = PWSTR::null();
            ConvertSidToStringSidW(user.User.Sid, &mut sid).map_err(io::Error::other)?;
            let text = sid.to_string().map_err(io::Error::other);
            LocalFree(Some(HLOCAL(sid.0.cast())));
            text
        }
    }

    fn is_reparse_point(metadata: &fs::Metadata) -> bool {
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
    }

    pub(super) fn resolve_private_dir(path: &Path, create: bool) -> io::Result<()> {
        let path = std::path::absolute(path)?;
        if create {
            let mut missing = Vec::new();
            let mut cursor = Some(path.as_path());
            while let Some(directory) = cursor {
                match fs::symlink_metadata(directory) {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        missing.push(directory);
                        cursor = directory.parent();
                    }
                    _ => break,
                }
            }
            if !missing.is_empty() {
                let security = UserOnlySecurity::new(UserOnlySecurityTarget::Directory)?;
                for directory in missing.into_iter().rev() {
                    let name = HSTRING::from(directory);
                    // SAFETY: `name` is null-terminated and the attributes
                    // outlive the call.
                    let created = unsafe {
                        CreateDirectoryW(&name, Some(std::ptr::from_ref(security.attributes())))
                    };
                    // Losing a race to create it is fine: whatever is there is
                    // checked below like any directory that already existed.
                    if let Err(error) = created
                        && fs::symlink_metadata(directory).is_err()
                    {
                        return Err(io::Error::other(error));
                    }
                }
            }
        }
        let metadata = fs::symlink_metadata(&path)?;
        if is_reparse_point(&metadata) {
            return Err(refusal(&path, "is a link or junction"));
        }
        if !metadata.is_dir() {
            return Err(refusal(&path, "is not a directory"));
        }
        Ok(())
    }

    pub(super) fn open_private_file(path: &Path) -> io::Result<fs::File> {
        fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(path)
    }

    pub(super) fn check_private_file(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
        if is_reparse_point(metadata) || !metadata.is_file() {
            return Err(refusal(path, "is not a regular file"));
        }
        Ok(())
    }

    /// Windows has no socket file type to check for; an `AF_UNIX` socket is a
    /// reparse point. Refuse only what cannot be one.
    pub(super) fn check_stale_socket(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
        if metadata.is_dir() {
            return Err(refusal(path, "is a directory, not a socket"));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/private_fs.rs"]
mod tests;
