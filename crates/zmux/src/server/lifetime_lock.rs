//! The lock that makes one daemon *the* daemon of a session directory.
//!
//! Starting used to be two separate steps: check whether the published
//! endpoint answered, then remove `zmux.sock` and bind it. Two daemons starting
//! together could both find nothing answering, and the second one's removal
//! unlinked the first one's live socket. Each then served whichever clients
//! happened to reach it, and neither knew about the other's sessions.
//!
//! A daemon now takes `zmux.lock` before it touches the socket, and holds it
//! until it exits. Only the holder removes a stale socket, binds and publishes.
//! One that cannot take it does not start: another daemon is running or is
//! starting, and the clients that asked will find that one. The holder still
//! checks that nothing answers on the socket, because a daemon from before this
//! lock existed holds no lock at all.
//!
//! The lock is never let go across an upgrade, because the replacement's first
//! moments are exactly when a starting daemon would otherwise take it:
//!
//! - **Unix** replaces the image in place. The lock's descriptor is kept open
//!   across the `execv` and named in the handover, so the replacement holds the
//!   same lock from its first instruction (see `prepare_and_exec`).
//! - **Windows** starts a separate replacement. The old daemon duplicates its
//!   lock handle into it before it stops, so the file stays held while both
//!   run and after the old one exits.
//!
//! On Unix the lock is `flock`, through [`std::fs::File::try_lock`]. The file
//! is opened close-on-exec: a shell that inherited it would share the lock and
//! keep it held after the daemon died, so no daemon could start again until
//! that shell exited. On Windows it is the file opened with no sharing at all:
//! a second open fails while any handle to the first is open, in whichever
//! process. A byte-range lock would not do there, because it belongs to the
//! process that took it and could not be handed to the replacement.
//!
//! The file lives in the session directory, which has already been created
//! and checked private, and is never removed: removing a lock file lets a
//! process that opened it a moment before lock a file nobody else can find.

use super::*;

use std::fs::{File, OpenOptions};

/// The lock file's name in the session directory.
pub(super) const LOCK_FILE_NAME: &str = "zmux.lock";

/// How long a starting daemon waits on a lock held by a daemon that is not
/// answering on the socket yet. A daemon that is starting answers within this;
/// one that does not is wedged, and starting beside it would split the clients.
const STARTUP_LOCK_WAIT: Duration = Duration::from_secs(2);

/// How long a replacement that was not handed the lock waits to take it —
/// from an image that predates the lock, which held none, or from a starting
/// daemon that took it in between and gives it up on finding this one's socket.
const RESUME_LOCK_WAIT: Duration = Duration::from_secs(10);

const LOCK_POLL: Duration = Duration::from_millis(20);

/// What the previous image hands its replacement: a descriptor number on Unix,
/// where it survives the `execv`; on Windows, a handle already duplicated into
/// this process.
#[cfg(unix)]
pub(super) type CarriedLock = std::os::fd::RawFd;
#[cfg(windows)]
pub(super) type CarriedLock = File;

/// The held lock. Dropping it releases it.
#[derive(Debug)]
pub(super) struct LifetimeLock {
    file: File,
}

impl LifetimeLock {
    pub(super) fn path(directory: &Path) -> PathBuf {
        directory.join(LOCK_FILE_NAME)
    }

    /// Takes the lock if nobody holds it, and reports `None` if somebody does.
    pub(super) fn try_acquire(directory: &Path) -> Result<Option<Self>> {
        let path = Self::path(directory);
        let Some(file) = open_lock_file(&path)? else {
            return Ok(None);
        };
        #[cfg(unix)]
        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(error).with_context(|| format!("locking {}", path.display()));
            }
        }
        Ok(Some(Self { file }))
    }

    /// Waits until `deadline` for whoever holds the lock to let it go.
    pub(super) fn acquire_by(directory: &Path, deadline: Instant) -> Result<Self> {
        loop {
            if let Some(lock) = Self::try_acquire(directory)? {
                return Ok(lock);
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "{} stayed locked by another multiplexer",
                Self::path(directory).display()
            );
            thread::sleep(LOCK_POLL);
        }
    }
}

/// Opens the lock file, creating it if need be, or reports `None` where the
/// open itself is the lock and somebody else has it (Windows).
#[cfg(unix)]
fn open_lock_file(path: &Path) -> Result<Option<File>> {
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};

    // `std` opens close-on-exec already; see the module documentation for why
    // that matters here.
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .with_context(|| format!("opening the multiplexer lock {}", path.display()))?;
    let metadata = file.metadata()?;
    // SAFETY: geteuid only reads the calling process's effective user ID.
    let user = unsafe { libc::geteuid() };
    anyhow::ensure!(
        metadata.file_type().is_file() && metadata.uid() == user,
        "{} is not a file this user owns",
        path.display()
    );
    if metadata.mode() & 0o077 != 0 {
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(Some(file))
}

#[cfg(windows)]
fn open_lock_file(path: &Path) -> Result<Option<File>> {
    use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};

    /// `FILE_FLAG_OPEN_REPARSE_POINT`: open a link itself, never its target.
    const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const REPARSE_POINT: u32 = 0x400;
    const ERROR_SHARING_VIOLATION: i32 = 32;

    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .custom_flags(OPEN_REPARSE_POINT)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION) => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("opening the multiplexer lock {}", path.display()));
        }
    };
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file() && metadata.file_attributes() & REPARSE_POINT == 0,
        "{} is not an ordinary file",
        path.display()
    );
    Ok(Some(file))
}

#[cfg(unix)]
impl LifetimeLock {
    /// Takes over the lock the previous image held, through the descriptor it
    /// kept open across the `execv`.
    ///
    /// Checked rather than trusted, because claiming a descriptor that is not
    /// open aborts the process: it has to be open, has to be this directory's
    /// lock file, and locking it has to succeed — which, for the descriptor
    /// that already holds the lock, it does.
    pub(super) fn adopt(descriptor: CarriedLock, directory: &Path) -> Result<Self> {
        use std::os::{fd::FromRawFd as _, unix::fs::MetadataExt as _};

        anyhow::ensure!(
            crate::upgrade::descriptor_is_open(descriptor),
            "the lock descriptor {descriptor} was not inherited"
        );
        // SAFETY: the previous image kept this descriptor open for this image to
        // own, and it is claimed exactly once.
        let file = unsafe { File::from_raw_fd(descriptor) };
        let lock = Self { file };
        lock.set_inherited_across_exec(false)?;
        let carried = lock.file.metadata()?;
        let expected = std::fs::symlink_metadata(Self::path(directory))?;
        anyhow::ensure!(
            carried.dev() == expected.dev() && carried.ino() == expected.ino(),
            "the inherited descriptor {descriptor} is not this directory's lock"
        );
        lock.file
            .try_lock()
            .map_err(|_| anyhow::anyhow!("the inherited lock descriptor does not hold the lock"))?;
        Ok(lock)
    }

    pub(super) fn raw_fd(&self) -> std::os::fd::RawFd {
        use std::os::fd::AsRawFd as _;
        self.file.as_raw_fd()
    }

    /// Clears or restores close-on-exec. Cleared only immediately before the
    /// upgrade's `execv`, and restored if it fails, because any process
    /// started while it is clear would share the lock for as long as it lives.
    pub(super) fn set_inherited_across_exec(&self, inherited: bool) -> Result<()> {
        let descriptor = self.raw_fd();
        // SAFETY: the descriptor is owned by `self.file` and open for the call.
        let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFD) };
        anyhow::ensure!(
            flags >= 0,
            "reading the lock's descriptor flags: {}",
            std::io::Error::last_os_error()
        );
        let flags = if inherited {
            flags & !libc::FD_CLOEXEC
        } else {
            flags | libc::FD_CLOEXEC
        };
        // SAFETY: as above, with a flag set derived from the one just read.
        let result = unsafe { libc::fcntl(descriptor, libc::F_SETFD, flags) };
        anyhow::ensure!(
            result >= 0,
            "setting the lock's close-on-exec flag: {}",
            std::io::Error::last_os_error()
        );
        Ok(())
    }
}

#[cfg(windows)]
impl LifetimeLock {
    /// Takes over the handle the previous daemon duplicated into this process.
    ///
    /// It cannot be compared with the file at the lock's path, because opening
    /// that path is exactly what the lock refuses. It came over a channel only
    /// the previous daemon could write to; what is checked is that it is a
    /// file, and that the lock is in fact held.
    pub(super) fn adopt(file: CarriedLock, directory: &Path) -> Result<Self> {
        anyhow::ensure!(
            file.metadata()?.is_file(),
            "the handed-over lock is not a file"
        );
        anyhow::ensure!(
            open_lock_file(&Self::path(directory))?.is_none(),
            "the handed-over lock does not hold {}",
            Self::path(directory).display()
        );
        Ok(Self { file })
    }

    pub(super) fn file(&self) -> &File {
        &self.file
    }
}

/// Runs `f` on the daemon's lifetime lock, if it holds one.
pub(super) fn held_lock<T>(daemon: &Daemon, f: impl FnOnce(&LifetimeLock) -> T) -> Option<T> {
    daemon
        .lifetime_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .map(f)
}

/// Becomes the daemon of `directory`, or says why another one is.
///
/// The socket is checked whether or not the lock was free: if the lock was
/// held, an answer means its holder is up and there is nothing to do; if it was
/// free, an answer means a daemon from before the lock is running.
pub(super) fn claim_directory(directory: &Path, socket: &Path) -> Result<LifetimeLock> {
    let deadline = Instant::now() + STARTUP_LOCK_WAIT;
    loop {
        let lock = LifetimeLock::try_acquire(directory)?;
        anyhow::ensure!(
            Stream::connect(socket).is_err(),
            "another multiplexer is already running on {}",
            socket.display()
        );
        if let Some(lock) = lock {
            return Ok(lock);
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "another multiplexer holds {} but is not answering on {}",
            LifetimeLock::path(directory).display(),
            socket.display()
        );
        thread::sleep(LOCK_POLL);
    }
}

/// The lock for a daemon replacing a previous image.
///
/// Never a reason to fail: by now the previous image is gone, and this one is
/// all that holds its sessions. Without the lock it runs as daemons did before
/// it existed, and says so.
pub(super) fn lock_for_replacement(
    directory: &Path,
    carried: Option<CarriedLock>,
) -> Option<LifetimeLock> {
    if let Some(carried) = carried {
        match LifetimeLock::adopt(carried, directory) {
            Ok(lock) => return Some(lock),
            Err(error) => log::warn!("could not take over the multiplexer lock: {error:#}"),
        }
    }
    match LifetimeLock::acquire_by(directory, Instant::now() + RESUME_LOCK_WAIT) {
        Ok(lock) => Some(lock),
        Err(error) => {
            log::error!("continuing without the multiplexer lock: {error:#}");
            None
        }
    }
}

#[cfg(test)]
#[path = "../tests/server/lifetime_lock.rs"]
mod tests;
