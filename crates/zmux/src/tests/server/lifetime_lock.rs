use super::*;

fn private_directory() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    crate::private_fs::create_private_dir(directory.path()).unwrap();
    directory
}

#[test]
fn only_one_holder_at_a_time() {
    let directory = private_directory();
    let held = LifetimeLock::try_acquire(directory.path())
        .unwrap()
        .expect("a free lock is taken");
    assert!(
        LifetimeLock::try_acquire(directory.path())
            .unwrap()
            .is_none()
    );

    drop(held);
    assert!(
        LifetimeLock::try_acquire(directory.path())
            .unwrap()
            .is_some()
    );
}

#[cfg(unix)]
#[test]
fn the_lock_is_private_and_never_inherited() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = private_directory();
    let lock = LifetimeLock::try_acquire(directory.path())
        .unwrap()
        .unwrap();
    // A shell that inherited the descriptor would hold the lock for as long as
    // it lived, after the daemon that took it had gone.
    let flags = unsafe { libc::fcntl(lock.raw_fd(), libc::F_GETFD) };
    assert_ne!(flags & libc::FD_CLOEXEC, 0);
    let mode = std::fs::metadata(LifetimeLock::path(directory.path()))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[cfg(unix)]
#[test]
fn a_link_in_place_of_the_lock_is_refused() {
    let directory = private_directory();
    let elsewhere = directory.path().join("elsewhere");
    std::fs::write(&elsewhere, b"").unwrap();
    std::os::unix::fs::symlink(&elsewhere, LifetimeLock::path(directory.path())).unwrap();

    assert!(LifetimeLock::try_acquire(directory.path()).is_err());
}

#[cfg(unix)]
#[test]
fn a_claim_never_removes_or_binds_over_a_socket_that_answers() {
    let directory = private_directory();
    let socket = directory.path().join("zmux.sock");
    // A daemon from before the lock: it answers, and holds no lock.
    let _listener = Listener::bind(&socket).unwrap();

    let error = claim_directory(directory.path(), &socket).unwrap_err();
    assert!(
        format!("{error:#}").contains("already running"),
        "{error:#}"
    );
    assert!(socket.exists());
    // Refusing also let go of the lock it took to look.
    assert!(
        LifetimeLock::try_acquire(directory.path())
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_claim_while_another_holds_the_lock_is_refused() {
    let directory = private_directory();
    let socket = directory.path().join("zmux.sock");
    let _held = LifetimeLock::try_acquire(directory.path())
        .unwrap()
        .unwrap();

    let error = claim_directory(directory.path(), &socket).unwrap_err();
    assert!(format!("{error:#}").contains("holds"), "{error:#}");
}

#[cfg(unix)]
#[test]
fn a_replacement_takes_over_the_descriptor_that_holds_the_lock() {
    use std::os::fd::IntoRawFd as _;

    let directory = private_directory();
    let lock = LifetimeLock::try_acquire(directory.path())
        .unwrap()
        .unwrap();
    // As across the `execv`: the same descriptor, inheritable so that it
    // survived, now owned by the new image.
    crate::upgrade::set_close_on_exec(std::os::fd::AsFd::as_fd(&lock.file), false).unwrap();
    let descriptor = lock.file.into_raw_fd();

    let adopted = LifetimeLock::adopt(descriptor, directory.path()).unwrap();
    assert!(
        LifetimeLock::try_acquire(directory.path())
            .unwrap()
            .is_none()
    );
    let flags = unsafe { libc::fcntl(adopted.raw_fd(), libc::F_GETFD) };
    assert_ne!(flags & libc::FD_CLOEXEC, 0);

    drop(adopted);
    assert!(
        LifetimeLock::try_acquire(directory.path())
            .unwrap()
            .is_some()
    );
}

#[cfg(unix)]
#[test]
fn a_descriptor_that_is_not_the_lock_is_not_adopted() {
    use std::os::fd::IntoRawFd as _;

    let directory = private_directory();
    let _lock = LifetimeLock::try_acquire(directory.path())
        .unwrap()
        .unwrap();
    let other = tempfile::tempfile().unwrap().into_raw_fd();

    assert!(LifetimeLock::adopt(other, directory.path()).is_err());
    // Nor one that was never inherited, which claiming would abort on.
    assert!(LifetimeLock::adopt(-1, directory.path()).is_err());
}
