//! The client-side identities a stored session is decrypted with.
//!
//! An identity file may itself be passphrase-encrypted, so an identity is
//! resolved lazily: the passphrase is asked for only when a decryption
//! actually needs that file, and the answer is remembered for the set.

use super::*;

/// A set of client-side age identities.
pub struct IdentitySet {
    pub(super) identities: Vec<Box<dyn age::Identity + Send + Sync>>,
}

impl fmt::Debug for IdentitySet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IdentitySet")
            .field("count", &self.identities.len())
            .finish()
    }
}

/// Where a passphrase comes from when an identity file turns out to be
/// encrypted and the caller supplied none.
///
/// A command line may ask at the controlling terminal. A window may not: `age`'s
/// callbacks read `/dev/tty` directly, so in a window that either finds nothing
/// or — when the window was launched from a terminal — blocks on input nobody
/// can see they are being asked for, with the UI thread waiting on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassphraseSource {
    /// Ask at the controlling terminal when nothing was supplied.
    Terminal,
    /// Use only what the caller supplied; a missing passphrase is a failure.
    ///
    /// This is what every caller inside a window wants, and it is what makes an
    /// undetected encrypted identity a prompt error rather than a hang.
    SuppliedOnly,
}

impl IdentitySet {
    /// Loads native age, PQ, and SSH identity files. Files may be repeated by
    /// callers in the same way `age -i` is repeatable.
    ///
    /// An encrypted file is asked about at the controlling terminal, so this is
    /// for command lines only — see [`Self::from_supplied_passphrases`].
    pub fn from_paths(paths: &[PathBuf]) -> Result<Self> {
        Self::from_paths_with_passphrases(paths, &[])
    }

    /// Loads identity files with positional passphrases collected by a caller,
    /// asking at the controlling terminal for any that are missing.
    pub fn from_paths_with_passphrases(
        paths: &[PathBuf],
        passphrases: &[Option<SessionSecret>],
    ) -> Result<Self> {
        Self::load(paths, passphrases, PassphraseSource::Terminal)
    }

    /// Loads identity files using only the passphrases the caller collected.
    ///
    /// The loader for a window: an encrypted identity with no passphrase fails
    /// here instead of reaching for a terminal that either is not there or is
    /// not being watched.
    pub fn from_supplied_passphrases(
        paths: &[PathBuf],
        passphrases: &[Option<SessionSecret>],
    ) -> Result<Self> {
        Self::load(paths, passphrases, PassphraseSource::SuppliedOnly)
    }

    fn load(
        paths: &[PathBuf],
        passphrases: &[Option<SessionSecret>],
        source: PassphraseSource,
    ) -> Result<Self> {
        let mut identities = Vec::new();
        for (index, path) in paths.iter().enumerate() {
            let passphrase = passphrases
                .get(index)
                .and_then(Option::as_ref)
                .map(|passphrase| age::secrecy::SecretString::from(passphrase.expose()));
            load_identity_file(path, &mut identities, passphrase, source)?;
        }
        anyhow::ensure!(!identities.is_empty(), "no identities were found");
        Ok(Self { identities })
    }

    /// Opens an age file with whichever of these identities fits it, armored or
    /// binary. The counterpart of [`Self::from_paths`], and the only way in to
    /// anything this store wrote.
    pub fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        let decryptor =
            age::Decryptor::new_buffered(age::armor::ArmoredReader::new(Cursor::new(ciphertext)))
                .context("parsing age ciphertext")?;
        let identities = self
            .identities
            .iter()
            .map(|identity| identity.as_ref() as &dyn age::Identity);
        let mut reader = decryptor
            .decrypt(identities)
            .context("decrypting age ciphertext")?;
        let mut plaintext = Vec::new();
        reader
            .read_to_end(&mut plaintext)
            .context("reading age plaintext")?;
        Ok(plaintext)
    }

    /// Decrypts an age file a chunk at a time. Persistence reconstruction uses
    /// this for scrollback so the encrypted history never has to be assembled
    /// in memory before it reaches the bounded terminal screen.
    pub(super) fn decrypt_file(
        &self,
        path: &Path,
        mut consume: impl FnMut(&[u8]) -> Result<()>,
    ) -> Result<()> {
        let file = fs::File::open(path)
            .with_context(|| format!("reading encrypted scrollback {}", path.display()))?;
        let decryptor = age::Decryptor::new_buffered(age::armor::ArmoredReader::new(file))
            .context("parsing age scrollback ciphertext")?;
        let identities = self
            .identities
            .iter()
            .map(|identity| identity.as_ref() as &dyn age::Identity);
        let mut reader = decryptor
            .decrypt(identities)
            .context("decrypting age scrollback ciphertext")?;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = reader
                .read(&mut buffer)
                .context("reading decrypted age scrollback")?;
            if read == 0 {
                break;
            }
            consume(&buffer[..read])?;
        }
        Ok(())
    }
}

#[derive(Clone)]
struct PromptCallbacks {
    passphrase: Option<age::secrecy::SecretString>,
    source: PassphraseSource,
}

impl age::Callbacks for PromptCallbacks {
    fn display_message(&self, _: &str) {}

    fn confirm(&self, _: &str, _: &str, _: Option<&str>) -> Option<bool> {
        None
    }

    fn request_public_string(&self, _: &str) -> Option<String> {
        None
    }

    fn request_passphrase(&self, description: &str) -> Option<age::secrecy::SecretString> {
        self.passphrase.clone().or_else(|| {
            // A window must never end up here: the read is on `/dev/tty` and the
            // caller is on the UI thread.
            if self.source == PassphraseSource::SuppliedOnly {
                return None;
            }
            secret_prompt::prompt_for_passphrase(description)
                .ok()
                .map(|secret| age::secrecy::SecretString::from(secret.as_str().to_owned()))
        })
    }
}

enum PassphraseIdentityState {
    Encrypted {
        bytes: Vec<u8>,
        filename: String,
        passphrase: Option<age::secrecy::SecretString>,
        source: PassphraseSource,
    },
    Loaded(Vec<Box<dyn age::Identity + Send + Sync>>),
    Failed(age::DecryptError),
}

struct PassphraseIdentity {
    state: Mutex<PassphraseIdentityState>,
}

impl PassphraseIdentity {
    fn new(
        bytes: Vec<u8>,
        filename: String,
        passphrase: Option<age::secrecy::SecretString>,
        source: PassphraseSource,
    ) -> Self {
        Self {
            state: Mutex::new(PassphraseIdentityState::Encrypted {
                bytes,
                filename,
                passphrase,
                source,
            }),
        }
    }

    fn unwrap_with(
        &self,
        unwrap: impl FnOnce(
            &[Box<dyn age::Identity + Send + Sync>],
        ) -> Option<std::result::Result<FileKey, age::DecryptError>>,
    ) -> Option<std::result::Result<FileKey, age::DecryptError>> {
        let mut state = self.state.lock().ok()?;
        let transition = std::mem::replace(
            &mut *state,
            PassphraseIdentityState::Failed(age::DecryptError::KeyDecryptionFailed),
        );
        match transition {
            PassphraseIdentityState::Encrypted {
                bytes,
                filename,
                passphrase,
                source,
            } => match decrypt_passphrase_identity_file(
                &bytes,
                &filename,
                passphrase.as_ref(),
                source,
            ) {
                Ok(identities) => {
                    *state = PassphraseIdentityState::Loaded(identities);
                }
                Err(error) => {
                    *state = PassphraseIdentityState::Failed(error);
                }
            },
            other @ PassphraseIdentityState::Loaded(_) => *state = other,
            other @ PassphraseIdentityState::Failed(_) => *state = other,
        }
        match &*state {
            PassphraseIdentityState::Loaded(identities) => unwrap(identities),
            PassphraseIdentityState::Failed(error) => Some(Err(error.clone())),
            PassphraseIdentityState::Encrypted { .. } => unreachable!("identity was loaded"),
        }
    }
}

impl age::Identity for PassphraseIdentity {
    fn unwrap_stanza(
        &self,
        stanza: &Stanza,
    ) -> Option<std::result::Result<FileKey, age::DecryptError>> {
        self.unwrap_with(|identities| {
            identities
                .iter()
                .find_map(|identity| identity.unwrap_stanza(stanza))
        })
    }

    fn unwrap_stanzas(
        &self,
        stanzas: &[Stanza],
    ) -> Option<std::result::Result<FileKey, age::DecryptError>> {
        self.unwrap_with(|identities| {
            identities
                .iter()
                .find_map(|identity| identity.unwrap_stanzas(stanzas))
        })
    }
}

fn read_identity_file(path: &Path) -> Result<Vec<u8>> {
    let bytes =
        fs::read(path).with_context(|| format!("reading identity file {}", path.display()))?;
    anyhow::ensure!(
        bytes.len() <= 16 * 1024 * 1024,
        "identity file is too large"
    );
    Ok(bytes)
}

/// Reports whether an identity file will need a passphrase before it can
/// unwrap an age file. This only parses the public SSH envelope; it never
/// attempts to decrypt the private key.
pub fn identity_path_requires_passphrase(path: &Path) -> Result<bool> {
    let bytes = read_identity_file(path)?;
    if bytes.starts_with(b"age-encryption.org/v1")
        || bytes.starts_with(b"-----BEGIN AGE ENCRYPTED FILE-----")
    {
        return Ok(true);
    }
    if std::str::from_utf8(&bytes).is_ok_and(|text| text.contains("-----BEGIN")) {
        let identity = age::ssh::Identity::from_buffer(
            BufReader::new(Cursor::new(&bytes)),
            Some(path.display().to_string()),
        )
        .with_context(|| format!("parsing SSH identity file {}", path.display()))?;
        return Ok(matches!(identity, age::ssh::Identity::Encrypted(_)));
    }
    Ok(false)
}

fn load_identity_file(
    path: &Path,
    identities: &mut Vec<Box<dyn age::Identity + Send + Sync>>,
    passphrase: Option<age::secrecy::SecretString>,
    source: PassphraseSource,
) -> Result<()> {
    let bytes = read_identity_file(path)?;
    if bytes.starts_with(b"age-encryption.org/v1")
        || bytes.starts_with(b"-----BEGIN AGE ENCRYPTED FILE-----")
    {
        identities.push(Box::new(PassphraseIdentity::new(
            bytes,
            path.display().to_string(),
            passphrase,
            source,
        )));
        return Ok(());
    }
    identities.extend(parse_identity_file_bytes(&bytes, path, passphrase, source)?);
    Ok(())
}

fn parse_identity_file_bytes(
    bytes: &[u8],
    path: &Path,
    passphrase: Option<age::secrecy::SecretString>,
    source: PassphraseSource,
) -> Result<Vec<Box<dyn age::Identity + Send + Sync>>> {
    let mut identities = Vec::new();
    if std::str::from_utf8(bytes).is_ok_and(|text| text.contains("-----BEGIN")) {
        let ssh = age::ssh::Identity::from_buffer(
            BufReader::new(Cursor::new(bytes)),
            Some(path.display().to_string()),
        )
        .with_context(|| format!("parsing SSH identity file {}", path.display()))?;
        identities.push(
            Box::new(ssh.with_callbacks(PromptCallbacks { passphrase, source }))
                as Box<dyn age::Identity + Send + Sync>,
        );
        return Ok(identities);
    }
    let text = std::str::from_utf8(bytes).context("identity file was not UTF-8")?;
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Ok(identity) = MlKem768X25519Identity::from_str(line) {
            identities.push(Box::new(identity) as Box<dyn age::Identity + Send + Sync>);
            continue;
        }
        if let Ok(identity) = age::x25519::Identity::from_str(line) {
            identities.push(Box::new(identity) as Box<dyn age::Identity + Send + Sync>);
            continue;
        }
        anyhow::bail!("identity file {} contains invalid data", path.display());
    }
    anyhow::ensure!(
        !identities.is_empty(),
        "identity file {} contained no identities",
        path.display()
    );
    Ok(identities)
}

fn decrypt_passphrase_identity_file(
    bytes: &[u8],
    filename: &str,
    passphrase: Option<&age::secrecy::SecretString>,
    source: PassphraseSource,
) -> std::result::Result<Vec<Box<dyn age::Identity + Send + Sync>>, age::DecryptError> {
    let decryptor =
        age::Decryptor::new_buffered(age::armor::ArmoredReader::new(Cursor::new(bytes)))?;
    let passphrase = passphrase
        .cloned()
        .or_else(|| {
            // As in `PromptCallbacks`: a window has no terminal to ask at.
            if source == PassphraseSource::SuppliedOnly {
                return None;
            }
            secret_prompt::prompt_for_passphrase(&format!(
                "Enter passphrase for encrypted identity file {filename}:"
            ))
            .ok()
            .map(|secret| age::secrecy::SecretString::from(secret.as_str().to_owned()))
        })
        .ok_or(age::DecryptError::KeyDecryptionFailed)?;
    let identity = age::scrypt::Identity::new(passphrase);
    let mut reader = decryptor
        .decrypt(std::iter::once(&identity as &dyn age::Identity))
        .map_err(|error| match error {
            age::DecryptError::DecryptionFailed => age::DecryptError::KeyDecryptionFailed,
            error => error,
        })?;
    let mut plaintext = Zeroizing::new(Vec::new());
    reader
        .read_to_end(&mut plaintext)
        .map_err(age::DecryptError::Io)?;
    parse_identity_file_bytes(
        &plaintext,
        Path::new(filename),
        None,
        PassphraseSource::SuppliedOnly,
    )
    .map_err(|_| age::DecryptError::InvalidHeader)
}

#[cfg(test)]
#[path = "../tests/persistence/identity.rs"]
mod tests;
