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
//!
//! # What an envelope is bound to
//!
//! age proves nothing about who encrypted a file, and the envelope is opened
//! wherever it is found — including in the session list a *remote* multiplexer
//! sends, after which the recovered key goes back to that multiplexer. A remote
//! host holding another session's envelope (sealed to the same recipients)
//! could therefore offer it as one of its own and be sent that session's key.
//! So since version 2 the sealed text is not the bare key but a short record
//! that also says what the key is for and which host it belongs to:
//!
//! ```text
//! zetta-sealed-session-key/2;purpose=session-authentication;host=SHA256:…,SHA256:…;key=<K>
//! ```
//!
//! `host` is the SSH host key fingerprints of the machine that sealed it — see
//! [`host_keys`] for why that is the anchor. The whole record, not just `K`, is
//! the session's secret: the verifier is computed over it, so the binding cannot
//! be stripped off and the key sent on its own, and a Zetta from before this
//! change, which sends whatever it decrypts, still opens these envelopes.
//!
//! [`open`] is for a key going back to *this* machine's multiplexer, and accepts
//! any version. [`open_for_remote`] is for a key about to be sent to another
//! host, and refuses unless that host is one the envelope names, judged by this
//! machine's own `known_hosts` for the destination the user chose. A version 1
//! envelope — a bare key, written before the binding existed — names no host,
//! so it is refused for remote use with an explanation rather than sent on
//! trust: there is no way to tell a legitimate one from a substituted one.
//!
//! The binding names a host, not a session. Session ids are not known when a
//! key is sealed (protecting a tab comes before the multiplexer is told), and a
//! tab's protection is reused when it is detached again. Nor would it add
//! anything: a session's verifier lives in the multiplexer on the host that
//! holds it, so a key substituted between two sessions on the bound host is
//! only ever disclosed to the process that already holds both.

mod host_keys;

use anyhow::{Context as _, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD_NO_PAD};
use zeroize::Zeroizing;

use crate::{
    auth::{SessionAuthentication, SessionSecret},
    persistence::{IdentitySet, RecipientSet},
    remote::RemoteTarget,
};

/// Bytes of entropy in a generated session key. At this size the Argon2id pass
/// over it is pure ceremony — there is nothing to brute-force — but it keeps the
/// verifier format identical to a typed secret's.
const KEY_BYTES: usize = 32;

/// Opens every sealed text from version 2 on; a text without it is version 1,
/// the bare key.
const SEALED_KEY_TAG: &str = "zetta-sealed-session-key/";
const SEALED_KEY_VERSION: &str = "2";
/// What a sealed key is for. There is one purpose today; naming it means a key
/// sealed for some later purpose can never be mistaken for a session secret.
const SESSION_AUTHENTICATION_PURPOSE: &str = "session-authentication";

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

/// Generates a session key bound to this host, hashes it, and seals it to
/// `recipients`.
///
/// Argon2id runs here, so this belongs on a background thread — the ~40 ms it
/// takes is the whole reason the interactive prompt hashes off the UI thread too.
/// So does reading this host's SSH host keys.
pub fn seal(recipients: &RecipientSet) -> Result<SealedSessionKey> {
    seal_for_host(recipients, &host_keys::local_fingerprints())
}

/// [`seal`], bound to the host named by `host_keys` rather than this one.
fn seal_for_host(recipients: &RecipientSet, host_keys: &[String]) -> Result<SealedSessionKey> {
    anyhow::ensure!(
        !recipients.is_empty(),
        "automatic session protection needs at least one configured recipient"
    );
    let mut key = Zeroizing::new([0; KEY_BYTES]);
    getrandom::fill(key.as_mut_slice()).context("generating an automatic session key")?;
    // Encoded rather than raw because a secret is a string all the way through
    // the protocol, and the sealed text is what both the verifier and the
    // envelope have to agree on.
    let encoded = Zeroizing::new(STANDARD_NO_PAD.encode(key.as_slice()));
    let sealed = Zeroizing::new(format!(
        "{SEALED_KEY_TAG}{SEALED_KEY_VERSION};purpose={SESSION_AUTHENTICATION_PURPOSE};host={};key={}",
        host_keys.join(","),
        encoded.as_str()
    ));
    let envelope = recipients
        .encrypt(sealed.as_bytes())
        .context("sealing the automatic session key to the configured recipients")?;
    // Binary rather than armored: this is rewritten into the catalog on every
    // publish, and armor costs a third more bytes plus newlines for no benefit.
    let envelope = STANDARD_NO_PAD.encode(&envelope);
    let authentication = SessionAuthentication::create(&sealed)
        .context("hashing the automatic session key")?
        .with_key_envelope(envelope);
    Ok(SealedSessionKey {
        authentication,
        secret: SessionSecret::from_zeroizing(sealed),
    })
}

/// Recovers the session key from an envelope, which is what proves the caller
/// controls one of the private keys it was sealed to — for a key that goes back
/// to this machine's own multiplexer. A key about to be sent to another host
/// goes through [`open_for_remote`] instead.
///
/// The returned secret is presented to the daemon exactly as a typed one would
/// be, so a wrong identity fails here rather than producing a secret that fails
/// verification later — the two are worth distinguishing in an error message.
pub fn open(envelope: &str, identities: &IdentitySet) -> Result<SessionSecret> {
    let sealed = decrypt(envelope, identities)?;
    SealedKey::parse(&sealed)?;
    Ok(SessionSecret::from_zeroizing(sealed))
}

/// Recovers the session key from an envelope a remote host offered, but only if
/// the envelope was sealed on the host `target` is: one whose SSH host key this
/// machine's `known_hosts` trusts for that destination.
///
/// Checked before the key leaves this process, because the remote host is both
/// the one that chose which envelope to offer and the one the key is sent to. A
/// refusal here means nothing was sent.
pub fn open_for_remote(
    envelope: &str,
    identities: &IdentitySet,
    target: &RemoteTarget,
) -> Result<SessionSecret> {
    open_for_remote_with(envelope, identities, target.destination(), || {
        host_keys::known_fingerprints(target)
    })
}

/// [`open_for_remote`], with the host keys trusted for the destination supplied
/// by `trusted_host_keys` — called only once there is a bound key to compare.
pub(crate) fn open_for_remote_with(
    envelope: &str,
    identities: &IdentitySet,
    destination: &str,
    trusted_host_keys: impl FnOnce() -> Result<Vec<String>>,
) -> Result<SessionSecret> {
    let sealed = decrypt(envelope, identities)?;
    let host_keys = match SealedKey::parse(&sealed)? {
        SealedKey::Bound { host_keys } => host_keys,
        SealedKey::Unbound => anyhow::bail!(
            "this session's sealed key was made by an older Zetta and does not say which host \
             it belongs to, so it was not sent to {destination}; reconnect the session on its \
             own host, or protect it again there with this version"
        ),
    };
    anyhow::ensure!(
        !host_keys.is_empty(),
        "this session's sealed key was made on a host whose SSH host keys could not be read, so \
         it can only be opened on that host and was not sent to {destination}"
    );
    let trusted = trusted_host_keys().with_context(|| {
        format!("finding the SSH host keys known for {destination}; the session key was not sent")
    })?;
    anyhow::ensure!(
        host_keys.iter().any(|key| trusted.contains(key)),
        "this session's sealed key belongs to a different host than the one known_hosts trusts \
         for {destination}, so it was not sent; a host offering another host's session key is \
         trying to obtain it"
    );
    Ok(SessionSecret::from_zeroizing(sealed))
}

fn decrypt(envelope: &str, identities: &IdentitySet) -> Result<Zeroizing<String>> {
    let ciphertext = STANDARD_NO_PAD
        .decode(envelope.trim())
        .context("decoding the sealed session key")?;
    let mut plaintext = identities
        .decrypt(&ciphertext)
        .context("opening the sealed session key with the configured identity")?;
    match String::from_utf8(std::mem::take(&mut plaintext)) {
        Ok(text) => Ok(Zeroizing::new(text)),
        Err(error) => {
            drop(Zeroizing::new(error.into_bytes()));
            anyhow::bail!("the sealed session key is not valid text")
        }
    }
}

/// What a sealed text says about its key.
#[derive(Debug, PartialEq, Eq)]
enum SealedKey {
    /// Version 2 or later: bound to the hosts with these SSH host keys.
    Bound { host_keys: Vec<String> },
    /// Version 1: the bare key, from before envelopes were bound to anything.
    Unbound,
}

impl SealedKey {
    fn parse(sealed: &str) -> Result<Self> {
        let Some(record) = sealed.strip_prefix(SEALED_KEY_TAG) else {
            anyhow::ensure!(!sealed.is_empty(), "the sealed session key is empty");
            return Ok(Self::Unbound);
        };
        let mut fields = record.split(';');
        let version = fields.next().unwrap_or_default();
        anyhow::ensure!(
            version == SEALED_KEY_VERSION,
            "the sealed session key is version {version}, which this Zetta cannot open"
        );
        let (mut purpose, mut host, mut key) = (None, None, None);
        for field in fields {
            let (name, value) = field
                .split_once('=')
                .context("the sealed session key has a malformed field")?;
            let slot = match name {
                "purpose" => &mut purpose,
                "host" => &mut host,
                "key" => &mut key,
                _ => anyhow::bail!("the sealed session key has an unknown field {name:?}"),
            };
            anyhow::ensure!(
                slot.replace(value).is_none(),
                "the sealed session key repeats its {name:?} field"
            );
        }
        anyhow::ensure!(
            purpose == Some(SESSION_AUTHENTICATION_PURPOSE),
            "the sealed key is not a session key"
        );
        anyhow::ensure!(
            key.is_some_and(|key| !key.is_empty()),
            "the sealed session key carries no key"
        );
        let host = host.context("the sealed session key names no host")?;
        Ok(Self::Bound {
            host_keys: host
                .split(',')
                .filter(|key| !key.is_empty())
                .map(str::to_owned)
                .collect(),
        })
    }
}

#[cfg(test)]
#[path = "tests/auto_protect.rs"]
mod tests;
