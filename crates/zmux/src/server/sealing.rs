//! Which recipients a session's disk records are encrypted to.
//!
//! `Configure` replaces the store's recipients, and any same-user process
//! holding the endpoint token may send it. That is harmless for an unprotected
//! session, which the same process could simply attach, but it must not be how
//! a protected session's verifier, state and scrollback reach somebody else's
//! key. So a protected session is pinned to recipients one of its own clients
//! chose — the owner or a holder, as the kernel or a Windows attestation
//! vouched for it — and a later `Configure` from anybody else changes only the
//! unprotected sessions. A protected session nobody vouched-for has configured
//! for is withheld from disk rather than written under a guess.
//!
//! The pin lives on the [`Session`], because the store is replaced on every
//! `Configure`; the store only holds the decision, re-applied from here.

use serde::{Deserialize, Serialize};

use super::*;

/// How many clients' choices are remembered. Each window configures once per
/// connection, so this is far beyond the windows a user has open; the oldest
/// is forgotten first, which only means a session it protects later is
/// withheld until it configures again.
#[cfg(feature = "session-persistence")]
const MAX_GRANTS: usize = 64;

/// The recipients each client the daemon could vouch for last configured.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecipientGrants {
    /// What the daemon was started with. Trusted for a session protected
    /// before any client has configured — a test, or a daemon started for a
    /// one-off command — and withdrawn by the first `Configure`, since every
    /// client sends one as it connects.
    #[serde(default)]
    pub startup: Option<Vec<String>>,
    /// Resolved recipients by process, most recent last.
    #[serde(default)]
    pub by_process: Vec<(u32, Vec<String>)>,
}

impl RecipientGrants {
    pub(super) fn starting_with(recipients: Option<Vec<String>>) -> Self {
        Self {
            startup: recipients.filter(|recipients| !recipients.is_empty()),
            by_process: Vec::new(),
        }
    }

    /// Records a `Configure`. Unvouched-for, it still ends the startup trust,
    /// because the store it describes has changed.
    ///
    /// Evicting the oldest past the bound can only leave a session protected
    /// later withheld from disk, never sealed to somebody else's key.
    #[cfg(feature = "session-persistence")]
    fn configured(
        &mut self,
        peer: Option<u32>,
        recipients: &[String],
        is_running: impl Fn(u32) -> bool,
    ) {
        self.startup = None;
        let Some(peer) = peer else { return };
        self.forget_exited(is_running);
        self.by_process.retain(|(process, _)| *process != peer);
        self.by_process.push((peer, recipients.to_vec()));
        if self.by_process.len() > MAX_GRANTS {
            self.by_process.remove(0);
        }
    }

    /// What a session protected by `peer`, and owned by `owner`, may be
    /// sealed to: what `peer` configured, or failing that what its owner did —
    /// `zmux share` protects a window's session without configuring anything
    /// itself — or the startup recipients nobody has replaced yet.
    pub(super) fn for_session(&self, peer: Option<u32>, owner: Option<u32>) -> Option<Vec<String>> {
        [peer, owner]
            .into_iter()
            .flatten()
            .find_map(|process| self.configured_by(process))
            .map(<[String]>::to_vec)
            .filter(|recipients| !recipients.is_empty())
            .or_else(|| self.startup.clone())
    }

    fn configured_by(&self, process: u32) -> Option<&[String]> {
        self.by_process
            .iter()
            .find(|(configured, _)| *configured == process)
            .map(|(_, recipients)| recipients.as_slice())
    }

    /// Forgets what exited processes configured, so a process that later
    /// reuses one of their IDs does not inherit the choice.
    fn forget_exited(&mut self, is_running: impl Fn(u32) -> bool) {
        self.by_process.retain(|(process, _)| is_running(*process));
    }
}

/// What `session`'s writes are sealed to right now.
#[cfg(feature = "session-persistence")]
pub(super) fn seal_for(session: &Session) -> crate::persistence::Seal {
    use crate::persistence::Seal;
    if session.authentication.is_none() {
        return Seal::Store;
    }
    session
        .sealed_to
        .clone()
        .map_or(Seal::Withheld, Seal::Recipients)
}

/// Pins a session `peer` has just protected; see
/// [`RecipientGrants::for_session`]. Left as it was when nobody it could use
/// configured anything, so re-protecting a session does not unpin it.
///
/// Protecting is the one explicit choice that re-pins a pinned session; see
/// [`configure_seals`] for why configuring is not.
pub(super) fn pin_protected(daemon: &Daemon, session: &mut Session, peer: Option<u32>) {
    if session.authentication.is_none() {
        return;
    }
    let mut grants = lock_grants(daemon);
    grants.forget_exited(crate::process_status::is_running);
    if let Some(recipients) = grants.for_session(peer, session.owner) {
        session.sealed_to = Some(recipients);
    }
}

/// Pins a session `peer` has just protected, and tells the store, so output
/// already queued for it is not written under the seal it had unprotected.
pub(super) fn protected_by(daemon: &Daemon, session: &mut Session, peer: Option<u32>) {
    pin_protected(daemon, session, peer);
    #[cfg(feature = "session-persistence")]
    reseal(daemon, session);
}

/// Records a `Configure` from `peer`, and pins the protected sessions it owns
/// or holds that nothing has pinned yet. Called under the session lock, before
/// the new store opens.
///
/// A pinned session is never re-pinned here. A window configures whenever its
/// configuration is reloaded, and anything holding the control socket's token
/// can both edit that file and ask for the reload; re-pinning would let it
/// choose the key the window's own protected sessions are written to.
#[cfg(feature = "session-persistence")]
pub(super) fn configure_seals(
    daemon: &Daemon,
    sessions: &mut [Session],
    peer: Option<u32>,
    recipients: &[String],
) {
    lock_grants(daemon).configured(peer, recipients, crate::process_status::is_running);
    if recipients.is_empty() {
        return;
    }
    for session in sessions {
        if session.authentication.is_some()
            && session.sealed_to.is_none()
            && session_identity_authorized(session, peer)
        {
            session.sealed_to = Some(recipients.to_vec());
        }
    }
}

/// Tells the store what one session is sealed to now. Skipped while nothing
/// is being persisted, which spares a spawn the wait for queued output: the
/// `Configure` that enables persistence reseals every session itself.
#[cfg(feature = "session-persistence")]
pub(super) fn reseal(daemon: &Daemon, session: &Session) {
    if !daemon.persistence.enabled() {
        return;
    }
    if let Some(store) = daemon.persistence.lock().as_mut() {
        seal_in(store, session);
    }
}

/// Tells a store what every session is sealed to.
#[cfg(feature = "session-persistence")]
pub(super) fn reseal_all(store: &mut PersistenceStore, sessions: &[Session]) {
    for session in sessions {
        seal_in(store, session);
    }
}

#[cfg(feature = "session-persistence")]
fn seal_in(store: &mut PersistenceStore, session: &Session) {
    if let Err(error) = store.seal(session.id, &seal_for(session)) {
        // Unparseable recipients cannot be written to, so nothing is.
        log::warn!(
            "could not seal session {} to its recipients: {error:#}",
            session.id
        );
        let _ = store.seal(session.id, &crate::persistence::Seal::Withheld);
    }
}

/// Grants for the daemon an upgrade replaced. One from before grants were
/// carried trusted whatever its store held, so the replacement trusts that
/// once, as startup, and pins the protected sessions it adopts to it.
pub(super) fn adopted_grants(
    grants: Option<RecipientGrants>,
    sessions: &mut [Session],
    store_recipients: Option<&[String]>,
) -> RecipientGrants {
    if let Some(grants) = grants {
        return grants;
    }
    let startup = store_recipients
        .filter(|recipients| !recipients.is_empty())
        .map(<[String]>::to_vec);
    for session in sessions {
        if session.authentication.is_some() && session.sealed_to.is_none() {
            session.sealed_to.clone_from(&startup);
        }
    }
    RecipientGrants::starting_with(startup)
}

/// Installs the grants an upgrade carried, and tells the store what every
/// adopted session is sealed to.
pub(super) fn adopt_seals(daemon: &Daemon, grants: Option<RecipientGrants>) {
    let mut sessions = daemon
        .sessions
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    #[cfg(feature = "session-persistence")]
    let mut persistence = daemon.persistence.lock();
    #[cfg(feature = "session-persistence")]
    let store_recipients = persistence
        .as_ref()
        .map(|store| store.recipient_values().to_vec());
    #[cfg(not(feature = "session-persistence"))]
    let store_recipients: Option<Vec<String>> = None;
    let grants = adopted_grants(grants, &mut sessions, store_recipients.as_deref());
    *lock_grants(daemon) = grants;
    #[cfg(feature = "session-persistence")]
    if let Some(store) = persistence.as_mut() {
        reseal_all(store, &sessions);
    }
}

pub(super) fn lock_grants(daemon: &Daemon) -> std::sync::MutexGuard<'_, RecipientGrants> {
    daemon
        .recipient_grants
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// Each test configures grants, which only a persisting build can.
#[cfg(all(test, feature = "session-persistence"))]
#[path = "../tests/server/sealing.rs"]
mod tests;
