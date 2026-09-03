//! Protecting background sessions with the user's age key instead of a dialog.
//!
//! When `sessions.persistence.auto_protect` is on, detaching, keeping and
//! sharing a tab no longer ask for a secret: a 256-bit key is generated, its
//! Argon2id verifier gates the session exactly as a typed secret's would, and
//! the key itself is sealed to the configured recipients so that reattaching
//! means opening that envelope with the configured identity. See
//! [`zmux::auto_protect`] for the shape of it and why the envelope is carried
//! rather than kept in memory.
//!
//! This module is the application's half: the resolved recipients and effective identity,
//! held together so that a window can seal and open without re-reading
//! configuration or re-resolving a `github:` alias over the network on every
//! detach.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use zmux::{
    auth::SessionSecret,
    auto_protect::SealedSessionKey,
    persistence::{IdentitySet, RecipientSet},
};

use crate::config::SessionPersistenceConfig;

/// Everything automatic protection needs, resolved once.
///
/// Built when configuration is loaded rather than when a session is protected,
/// because resolving a `github:` recipient is a network fetch and detaching a tab
/// is not a moment to discover that the network is slow.
pub(crate) struct SessionAutoProtect {
    recipients: RecipientSet,
    identity_paths: Vec<PathBuf>,
}

impl SessionAutoProtect {
    /// Resolves automatic protection from configuration.
    ///
    /// `Ok(None)` means it was not asked for, or lacks a recipient or a readable
    /// identity — the settings page will not have offered the toggle in that
    /// case, but a hand-edited configuration file can still get here. `Err` means
    /// it *was* asked for and could not be set up, which the caller reports
    /// rather than swallows: silently falling back to no protection would be the
    /// one outcome the user did not choose.
    pub(crate) fn resolve(config: &SessionPersistenceConfig) -> Result<Option<Self>> {
        if !config.auto_protect_is_configured() {
            return Ok(None);
        }
        let Some(identity) = config.resolved_identity().filter(|path| path.is_file()) else {
            return Ok(None);
        };
        let recipients = zmux::persistence::resolve_recipients(&config.recipients)
            .context("resolving the recipients automatic session protection seals keys to")?;
        anyhow::ensure!(
            !recipients.is_empty(),
            "automatic session protection needs at least one recipient"
        );
        Ok(Some(Self {
            recipients,
            identity_paths: vec![identity],
        }))
    }

    /// Whether resolving would do network I/O, and so should not run on the UI
    /// thread. A plain `age1…` or `ssh-…` recipient is parsed in microseconds;
    /// only a `github:` alias is fetched.
    pub(crate) fn resolution_is_blocking(config: &SessionPersistenceConfig) -> bool {
        config
            .recipients
            .iter()
            .any(|recipient| recipient.trim().starts_with("github:"))
    }

    /// Generates a session key and the protection built from it.
    ///
    /// Argon2id runs inside, so callers put this on a background thread.
    pub(crate) fn seal(&self) -> Result<SealedSessionKey> {
        zmux::auto_protect::seal(&self.recipients)
    }

    /// Whether the effective identity is itself encrypted, and so cannot be
    /// loaded without asking someone for its passphrase.
    ///
    /// The answer decides whether an unlock can be silent. Getting this wrong is
    /// not a degraded prompt but a dead end: `age` falls back to reading a
    /// passphrase from the controlling terminal, a window has none, and the
    /// identity then decrypts nothing — reported as "No matching keys found",
    /// which names the wrong problem entirely.
    pub(crate) fn identity_passphrase_required(&self) -> Result<bool> {
        for path in &self.identity_paths {
            if zmux::persistence::identity_path_requires_passphrase(path)
                .with_context(|| format!("inspecting the identity {}", path.display()))?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Recovers the key a session was protected with, which is what proves this
    /// window controls the private key it was sealed to.
    ///
    /// `passphrase` unlocks an encrypted identity file and is positional with the
    /// configured paths, exactly as the disk-resume path supplies it — the one
    /// working example of this, and the reason that path handles an encrypted SSH
    /// key while this did not.
    pub(crate) fn open(
        &self,
        envelope: &str,
        passphrase: Option<SessionSecret>,
    ) -> Result<SessionSecret> {
        let passphrases = vec![passphrase; self.identity_paths.len()];
        // Never the terminal: this runs on the UI thread, and `age`'s fallback
        // would block it on a `/dev/tty` read nobody can see.
        let identities = IdentitySet::from_supplied_passphrases(&self.identity_paths, &passphrases)
            .context("loading the identity that opens automatically protected sessions")?;
        zmux::auto_protect::open(envelope, &identities)
    }
}

#[cfg(test)]
#[path = "tests/session_auto_protect.rs"]
mod tests;
