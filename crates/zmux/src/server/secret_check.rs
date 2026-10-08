//! Checking a session secret without holding the sessions lock.
//!
//! Argon2 is slow on purpose — tens of milliseconds per check. Run under
//! [`Daemon::sessions`], one attach to a protected session stopped the drain,
//! every shared pane's input and resize, and every other session's controls for
//! that long, and anyone with the endpoint token could repeat it once per
//! refusal window per protected session.
//!
//! [`check_session_secret`] reads the verifier under the lock, verifies with the
//! lock released, and takes the lock again to record the outcome. Three things
//! keep that as strict as checking under the lock was:
//!
//! - One check per session at a time ([`VerificationGate`]). The refusal window
//!   bounds the guessing rate for a session only because attempts see each
//!   other's outcome; a second attempt therefore waits for the first and then
//!   meets the window it opened, rather than racing it through Argon2.
//! - A bound on checks across all sessions, because each one holds Argon2's
//!   working memory.
//! - A check only counts against the verifier it was made against. A proof is
//!   honoured through [`SessionAuthentication::authorizes`], which compares
//!   identity, so a verifier replaced mid-check invalidates it; and a failure
//!   against a replaced verifier is not charged to its replacement.
//!
//! Nothing here is cached beyond the request that asked: a [`SecretProof`]
//! lives as long as the request handler holding it.
//!
//! Resuming a protected disk record ([`check_resume_secret`]) goes through the
//! same gate, so the bound on concurrent checks covers every Argon2 run the
//! daemon makes. A record is not a live session, so its failure count and
//! refusal window are kept here too, in [`VerificationGate`], rather than on a
//! [`Session`] — and they are the daemon's own, not the ones the resuming
//! client reports from the record it decrypted.

use super::*;
use crate::auth::VerifiedSession;

/// Argon2 checks running at once, across every session.
///
/// Each holds the algorithm's working memory — at most
/// [`crate::auth::MAX_VERIFIER_MEMORY_KIB`] (19 MiB), which is what
/// [`SessionAuthentication::create`] uses and the most any accepted verifier
/// may ask for — so this caps what a burst of attempts against many sessions
/// and records can claim, while still letting a few sessions authenticate side
/// by side rather than queueing behind one another.
pub(super) const MAX_CONCURRENT_CHECKS: usize = 4;

/// How many times one check starts again because the verifier was replaced
/// under it. Reprotecting is a deliberate user action, so more than this within
/// a single check is not worth waiting out; the attempt fails uncounted.
const MAX_VERIFIER_REPLACEMENTS: usize = 2;

/// Admits the Argon2 checks: one per session, and [`MAX_CONCURRENT_CHECKS`] in
/// all.
///
/// Its own lock rather than state on [`Session`]: waiting here must never hold
/// the sessions lock, which is the whole point, and a session-held flag would
/// leave a waiter needing that lock to find out when to stop waiting.
///
/// Keyed by session id for live sessions and by record id for resumes. The two
/// share a number space — a record resumes as the session with its id, and a
/// resume of a record whose session is live is refused — so a shared key can
/// only ever serialize two checks, never let one skip the other.
#[derive(Default)]
pub(super) struct VerificationGate {
    in_flight: Mutex<HashSet<u64>>,
    released: Condvar,
    /// Failed resumes of protected disk records, by record id.
    ///
    /// The authoritative refusal window for a resume while this daemon runs.
    /// The resume request also carries the count and window persisted in the
    /// record, but that is what the client says it decrypted, so it can only
    /// make a resume wait longer, never shorter. A restart loses this, leaving
    /// only the persisted state the client reports — see [`check_resume_secret`]
    /// for why that is the limit of what an online backoff can promise.
    #[cfg(feature = "session-persistence")]
    resume_failures: Mutex<HashMap<u64, ResumeFailures>>,
}

#[cfg(feature = "session-persistence")]
#[derive(Clone, Copy)]
struct ResumeFailures {
    count: u32,
    refuse_until: Instant,
}

/// A check's place in the [`VerificationGate`], given back on drop — including
/// by a panic inside Argon2, which would otherwise lock the session's secret
/// out for the life of the daemon.
struct GateTicket<'a> {
    gate: &'a VerificationGate,
    session_id: u64,
}

impl VerificationGate {
    fn enter(&self, session_id: u64) -> GateTicket<'_> {
        let mut in_flight = self
            .in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while in_flight.contains(&session_id) || in_flight.len() >= MAX_CONCURRENT_CHECKS {
            in_flight = self
                .released
                .wait(in_flight)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        in_flight.insert(session_id);
        GateTicket {
            gate: self,
            session_id,
        }
    }
}

impl Drop for GateTicket<'_> {
    fn drop(&mut self) {
        self.gate
            .in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.session_id);
        // All, not one: the waiter that can proceed may be for a different
        // session than the first one woken.
        self.gate.released.notify_all();
    }
}

pub(super) enum SecretCheck {
    /// The secret matches the verifier the session had when it was checked.
    /// Whoever acts on it still confirms it with
    /// [`SessionAuthentication::authorizes`] under the lock it acts under.
    Verified(VerifiedSession),
    /// A wrong secret, or an attempt inside the refusal window. Reported alike,
    /// so the window cannot be probed.
    Failed,
    /// The session ended, or stopped being protected, so there was nothing to
    /// check against. The caller's own look at the session decides what that
    /// means for the request.
    NotApplicable,
}

/// Checks `secret` against session `session_id`'s verifier, and records the
/// outcome in the session's failure count and refusal window.
///
/// Takes [`Daemon::sessions`] itself and releases it while Argon2 runs, so it
/// must be called without that lock held.
pub(super) fn check_session_secret(daemon: &Daemon, session_id: u64, secret: &str) -> SecretCheck {
    // Refused before queueing: an attempt inside the window must not wait
    // behind a check whose outcome cannot change its answer.
    if let Err(outcome) = verifier_to_check(daemon, session_id) {
        return outcome;
    }
    let _ticket = daemon.verification_gate.enter(session_id);
    for _ in 0..=MAX_VERIFIER_REPLACEMENTS {
        // Read again inside the gate: the attempt this one queued behind may
        // have opened a refusal window, or replaced nothing and reset one.
        let authentication = match verifier_to_check(daemon, session_id) {
            Ok(authentication) => authentication,
            Err(outcome) => return outcome,
        };
        let verified = authentication.verify(secret);

        let mut sessions = daemon
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
            return SecretCheck::NotApplicable;
        };
        let Some(current) = session.authentication.as_ref() else {
            return SecretCheck::NotApplicable;
        };
        if !current.is_same_verifier(&authentication) {
            // Reprotected while this ran: neither a match nor a mismatch says
            // anything about the new verifier, and replacing it already reset
            // the count this would otherwise charge.
            continue;
        }
        return match verified {
            Some(verified) => {
                session.failed_authentications = 0;
                session.refuse_until = None;
                SecretCheck::Verified(verified)
            }
            None => {
                session.failed_authentications = session.failed_authentications.saturating_add(1);
                session.refuse_until = Instant::now().checked_add(
                    crate::auth::failed_authentication_delay(session.failed_authentications),
                );
                SecretCheck::Failed
            }
        };
    }
    SecretCheck::Failed
}

#[cfg(feature = "session-persistence")]
/// What checking a resume secret decided.
pub(super) enum ResumeCheck {
    Verified,
    /// Inside a refusal window: nothing was checked or counted.
    Refused,
    /// A wrong secret, now counted. The record should persist this count and
    /// window, so a later daemon starts from them.
    Failed {
        failed_authentications: u32,
        backoff_seconds: u64,
    },
}

#[cfg(feature = "session-persistence")]
/// The persisted authentication state a resume request reports for its record.
///
/// Honoured only where it is stricter than the daemon's own: it comes from the
/// client, and a client can only lie it shorter.
#[derive(Clone, Copy)]
pub(super) struct ReportedResumeBackoff {
    pub(super) failed_authentications: u32,
    /// Whether the reported window is still open.
    pub(super) refusing: bool,
}

#[cfg(feature = "session-persistence")]
/// Whether resumes of `record_id` are inside a refusal window, by the daemon's
/// own count or by the record's reported one.
pub(super) fn resume_refused(
    daemon: &Daemon,
    record_id: u64,
    reported: ReportedResumeBackoff,
) -> bool {
    reported.refusing
        || daemon
            .verification_gate
            .resume_failures
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&record_id)
            .is_some_and(|failures| Instant::now() < failures.refuse_until)
}

#[cfg(feature = "session-persistence")]
/// Checks a resume secret against the verifier of disk record `record_id`,
/// inside the [`VerificationGate`], and records the outcome in the daemon's own
/// failure count for that record.
///
/// What this bounds is online guessing through this daemon: one check per
/// record at a time, inside the shared bound on concurrent checks, and a
/// doubling refusal window between failures that the client cannot shorten.
/// It is not a bound on guessing in general. The verifier is inside the
/// record, so whoever can decrypt the record to resume it can also guess
/// against the verifier offline, at whatever rate their hardware allows; and
/// across a daemon restart the only memory of earlier failures is the
/// persisted count, which reaches this daemon through that same client. The
/// window is a rate limit for the honest path, not a defence against the
/// holder of the identity.
pub(super) fn check_resume_secret(
    daemon: &Daemon,
    record_id: u64,
    authentication: &SessionAuthentication,
    secret: &str,
    reported: ReportedResumeBackoff,
) -> ResumeCheck {
    if resume_refused(daemon, record_id, reported) {
        return ResumeCheck::Refused;
    }
    let _ticket = daemon.verification_gate.enter(record_id);
    // Again inside the gate: the resume this one queued behind may have
    // opened a window.
    if resume_refused(daemon, record_id, reported) {
        return ResumeCheck::Refused;
    }
    let verified = authentication.verify(secret).is_some();
    let mut failures = daemon
        .verification_gate
        .resume_failures
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if verified {
        failures.remove(&record_id);
        return ResumeCheck::Verified;
    }
    let count = failures
        .get(&record_id)
        .map_or(0, |failures| failures.count)
        .max(reported.failed_authentications)
        .saturating_add(1);
    let delay = crate::auth::failed_authentication_delay(count);
    let now = Instant::now();
    failures.insert(
        record_id,
        ResumeFailures {
            count,
            refuse_until: now.checked_add(delay).unwrap_or(now),
        },
    );
    ResumeCheck::Failed {
        failed_authentications: count,
        backoff_seconds: delay.as_secs(),
    }
}

/// The verifier to check a secret against, or the answer when there is none to
/// check or the session is refusing attempts.
fn verifier_to_check(
    daemon: &Daemon,
    session_id: u64,
) -> Result<SessionAuthentication, SecretCheck> {
    let sessions = daemon
        .sessions
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(session) = sessions.iter().find(|session| session.id == session_id) else {
        return Err(SecretCheck::NotApplicable);
    };
    let Some(authentication) = session.authentication.clone() else {
        return Err(SecretCheck::NotApplicable);
    };
    if session
        .refuse_until
        .is_some_and(|until| Instant::now() < until)
    {
        return Err(SecretCheck::Failed);
    }
    Ok(authentication)
}

/// What a request's session secret proved, established before the request
/// takes the sessions lock to act on it.
///
/// Held for one request and consulted through [`session_control_authorized`],
/// which honours a proof only for the verifier it was made against.
#[derive(Default)]
pub(super) struct SecretProof(Vec<VerifiedSession>);

impl SecretProof {
    /// For a request that carries no secret, or must not be authorized by one.
    pub(super) const NONE: Self = Self(Vec::new());

    pub(super) fn authorizes(&self, authentication: &SessionAuthentication) -> bool {
        self.0
            .iter()
            .any(|verified| authentication.authorizes(verified))
    }
}

/// Checks `secret` against each protected session `selects` picks out that this
/// peer could not control by identity alone — see
/// [`session_identity_authorized`].
///
/// Sessions the peer owns or holds are skipped, as `session_control_authorized`
/// used to skip them before verifying: a window controlling its own session
/// pays no Argon2 for it. If that changes before the request acts, the request
/// is refused rather than authorized, which is the safe way to lose the race.
pub(super) fn prove_session_secret(
    daemon: &Daemon,
    peer_process_id: Option<u32>,
    secret: Option<&str>,
    selects: impl Fn(&Session) -> bool,
) -> SecretProof {
    let Some(secret) = secret else {
        return SecretProof::NONE;
    };
    let session_ids: Vec<u64> = daemon
        .sessions
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .filter(|session| {
            session.authentication.is_some()
                && !session_identity_authorized(session, peer_process_id)
                && selects(session)
        })
        .map(|session| session.id)
        .collect();
    SecretProof(
        session_ids
            .into_iter()
            .filter_map(
                |session_id| match check_session_secret(daemon, session_id, secret) {
                    SecretCheck::Verified(verified) => Some(verified),
                    SecretCheck::Failed | SecretCheck::NotApplicable => None,
                },
            )
            .collect(),
    )
}

#[cfg(test)]
#[path = "../tests/server/secret_check.rs"]
pub(super) mod tests;
