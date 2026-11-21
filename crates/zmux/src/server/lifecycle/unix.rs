//! How a Unix pane's process starts: the pty, the shell-integration
//! bootstrap read back from it, and the forwarded-agent links the pane's
//! `SSH_AUTH_SOCK` resolves through.

use super::*;

/// How much of a pane's startup chatter is kept while looking for a bootstrap
/// marker. The marker cannot straddle more than one read boundary, so a fixed
/// window finds it without growing a buffer for a shell that prints a banner.
pub(super) const BOOTSTRAP_SCAN_WINDOW: usize = 8 * 1024;

/// Distinguishes one pane's bootstrap from another's, so a marker left in a
/// shell's scrollback cannot satisfy a later handshake.
pub(super) static NEXT_BOOTSTRAP_MARKER_ID: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(1);

/// How long the bootstrap below may take before the pane is started without it.
/// Generous next to a shell that normally reaches its prompt in a few tens of
/// milliseconds, and bounded because nothing may wedge a spawn.
pub(super) const SHELL_INTEGRATION_BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(3);

/// Loads Zetta's shell integration into a pane before anybody can see it.
///
/// The wrapper is delivered as if typed, so the terminal echoes it — to *every*
/// viewer of a shared pane, not just whoever wrote it. A viewer that did not
/// write it has no idea what it is looking at, and hiding it viewer-side is a
/// race against the replay that delivers the whole exchange at once. Doing it
/// here avoids the question: the pane has no attachment and nothing retained
/// yet, so the exchange reaches nobody and is in no replay.
///
/// Best effort by construction. A shell that never answers leaves the pane
/// without its integration, which is what it would have had anyway.
pub(super) fn load_shell_integration(
    pty: &tty::Pty,
    command: &zetta_profiles::ProfileCommand,
) -> Result<()> {
    use std::io::Write as _;

    let kind = command.shell_kind();
    if zetta_profiles::runs_a_command(&command.args) {
        return Ok(());
    }
    let Some(payload) = zetta_profiles::shell_integration_startup_command(kind, &command.args)
    else {
        return Ok(());
    };
    let marker_id = NEXT_BOOTSTRAP_MARKER_ID.fetch_add(1, Ordering::Relaxed);
    let Some(wrapper) = zetta_profiles::init_command_wrapper(kind, marker_id) else {
        return Ok(());
    };

    let deadline = Instant::now() + SHELL_INTEGRATION_BOOTSTRAP_TIMEOUT;
    let mut file = pty
        .file()
        .try_clone()
        .context("cloning the pane's terminal")?;
    file.write_all(wrapper.as_bytes())?;
    file.write_all(b"\r")?;
    file.flush()?;
    if !read_until(
        &mut file,
        &zetta_profiles::init_command_marker(marker_id),
        deadline,
    )? {
        log::debug!("pane did not ask for its shell integration in time");
        return Ok(());
    }
    file.write_all(&payload)?;
    file.flush()?;
    if !read_until(
        &mut file,
        &zetta_profiles::init_command_done_title(marker_id),
        deadline,
    )? {
        log::debug!("pane did not finish loading its shell integration in time");
    }
    Ok(())
}

/// Reads until `needle` appears or the deadline passes, discarding everything.
///
/// Only the tail is kept, because the needle cannot straddle more than one
/// boundary: a fixed window is enough to find it and keeps a chatty startup
/// from growing a buffer nobody reads.
pub(super) fn read_until(
    file: &mut std::fs::File,
    needle: &str,
    deadline: Instant,
) -> Result<bool> {
    use std::io::Read as _;
    use std::os::fd::AsRawFd as _;

    let mut tail = String::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(false);
        }
        let mut poll = libc::pollfd {
            fd: file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one initialized `pollfd` describing a descriptor this
        // function owns a handle to, with a bounded millisecond timeout.
        let ready = unsafe {
            libc::poll(
                &raw mut poll,
                1,
                i32::try_from(left.as_millis()).unwrap_or(i32::MAX),
            )
        };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error).context("waiting for the pane's terminal");
        }
        if ready == 0 {
            return Ok(false);
        }
        let read = match file.read(&mut buffer) {
            Ok(0) => return Ok(false),
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error).context("reading the pane's terminal"),
        };
        tail.push_str(&String::from_utf8_lossy(&buffer[..read]));
        if tail.contains(needle) {
            return Ok(true);
        }
        if tail.len() > BOOTSTRAP_SCAN_WINDOW {
            let keep = tail.len() - needle.len().max(1);
            tail.drain(..tail.floor_char_boundary(keep));
        }
    }
}

/// Starts a pane's process, never while an upgrade has made the descriptors it
/// carries inheritable: the shell would hold every other pane's terminal.
pub(super) fn start_terminal(
    daemon: &Daemon,
    options: &tty::Options,
    size: TerminalSize,
) -> std::io::Result<tty::Pty> {
    let _no_upgrade = daemon.spawn_gate.read().recover_poison();
    tty::new(options, window_size(size), 0)
}

pub(super) fn pane_agent_fallback(env: &HashMap<String, String>) -> PathBuf {
    env.get("SSH_AUTH_SOCK")
        .filter(|path| !path.is_empty())
        .map_or_else(crate::paths::forwarded_agent_socket, PathBuf::from)
}

pub(super) fn prepare_pane_agent_links(pane_id: u64, fallback_target: &Path) {
    use std::os::unix::fs::symlink;

    let path = crate::paths::pane_forwarded_agent_socket(pane_id);
    let fallback = crate::paths::pane_forwarded_agent_fallback(pane_id);
    let install = |target: &Path, path: &Path| {
        let temporary = path.with_extension(format!("new.{}", std::process::id()));
        let _ = std::fs::remove_file(&temporary);
        symlink(target, &temporary).and_then(|()| std::fs::rename(&temporary, path))
    };
    let result = install(fallback_target, &fallback).and_then(|()| install(&fallback, &path));
    if let Err(error) = result {
        log::debug!("could not prepare pane {pane_id}'s forwarded-agent links: {error}");
    }
}

/// Removes the pair of links [`prepare_pane_agent_links`] made for a pane.
///
/// Called where the daemon discards a pane, and nowhere else: not from a
/// `Drop`, because an in-place upgrade hands live panes to its replacement and
/// the old process must not take their links with it. Before this, every pane
/// a daemon had ever started left its two links behind in the session
/// directory.
pub(in crate::server) fn remove_pane_agent_links(pane_id: u64) {
    for path in [
        crate::paths::pane_forwarded_agent_socket(pane_id),
        crate::paths::pane_forwarded_agent_fallback(pane_id),
    ] {
        if let Err(error) = std::fs::remove_file(&path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            log::debug!("could not remove {}: {error}", path.display());
        }
    }
}

/// How long a shared client's socket write may stall before its relay gives up
/// and stops relaying to it.
///
/// A backstop, and deliberately the same patience as [`RELAY_STALL_TIMEOUT`]: both
/// answer "this viewer has accepted nothing for a while", so they should not
/// disagree about how long a while is. When this was five times more patient it won
/// the race often enough to matter — a viewer that had stopped reading held its
/// pane for four and a half seconds instead of the one the stall rule intends,
/// because the write blocked before the backlog had grown enough to be noticed.
///
/// It bounds that one client's relay thread and nothing else. It used to bound the
/// *drain* thread, because the relay wrote to every viewer's socket while holding
/// the sessions lock — so a viewer that stopped reading froze every pane the daemon
/// holds. The queue in [`SharedClient::relay`] is what took the drain out of that
/// path.
pub(in crate::server) const RELAY_WRITE_TIMEOUT: Duration = RELAY_STALL_TIMEOUT;
