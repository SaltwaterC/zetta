//! Protecting a session with the user's age key pair instead of a typed secret.
//!
//! A session is normally protected by asking a person for a secret and keeping
//! its Argon2id verifier. That is the right thing when a person is the only
//! source of the secret — but a user who has already configured
//! `sessions.persistence.recipients` has a key pair that guards session state on
//! disk, and asking them to also invent and retype a passphrase protects nothing
//! the key pair does not already protect.
//!
//! So the secret is generated instead of typed:
//!
//! ```text
//! K        = 32 random bytes, base64
//! verifier = Argon2id(K)                    <- the ordinary verifier slot
//! envelope = age_encrypt(recipients, K)     <- travels beside the verifier
//! ```
//!
//! Nothing about verification changes. `K` is simply a passphrase no person could
//! remember, so [`crate::auth`] is untouched, the daemon needs no age code, and
//! there is no second authentication protocol to keep correct. The daemon never
//! stores `K`: it stores the verifier and sees `K` only while checking an attach,
//! exactly as with a typed secret.
//!
//! The envelope is an age v1 file — public ciphertext — so it is carried wherever
//! the verifier is carried, including the published catalog and the inside of the
//! encrypted record. Publishing it discloses nothing, because opening it needs the
//! private key; and it has to be carried rather than kept in memory, because a key
//! that cannot be recovered after the daemon restarts would take its session with
//! it. Its *presence* is what marks a session as auto-protected, which is why no
//! separate flag exists anywhere.
//!
//! The strength of this is the strength of the identity file. A passphrase-less
//! identity readable by the same user is the weak point — the same trade-off
//! encrypted disk retention already makes.

use anyhow::{Context as _, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD_NO_PAD};
use zeroize::Zeroizing;

use crate::{
    auth::{SessionAuthentication, SessionSecret},
    persistence::{IdentitySet, RecipientSet},
};

/// Bytes of entropy in a generated session key. At this size the Argon2id pass
/// over it is pure ceremony — there is nothing to brute-force — but it keeps the
/// verifier format identical to a typed secret's.
const KEY_BYTES: usize = 32;

/// A generated session key and the protection built from it.
///
/// The `authentication` already carries the envelope, so a caller that hands it
/// on cannot separate the verifier from the way back in. The `secret` is what
/// this process needs *now*, to act on the session it has just protected without
/// immediately reopening the envelope it only just sealed.
pub struct SealedSessionKey {
    pub authentication: SessionAuthentication,
    pub secret: SessionSecret,
}

/// Generates a session key, hashes it, and seals it to `recipients`.
///
/// Argon2id runs here, so this belongs on a background thread — the ~40 ms it
/// takes is the whole reason the interactive prompt hashes off the UI thread too.
pub fn seal(recipients: &RecipientSet) -> Result<SealedSessionKey> {
    anyhow::ensure!(
        !recipients.is_empty(),
        "automatic session protection needs at least one configured recipient"
    );
    let mut key = Zeroizing::new([0; KEY_BYTES]);
    getrandom::fill(key.as_mut_slice()).context("generating an automatic session key")?;
    // Encoded rather than raw because a secret is a string all the way through
    // the protocol, and the encoded form is what both the verifier and the
    // envelope have to agree on.
    let encoded = Zeroizing::new(STANDARD_NO_PAD.encode(key.as_slice()));
    let envelope = recipients
        .encrypt(encoded.as_bytes())
        .context("sealing the automatic session key to the configured recipients")?;
    // Binary rather than armored: this is rewritten into the catalog on every
    // publish, and armor costs a third more bytes plus newlines for no benefit.
    let envelope = STANDARD_NO_PAD.encode(&envelope);
    let authentication = SessionAuthentication::create(&encoded)
        .context("hashing the automatic session key")?
        .with_key_envelope(envelope);
    Ok(SealedSessionKey {
        authentication,
        secret: SessionSecret::from_zeroizing(encoded),
    })
}

/// Recovers the session key from an envelope, which is what proves the caller
/// controls one of the private keys it was sealed to.
///
/// The returned secret is presented to the daemon exactly as a typed one would
/// be, so a wrong identity fails here rather than producing a secret that fails
/// verification later — the two are worth distinguishing in an error message.
pub fn open(envelope: &str, identities: &IdentitySet) -> Result<SessionSecret> {
    let ciphertext = STANDARD_NO_PAD
        .decode(envelope.trim())
        .context("decoding the sealed session key")?;
    let plaintext = Zeroizing::new(
        identities
            .decrypt(&ciphertext)
            .context("opening the sealed session key with the configured identity")?,
    );
    let secret = String::from_utf8(plaintext.to_vec())
        .context("the sealed session key is not valid text")?;
    Ok(SessionSecret::from_zeroizing(Zeroizing::new(secret)))
}

#[cfg(test)]
#[path = "tests/auto_protect.rs"]
mod tests;
