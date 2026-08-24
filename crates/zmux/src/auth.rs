//! Session authentication: the Argon2id verifier a protected session is
//! reattached with, and the backoff that bounds guessing at it.
//!
//! This is deliberately free of any terminal, GPUI or platform dependency so
//! the daemon and the client can share one implementation.

use std::{sync::Arc, time::Duration};

use anyhow::{Context as _, Result};
use argon2::{
    Argon2, PasswordHash, PasswordHasher as _, PasswordVerifier as _, password_hash::SaltString,
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
    /// can never be reattached, however correct the secret.
    pub fn from_verifier(verifier: String) -> Result<Self> {
        PasswordHash::new(&verifier)
            .map_err(|error| anyhow::anyhow!("unusable session verifier: {error}"))?;
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
    pub fn verify(&self, secret: &str) -> Option<VerifiedSession> {
        PasswordHash::new(&self.verifier)
            .ok()
            .filter(|verifier| {
                Argon2::default()
                    .verify_password(secret.as_bytes(), verifier)
                    .is_ok()
            })
            .map(|_| VerifiedSession {
                verifier: self.verifier.clone(),
            })
    }

    /// Whether `authorization` was produced by verifying a secret against *this*
    /// session's verifier, rather than some other session's.
    pub fn authorizes(&self, authorization: &VerifiedSession) -> bool {
        Arc::ptr_eq(&self.verifier, &authorization.verifier)
    }

    #[cfg(test)]
    fn encoded(&self) -> &str {
        &self.verifier
    }
}

#[cfg(test)]
#[path = "tests/auth.rs"]
mod tests;
