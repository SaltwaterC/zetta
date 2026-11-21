//! Sharing one transport per target, and keeping an idle one alive for
//! `LINGER` when the process has opted in.

use super::*;

pub(super) static KEEP_IDLE: AtomicBool = AtomicBool::new(false);

/// Keeps shared transports for [`LINGER`] after their last user. Without this,
/// [`RemoteTransport::shared`] hands out a transport of the caller's own, which
/// is right for a command that exits as soon as it is done.
pub fn keep_idle_transports() {
    KEEP_IDLE.store(true, Ordering::Release);
}

/// Stops keeping idle transports and ends the SSH logins of those nobody is
/// using. For a process that called [`keep_idle_transports`], on its way out.
pub fn release_idle_transports() {
    KEEP_IDLE.store(false, Ordering::Release);
    let released = std::mem::take(&mut registry().lock().recover_poison().entries);
    drop(released);
}

/// Transports handed out by [`RemoteTransport::shared`], with when each was
/// last seen without a user.
pub(super) struct Registry {
    pub(super) entries: HashMap<RemoteTarget, SharedEntry>,
    pub(super) reaping: bool,
}

pub(super) struct SharedEntry {
    pub(super) transport: Arc<RemoteTransport>,
    pub(super) released_at: Option<Instant>,
}

pub(super) fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        Mutex::new(Registry {
            entries: HashMap::new(),
            reaping: false,
        })
    })
}

/// Drops shared transports nobody has used for [`LINGER`], and exits once
/// there are none left to watch.
pub(super) fn reap_shared_transports() {
    loop {
        thread::sleep(REAPER_INTERVAL);
        let mut expired = Vec::new();
        let finished = {
            let mut registry = registry().lock().recover_poison();
            let now = Instant::now();
            registry.entries.retain(|_, entry| {
                if Arc::strong_count(&entry.transport) > 1 {
                    entry.released_at = None;
                    return true;
                }
                let released = *entry.released_at.get_or_insert(now);
                if now.duration_since(released) < LINGER {
                    return true;
                }
                expired.push(entry.transport.clone());
                false
            });
            if registry.entries.is_empty() {
                registry.reaping = false;
            }
            !registry.reaping
        };
        // Dropping the last handle ends the SSH login, which waits for the
        // child; do it outside the registry lock.
        drop(expired);
        if finished {
            return;
        }
    }
}
