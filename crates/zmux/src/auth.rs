//! Session authentication: the Argon2id verifier a protected session is
//! reattached with, and the backoff that bounds guessing at it.
//!
//! This is deliberately free of any terminal, GPUI or platform dependency so
//! the daemon and the client can share one implementation.

use std::{sync::Arc, time::Duration};

use anyhow::{Context as _, Result};
use argon2::{
    Algorithm, Argon2, Params, PasswordHash, PasswordHasher as _, PasswordVerifier as _, Version,
    password_hash::SaltString,
};
use subtle::ConstantTimeEq as _;
use zeroize::Zeroizing;

/// How long a session refuses reconnect attempts after one wrong secret, and
/// the ceiling that doubling reaches.
///
/// The window is enforced by *rejecting* early attempts rather than sleeping on
/// them. Sleeping would hold the process control thread, which answers one
/// request at a time, so a wrong secret could be used deliberately to stall
/// every other control command for the length of the backoff. Rejecting costs
/// an attacker exactly the same waiting time and costs everyone else nothing.
const FAILED_AUTHENTICATION_DELAY: Duration = Duration::from_secs(1);
const MAX_FAILED_AUTHENTICATION_DELAY: Duration = Duration::from_secs(30);

/// The refusal window after `failures` consecutive wrong secrets: doubling from
/// [`FAILED_AUTHENTICATION_DELAY`] up to [`MAX_FAILED_AUTHENTICATION_DELAY`].
///
/// Attempts serialize through the control socket, so this is a global bound on
/// the guessing rate for a session, not a per-connection one.
pub fn failed_authentication_delay(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(u32::BITS - 1);
    FAILED_AUTHENTICATION_DELAY
        .saturating_mul(1_u32.checked_shl(doublings).unwrap_or(u32::MAX))
        .min(MAX_FAILED_AUTHENTICATION_DELAY)
}

/// The most an accepted verifier may ask one check to spend, as Argon2's
/// memory in KiB, passes and lanes.
///
/// Exactly what [`SessionAuthentication::create`] produces — argon2 0.5's
/// defaults: 19 MiB, two passes, one lane — and so what every verifier this
/// code has ever written uses, persisted and handed-over records included.
/// Verification takes its costs from the verifier string rather than from this
/// process, so without a ceiling a client could protect its own session with a
/// verifier that costs gigabytes and minutes, then trigger it as often as the
/// refusal window allows. With one, a check costs at most what a check of an
/// ordinary session costs, and the daemon's bound on concurrent checks is a
/// bound on memory.
///
/// Raising a default here (or in a future argon2) means raising this too:
/// `a_created_verifier_is_within_the_import_policy` fails otherwise.
pub const MAX_VERIFIER_MEMORY_KIB: u32 = Params::DEFAULT_M_COST;
const MAX_VERIFIER_PASSES: u32 = Params::DEFAULT_T_COST;
const MAX_VERIFIER_LANES: u32 = Params::DEFAULT_P_COST;
/// The hash length [`SessionAuthentication::create`] writes.
const VERIFIER_OUTPUT_BYTES: usize = Params::DEFAULT_OUTPUT_LEN;
/// Salt lengths accepted. `create` writes 16 bytes; the upper end is the PHC
/// string format's own limit.
const MIN_VERIFIER_SALT_BYTES: usize = 16;
const MAX_VERIFIER_SALT_BYTES: usize = 64;

/// Parses `verifier` and checks it against the import policy: Argon2id,
/// version 0x13, no secret key id or associated data, a 16–64 byte salt, a
/// 32-byte hash, and costs no higher than [`MAX_VERIFIER_MEMORY_KIB`],
/// [`MAX_VERIFIER_PASSES`] and [`MAX_VERIFIER_LANES`].
///
/// Only parsing happens here: nothing is hashed, so a verifier asking for an
/// unbounded cost is refused without that cost ever being paid.
fn policy_checked(verifier: &str) -> Result<PasswordHash<'_>> {
    let hash = PasswordHash::new(verifier)
        .map_err(|error| anyhow::anyhow!("unusable session verifier: {error}"))?;
    anyhow::ensure!(
        Algorithm::try_from(hash.algorithm).ok() == Some(Algorithm::Argon2id),
        "session verifiers must use Argon2id"
    );
    anyhow::ensure!(
        hash.version
            .and_then(|version| Version::try_from(version).ok())
            == Some(Version::V0x13),
        "session verifiers must name Argon2 version 19"
    );
    let params = Params::try_from(&hash)
        .map_err(|error| anyhow::anyhow!("unusable session verifier parameters: {error}"))?;
    anyhow::ensure!(
        params.m_cost() <= MAX_VERIFIER_MEMORY_KIB
            && params.t_cost() <= MAX_VERIFIER_PASSES
            && params.p_cost() <= MAX_VERIFIER_LANES,
        "session verifier costs m={}, t={}, p={} exceed the accepted m={MAX_VERIFIER_MEMORY_KIB}, \
         t={MAX_VERIFIER_PASSES}, p={MAX_VERIFIER_LANES}",
        params.m_cost(),
        params.t_cost(),
        params.p_cost(),
    );
    anyhow::ensure!(
        params.keyid().is_empty() && params.data().is_empty(),
        "session verifiers must not carry a key id or associated data"
    );
    anyhow::ensure!(
        hash.hash
            .as_ref()
            .is_some_and(|output| output.len() == VERIFIER_OUTPUT_BYTES),
        "session verifiers must carry a {VERIFIER_OUTPUT_BYTES}-byte hash"
    );
    let mut salt = [0; MAX_VERIFIER_SALT_BYTES];
    let salt_bytes = hash
        .salt
        .and_then(|salt_string| salt_string.decode_b64(&mut salt).ok())
        .map_or(0, <[u8]>::len);
    anyhow::ensure!(
        (MIN_VERIFIER_SALT_BYTES..=MAX_VERIFIER_SALT_BYTES).contains(&salt_bytes),
        "session verifiers must carry a {MIN_VERIFIER_SALT_BYTES} to {MAX_VERIFIER_SALT_BYTES} byte salt"
    );
    Ok(hash)
}

/// A session secret in transit between the CLI and the process that owns the
/// session. The inner value is zeroized on drop and never rendered by `Debug`,
/// so it cannot leak through a derived `Debug` on a containing message type.
#[derive(Clone, Default, Eq)]
pub struct SessionSecret(Zeroizing<String>);

impl SessionSecret {
    pub fn new(secret: String) -> Self {
        Self(Zeroizing::new(secret))
    }

    /// Takes ownership of an already-protected buffer. Used by the CLI prompt,
    /// which accumulates the typed secret in place: copying it out to call
    /// [`Self::new`] would leave the plaintext behind in freed memory.
    pub fn from_zeroizing(secret: Zeroizing<String>) -> Self {
        Self(secret)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SessionSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SessionSecret(<redacted>)")
    }
}

impl PartialEq for SessionSecret {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_bytes().ct_eq(other.0.as_bytes()).into()
    }
}

#[derive(Clone)]
pub struct SessionAuthentication {
    verifier: Arc<str>,
    /// The sealed session key, when the secret was generated rather than typed —
    /// see [`crate::auto_protect`].
    ///
    /// Held here rather than passed alongside because the two are only
    /// meaningful together: a verifier whose envelope went missing is a session
    /// nobody can open, and an envelope whose verifier was replaced is a way in
    /// that opens nothing. Every path that carries protection from one process to
    /// another therefore carries both without having to remember to.
    ///
    /// Never read by this module. It is public ciphertext, and opening it needs
    /// an age identity that the process holding a session deliberately lacks.
    key_envelope: Option<Arc<str>>,
}

/// Proof that a secret was checked against a session's verifier. It can only be
/// produced by [`SessionAuthentication::verify`], so a caller holding one has
/// necessarily authenticated rather than merely obtained a verifier clone.
#[derive(Clone)]
pub struct VerifiedSession {
    verifier: Arc<str>,
}

impl SessionAuthentication {
    pub fn create(secret: &str) -> Result<Self> {
        anyhow::ensure!(
            !secret.is_empty(),
            "session authentication must not be empty"
        );
        let mut salt = [0; 16];
        getrandom::fill(&mut salt).context("generating session authentication salt")?;
        let salt = SaltString::encode_b64(&salt)
            .map_err(|error| anyhow::anyhow!("encoding session authentication salt: {error}"))?;
        let verifier = Argon2::default()
            .hash_password(secret.as_bytes(), &salt)
            .map_err(|error| anyhow::anyhow!("hashing session authentication: {error}"))?
            .to_string()
            .into();
        Ok(Self {
            verifier,
            key_envelope: None,
        })
    }

    /// Records the sealed session key that can give this verifier's secret back
    /// to whoever holds the matching age identity.
    ///
    /// Consuming rather than assigning, so an envelope can only be attached at
    /// the point a verifier is built from a key that was actually sealed.
    pub fn with_key_envelope(mut self, envelope: impl Into<Arc<str>>) -> Self {
        self.key_envelope = Some(envelope.into());
        self
    }

    /// Rebuilds a verifier created elsewhere — the application hashes the
    /// secret away from its UI thread and sends only the result, so the
    /// plaintext never crosses the socket.
    ///
    /// The encoding is validated here rather than at the first attach: storing
    /// something unparseable would leave a session that looks protected and
    /// can never be reattached, however correct the secret. So is the cost
    /// policy (see [`MAX_VERIFIER_MEMORY_KIB`]), because the verifier is
    /// supplied by whoever protects the session and its costs are what every
    /// later check of it will pay.
    pub fn from_verifier(verifier: String) -> Result<Self> {
        policy_checked(&verifier)?;
        Ok(Self {
            verifier: verifier.into(),
            key_envelope: None,
        })
    }

    /// Rebuilds a verifier handed over by the multiplexer this one replaces,
    /// keeping a session whose verifier fails the cost policy rather than
    /// failing the whole handover over it.
    ///
    /// Such a verifier can only exist because a multiplexer from before the
    /// policy accepted it from a client, so the session it protects stays
    /// protected but can no longer be opened by a secret: [`Self::verify`]
    /// refuses it without running Argon2. Its owner can still close it. Unlike
    /// [`Self::from_verifier`], only an unparseable verifier is an error.
    pub fn adopt_verifier(verifier: String) -> Result<Self> {
        if let Err(error) = policy_checked(&verifier) {
            PasswordHash::new(&verifier)
                .map_err(|error| anyhow::anyhow!("unusable session verifier: {error}"))?;
            log::warn!("adopted a session whose verifier no secret can open any more: {error:#}");
        }
        Ok(Self {
            verifier: verifier.into(),
            key_envelope: None,
        })
    }

    /// The encoded verifier, for handing to the process that will hold the
    /// session. This is a hash, not a secret, but it is still never published
    /// in the catalog.
    pub fn verifier(&self) -> &str {
        &self.verifier
    }

    /// The sealed session key, when this session was protected automatically.
    ///
    /// `None` for a typed secret: there is nothing to recover, because only the
    /// person who chose it knows it.
    pub fn key_envelope(&self) -> Option<&str> {
        self.key_envelope.as_deref()
    }

    /// Checks `secret` against this verifier, returning proof of the check on
    /// success. Returning [`VerifiedSession`] rather than `bool` is what keeps
    /// authorization and authentication from drifting apart: the only way to
    /// obtain the value a reattach demands is to pass a correct secret through
    /// here.
    ///
    /// The cost policy is checked again first, without hashing. Every
    /// constructor but [`Self::adopt_verifier`] has already enforced it, and
    /// that one exists precisely to keep a verifier that fails it from ever
    /// being run.
    pub fn verify(&self, secret: &str) -> Option<VerifiedSession> {
        let verifier = policy_checked(&self.verifier).ok()?;
        let check = || {
            Argon2::default()
                .verify_password(secret.as_bytes(), &verifier)
                .is_ok()
        };
        #[cfg(test)]
        let matched = verification_probe::observe(&self.verifier, check);
        #[cfg(not(test))]
        let matched = check();
        matched.then(|| VerifiedSession {
            verifier: self.verifier.clone(),
        })
    }

    /// Whether `authorization` was produced by verifying a secret against *this*
    /// session's verifier, rather than some other session's.
    pub fn authorizes(&self, authorization: &VerifiedSession) -> bool {
        Arc::ptr_eq(&self.verifier, &authorization.verifier)
    }

    /// Whether `other` is this same verifier rather than a replacement for it —
    /// by identity, as [`Self::authorizes`] is, so a session reprotected with
    /// the very same secret still counts as replaced.
    pub fn is_same_verifier(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.verifier, &other.verifier)
    }

    #[cfg(test)]
    fn encoded(&self) -> &str {
        &self.verifier
    }
}

/// Counts the Argon2 runs made against chosen verifiers, so a test can see how
/// many ran at once rather than inferring it from timing.
///
/// Keyed by the verifier string, which carries a random salt, so tests running
/// in parallel never observe each other's checks.
#[cfg(test)]
pub(crate) mod verification_probe {
    use std::{
        collections::HashMap,
        sync::{
            Arc, LazyLock, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    #[derive(Default)]
    pub(crate) struct Probe {
        in_flight: AtomicUsize,
        peak: AtomicUsize,
        runs: AtomicUsize,
        /// Extra time each run is held for, so overlapping callers overlap.
        hold: Duration,
    }

    impl Probe {
        /// Only the resume tests measure overlap, and they need the daemon
        /// fixture that exists only where they are compiled.
        #[cfg(all(unix, not(target_os = "macos"), feature = "session-persistence"))]
        pub(crate) fn peak(&self) -> usize {
            self.peak.load(Ordering::SeqCst)
        }

        pub(crate) fn runs(&self) -> usize {
            self.runs.load(Ordering::SeqCst)
        }
    }

    static PROBES: LazyLock<Mutex<HashMap<String, Arc<Probe>>>> = LazyLock::new(Default::default);

    pub(crate) fn watch(verifier: &str, hold: Duration) -> Arc<Probe> {
        let probe = Arc::new(Probe {
            hold,
            ..Probe::default()
        });
        PROBES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(verifier.to_owned(), Arc::clone(&probe));
        probe
    }

    pub(super) fn observe(verifier: &str, check: impl FnOnce() -> bool) -> bool {
        let probe = PROBES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(verifier)
            .cloned();
        let Some(probe) = probe else {
            return check();
        };
        let running = probe.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        probe.peak.fetch_max(running, Ordering::SeqCst);
        probe.runs.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(probe.hold);
        let matched = check();
        probe.in_flight.fetch_sub(1, Ordering::SeqCst);
        matched
    }
}

#[cfg(test)]
#[path = "tests/auth.rs"]
mod tests;
