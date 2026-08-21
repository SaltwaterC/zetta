//! Encrypted detached-session persistence.
//!
//! The daemon owns the recipient side of this module. Identities are a client
//! concern: a daemon can write encrypted records without ever receiving the
//! private keys that can read them back. The age files produced here are
//! ordinary age v1 files, so the store does not introduce a second encrypted
//! file format for metadata, snapshots, or scrollback.

use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    fmt, fs,
    io::{self, BufReader, Cursor, Read as _, Write as _},
    path::{Path, PathBuf},
    str::FromStr,
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use age_core::{
    format::{FILE_KEY_BYTES, FileKey, Stanza},
    secrecy::ExposeSecret,
};
use anyhow::{Context as _, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD_NO_PAD};
use hpke::{
    Deserializable as _, OpModeR, OpModeS, Serializable as _,
    aead::ChaCha20Poly1305,
    kdf::HkdfSha256,
    kem::{Kem as _, XWing},
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::{auth::SessionSecret, catalog, protocol::BackgroundSessionSummary, secret_prompt};

const PQ_RECIPIENT_HRP: &str = "age1pq";
const PQ_IDENTITY_HRP: &str = "AGE-SECRET-KEY-PQ-";
const PQ_STANZA: &str = "mlkem768x25519";
const PQ_LABEL: &[u8] = b"age-encryption.org/mlkem768x25519";
const PQ_LABEL_NAME: &str = "postquantum";
const PQ_ENCAPSULATED_KEY_BYTES: usize = 1120;
const PQ_PUBLIC_KEY_BYTES: usize = 1216;
const PQ_PRIVATE_KEY_BYTES: usize = 32;
const PQ_CIPHERTEXT_BYTES: usize = FILE_KEY_BYTES + 16;

const MAX_GITHUB_RESPONSE_BYTES: u64 = 1024 * 1024;
const GITHUB_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RECORDS: usize = 64;
const MAX_STORAGE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_RECORD_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const SEGMENT_BYTES: usize = 8 * 1024 * 1024;
const SEGMENT_INTERVAL: Duration = Duration::from_secs(5 * 60);
const MANIFEST_VERSION: u32 = 1;

/// The disk settings passed from Zetta to a daemon.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PersistenceOptions {
    /// Age recipients and `github:USER` convenience entries.
    pub recipients: Vec<String>,
    /// The client-side default identity used by resume. The daemon never
    /// receives this path as part of its startup options.
    pub identity: Option<PathBuf>,
}

/// The private file handed to a daemon at startup. It contains only resolved
/// public recipients; identities and `github:` aliases never cross the client
/// boundary.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DaemonOptionsFile {
    pub recipients: Vec<String>,
}

/// An opaque record that is safe to show before an identity has been supplied.
pub use crate::protocol::RestorableSessionRecord as RestorableRecord;

/// The decrypted session state used by the client and daemon protocol.
///
/// The on-disk representation is [`PersistedSessionMetadata`]; snapshot bytes
/// are kept in the separate encrypted byte stream and are joined back onto
/// this type only after the client has decrypted both files.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistedSession {
    pub id: u64,
    pub created_at: u64,
    pub updated_at: u64,
    pub summary: BackgroundSessionSummary,
    pub state: serde_json::Value,
    pub verifier: Option<String>,
    pub failed_authentications: u32,
    pub backoff_seconds: u64,
    pub snapshots: Vec<PersistedSnapshot>,
}

/// A snapshot kept inside the encrypted session record.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistedSnapshot {
    pub pane_id: u64,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedSessionMetadata {
    id: u64,
    created_at: u64,
    updated_at: u64,
    summary: BackgroundSessionSummary,
    state: serde_json::Value,
    verifier: Option<String>,
    failed_authentications: u32,
    backoff_seconds: u64,
    snapshots: Vec<SnapshotLocation>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotLocation {
    pane_id: u64,
    segment: u64,
    offset: u64,
    length: u64,
}

fn snapshot_path(directory: &Path, session_id: u64, sequence: u64) -> PathBuf {
    directory.join(format!("session-{session_id}-bytes-segment-{sequence}.age"))
}

/// A parsed collection of age recipients.
pub struct RecipientSet {
    recipients: Vec<Box<dyn age::Recipient + Send + Sync>>,
    post_quantum: bool,
}

impl fmt::Debug for RecipientSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RecipientSet")
            .field("count", &self.recipients.len())
            .field("post_quantum", &self.post_quantum)
            .finish()
    }
}

impl RecipientSet {
    /// Parses configured recipients without doing network I/O.
    pub fn parse(values: &[String]) -> Result<Self> {
        let mut recipients = Vec::new();
        let mut post_quantum = None;
        for value in values {
            let value = value.trim();
            anyhow::ensure!(!value.is_empty(), "age recipient entries must not be empty");
            anyhow::ensure!(
                !value.starts_with("github:"),
                "github recipients must be resolved before parsing the daemon options"
            );
            let is_pq = value.starts_with("age1pq1");
            match post_quantum {
                Some(previous) if previous != is_pq => anyhow::bail!(
                    "post-quantum age recipients cannot be mixed with classical or SSH recipients"
                ),
                None => post_quantum = Some(is_pq),
                _ => {}
            }
            let recipient = parse_recipient(value)?;
            recipients.push(recipient);
        }
        Ok(Self {
            recipients,
            post_quantum: post_quantum.unwrap_or(false),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.recipients.is_empty()
    }

    pub fn is_post_quantum(&self) -> bool {
        self.post_quantum
    }

    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        anyhow::ensure!(
            !self.recipients.is_empty(),
            "no persistence recipients configured"
        );
        // age v1's Encryptor provides both the recipient envelope and the
        // standard ChaCha20-Poly1305 STREAM payload construction.
        let encryptor = age::Encryptor::with_recipients(
            self.recipients
                .iter()
                .map(|recipient| recipient.as_ref() as &dyn age::Recipient),
        )
        .context("creating age encryptor")?;
        let mut ciphertext = Vec::with_capacity(plaintext.len() + 4096);
        let mut writer = encryptor
            .wrap_output(&mut ciphertext)
            .context("creating age output stream")?;
        writer
            .write_all(plaintext)
            .context("writing age plaintext")?;
        writer.finish().context("finishing age output stream")?;
        Ok(ciphertext)
    }
}

/// A set of client-side age identities.
pub struct IdentitySet {
    identities: Vec<Box<dyn age::Identity + Send + Sync>>,
}

impl fmt::Debug for IdentitySet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IdentitySet")
            .field("count", &self.identities.len())
            .finish()
    }
}

impl IdentitySet {
    /// Loads native age, PQ, and SSH identity files. Files may be repeated by
    /// callers in the same way `age -i` is repeatable.
    pub fn from_paths(paths: &[PathBuf]) -> Result<Self> {
        Self::from_paths_with_passphrases(paths, &[])
    }

    /// Loads identity files with positional passphrases collected by a caller.
    /// A missing passphrase keeps the normal terminal prompt for standalone
    /// clients; GUI callers can therefore provide passphrases without making
    /// the daemon or the GUI depend on a controlling terminal.
    pub fn from_paths_with_passphrases(
        paths: &[PathBuf],
        passphrases: &[Option<SessionSecret>],
    ) -> Result<Self> {
        let mut identities = Vec::new();
        for (index, path) in paths.iter().enumerate() {
            let passphrase = passphrases
                .get(index)
                .and_then(Option::as_ref)
                .map(|passphrase| age::secrecy::SecretString::from(passphrase.expose()));
            load_identity_file(path, &mut identities, passphrase)?;
        }
        anyhow::ensure!(!identities.is_empty(), "no identities were found");
        Ok(Self { identities })
    }

    fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>> {
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
}

/// Opens encrypted session metadata and snapshot bytes without opening the
/// daemon's recipient side. This is the client-side half of resume.
pub fn load_session_from_directory(
    base: &Path,
    id: u64,
    identities: &IdentitySet,
) -> Result<PersistedSession> {
    load_session_from_persistence_directory(&base.join("persistence"), id, identities)
}

fn load_session_from_persistence_directory(
    directory: &Path,
    id: u64,
    identities: &IdentitySet,
) -> Result<PersistedSession> {
    let path = directory.join(format!("session-{id}.age"));
    let ciphertext = fs::read(&path)
        .with_context(|| format!("reading encrypted session record {}", path.display()))?;
    let plaintext = identities.decrypt(&ciphertext)?;
    let metadata: PersistedSessionMetadata =
        serde_json::from_slice(&plaintext).context("parsing encrypted session metadata")?;
    let snapshots = load_snapshots(directory, id, &metadata.snapshots, identities)?;
    Ok(PersistedSession {
        id: metadata.id,
        created_at: metadata.created_at,
        updated_at: metadata.updated_at,
        summary: metadata.summary,
        state: metadata.state,
        verifier: metadata.verifier,
        failed_authentications: metadata.failed_authentications,
        backoff_seconds: metadata.backoff_seconds,
        snapshots,
    })
}

fn load_snapshots(
    directory: &Path,
    session_id: u64,
    locations: &[SnapshotLocation],
    identities: &IdentitySet,
) -> Result<Vec<PersistedSnapshot>> {
    let mut segments = HashMap::new();
    locations
        .iter()
        .map(|location| {
            if let Entry::Vacant(entry) = segments.entry(location.segment) {
                let path = snapshot_path(directory, session_id, location.segment);
                let ciphertext = fs::read(&path).with_context(|| {
                    format!("reading encrypted session bytes {}", path.display())
                })?;
                entry.insert(identities.decrypt(&ciphertext)?);
            }
            let segment = segments
                .get(&location.segment)
                .expect("snapshot segment was inserted");
            let offset =
                usize::try_from(location.offset).context("snapshot offset is too large")?;
            let length =
                usize::try_from(location.length).context("snapshot length is too large")?;
            let end = offset
                .checked_add(length)
                .context("snapshot range overflowed")?;
            anyhow::ensure!(
                end <= segment.len(),
                "snapshot range exceeds encrypted session byte stream"
            );
            Ok(PersistedSnapshot {
                pane_id: location.pane_id,
                bytes: segment[offset..end].to_vec(),
            })
        })
        .collect()
}

/// Parses one configured recipient. GitHub entries intentionally do not reach
/// this function: resolving them is an explicit startup operation.
pub fn parse_recipient(value: &str) -> Result<Box<dyn age::Recipient + Send + Sync>> {
    if value.starts_with("age1pq1") {
        return Ok(Box::new(MlKem768X25519Recipient::from_str(value)?));
    }
    if value.starts_with("age1") {
        return Ok(Box::new(
            age::x25519::Recipient::from_str(value)
                .map_err(|_| anyhow::anyhow!("invalid age X25519 recipient"))?,
        ));
    }
    if value.starts_with("ssh-") {
        return Ok(Box::new(age::ssh::Recipient::from_str(value).map_err(
            |error| anyhow::anyhow!("invalid SSH recipient: {error:?}"),
        )?));
    }
    anyhow::bail!("invalid age recipient")
}

/// Resolves `github:USER` entries and then applies the same parser and PQ
/// mixing rule as direct recipients. Configuration parsing never calls this.
pub fn resolve_recipients(values: &[String]) -> Result<RecipientSet> {
    let mut resolved = Vec::new();
    for value in values {
        let value = value.trim();
        if let Some(username) = value.strip_prefix("github:") {
            resolved.extend(fetch_github_recipients(username)?);
        } else {
            resolved.push(value.to_owned());
        }
    }
    RecipientSet::parse(&resolved)
}

/// Resolves aliases and validates the complete recipient set, returning only
/// the public strings the daemon needs to encrypt future records.
pub fn resolve_recipient_strings(values: &[String]) -> Result<Vec<String>> {
    let mut resolved = Vec::new();
    for value in values {
        let value = value.trim();
        if let Some(username) = value.strip_prefix("github:") {
            resolved.extend(fetch_github_recipients(username)?);
        } else {
            resolved.push(value.to_owned());
        }
    }
    RecipientSet::parse(&resolved)?;
    Ok(resolved)
}

fn fetch_github_recipients(username: &str) -> Result<Vec<String>> {
    validate_github_username(username)?;
    let url = format!("https://github.com/{username}.keys");
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(GITHUB_TIMEOUT)
        .timeout(GITHUB_TIMEOUT)
        .user_agent("zetta-zmux")
        .build()
        .context("creating GitHub key client")?;
    let response = client.get(url).send().context("fetching GitHub SSH keys")?;
    anyhow::ensure!(
        response.status().is_success(),
        "GitHub did not return SSH keys"
    );
    if response
        .content_length()
        .is_some_and(|length| length > MAX_GITHUB_RESPONSE_BYTES)
    {
        anyhow::bail!("GitHub SSH key response is too large");
    }
    let mut body = Vec::new();
    response
        .take(MAX_GITHUB_RESPONSE_BYTES + 1)
        .read_to_end(&mut body)
        .context("reading GitHub SSH keys")?;
    anyhow::ensure!(
        body.len() as u64 <= MAX_GITHUB_RESPONSE_BYTES,
        "GitHub SSH key response is too large"
    );
    parse_github_keys(&body)
}

fn validate_github_username(username: &str) -> Result<()> {
    anyhow::ensure!(
        (1..=39).contains(&username.len())
            && username
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && !username.starts_with('-')
            && !username.ends_with('-')
            && !username.contains("--"),
        "invalid GitHub username"
    );
    Ok(())
}

fn parse_github_keys(body: &[u8]) -> Result<Vec<String>> {
    let text = std::str::from_utf8(body).context("GitHub SSH key response was not UTF-8")?;
    let mut keys = Vec::new();
    for line in text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let mut fields = line.split_whitespace();
        let key_type = fields.next().context("malformed GitHub SSH key")?;
        let encoded = fields.next().context("malformed GitHub SSH key")?;
        let valid_blob = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .is_ok();
        if !valid_blob {
            anyhow::bail!("malformed GitHub SSH key");
        }
        let canonical = format!("{key_type} {encoded}");
        match key_type {
            "ssh-ed25519" | "ssh-rsa" => {
                parse_recipient(&canonical)?;
                keys.push(canonical);
            }
            _ => match age::ssh::Recipient::from_str(&canonical) {
                Ok(_) => unreachable!("age returned a supported SSH key type as unsupported"),
                Err(
                    age::ssh::ParseRecipientKeyError::Unsupported(_)
                    | age::ssh::ParseRecipientKeyError::Ignore,
                ) => {
                    // Valid but unsupported SSH key types are ignored, matching
                    // age's recipient-file behavior.
                }
                Err(error) => anyhow::bail!("malformed GitHub SSH key: {error:?}"),
            },
        }
    }
    anyhow::ensure!(!keys.is_empty(), "GitHub account has no supported SSH keys");
    Ok(keys)
}

/// The ML-KEM-768/X25519 age recipient used by age 1.3.1.
#[derive(Clone, PartialEq, Eq)]
pub struct MlKem768X25519Recipient {
    key: <XWing as hpke::kem::Kem>::PublicKey,
}

impl fmt::Debug for MlKem768X25519Recipient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_public(formatter)
    }
}

impl fmt::Display for MlKem768X25519Recipient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_public(formatter)
    }
}

impl MlKem768X25519Recipient {
    fn fmt_public(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}",
            bech32_encode(PQ_RECIPIENT_HRP, &self.key.to_bytes(), false)
        )
    }
}

impl FromStr for MlKem768X25519Recipient {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let bytes = bech32_decode(value, PQ_RECIPIENT_HRP, false)?;
        anyhow::ensure!(
            bytes.len() == PQ_PUBLIC_KEY_BYTES,
            "invalid ML-KEM-768/X25519 recipient length"
        );
        let key = <XWing as hpke::kem::Kem>::PublicKey::from_bytes(&bytes)
            .map_err(|_| anyhow::anyhow!("invalid ML-KEM-768/X25519 recipient key"))?;
        Ok(Self { key })
    }
}

impl age::Recipient for MlKem768X25519Recipient {
    fn wrap_file_key(
        &self,
        file_key: &FileKey,
    ) -> std::result::Result<(Vec<Stanza>, HashSet<String>), age::EncryptError> {
        let (enc, mut context) = hpke::setup_sender::<ChaCha20Poly1305, HkdfSha256, XWing>(
            &OpModeS::Base,
            &self.key,
            PQ_LABEL,
        )
        .map_err(|error| age::EncryptError::Io(io::Error::other(error.to_string())))?;
        let body = context
            .seal(file_key.expose_secret(), &[])
            .map_err(|error| age::EncryptError::Io(io::Error::other(error.to_string())))?;
        Ok((
            vec![Stanza {
                tag: PQ_STANZA.to_owned(),
                args: vec![STANDARD_NO_PAD.encode(enc.to_bytes())],
                body,
            }],
            [PQ_LABEL_NAME.to_owned()].into_iter().collect(),
        ))
    }
}

/// The ML-KEM-768/X25519 age identity used by age 1.3.1.
#[derive(Clone)]
pub struct MlKem768X25519Identity {
    key: <XWing as hpke::kem::Kem>::PrivateKey,
}

impl fmt::Debug for MlKem768X25519Identity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MlKem768X25519Identity(REDACTED)")
    }
}

impl MlKem768X25519Identity {
    pub fn generate() -> Self {
        let (key, _) = XWing::gen_keypair();
        Self { key }
    }

    pub fn to_recipient(&self) -> MlKem768X25519Recipient {
        MlKem768X25519Recipient {
            key: XWing::sk_to_pk(&self.key),
        }
    }

    fn encoded(&self) -> String {
        bech32_encode(PQ_IDENTITY_HRP, &self.key.to_bytes(), true)
    }
}

impl fmt::Display for MlKem768X25519Identity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.encoded())
    }
}

impl FromStr for MlKem768X25519Identity {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let bytes = bech32_decode(value, PQ_IDENTITY_HRP, true)?;
        anyhow::ensure!(
            bytes.len() == PQ_PRIVATE_KEY_BYTES,
            "invalid ML-KEM-768/X25519 identity length"
        );
        let key = <XWing as hpke::kem::Kem>::PrivateKey::from_bytes(&bytes)
            .map_err(|_| anyhow::anyhow!("invalid ML-KEM-768/X25519 identity key"))?;
        Ok(Self { key })
    }
}

impl age::Identity for MlKem768X25519Identity {
    fn unwrap_stanza(
        &self,
        stanza: &Stanza,
    ) -> Option<std::result::Result<FileKey, age::DecryptError>> {
        if stanza.tag != PQ_STANZA {
            return None;
        }
        let encoded = match stanza.args.as_slice() {
            [encoded] => encoded,
            _ => return Some(Err(age::DecryptError::InvalidHeader)),
        };
        let encoded = match STANDARD_NO_PAD.decode(encoded) {
            Ok(encoded) if encoded.len() == PQ_ENCAPSULATED_KEY_BYTES => encoded,
            _ => return Some(Err(age::DecryptError::InvalidHeader)),
        };
        if stanza.body.len() != PQ_CIPHERTEXT_BYTES {
            return Some(Err(age::DecryptError::InvalidHeader));
        }
        let enc = match <XWing as hpke::kem::Kem>::EncappedKey::from_bytes(&encoded) {
            Ok(enc) => enc,
            Err(_) => return Some(Err(age::DecryptError::InvalidHeader)),
        };
        let mut context = match hpke::setup_receiver::<ChaCha20Poly1305, HkdfSha256, XWing>(
            &OpModeR::Base,
            &self.key,
            &enc,
            PQ_LABEL,
        ) {
            Ok(context) => context,
            Err(_) => return Some(Err(age::DecryptError::InvalidHeader)),
        };
        match context.open(&stanza.body, &[]) {
            Ok(plaintext) if plaintext.len() == FILE_KEY_BYTES => {
                let mut plaintext = plaintext;
                Some(Ok(FileKey::init_with_mut(|file_key| {
                    file_key.copy_from_slice(&plaintext);
                    plaintext.fill(0);
                })))
            }
            // X-Wing is anonymous. A failed open means this stanza was not
            // addressed to this identity, so age should try the next one.
            _ => None,
        }
    }
}

fn bech32_encode(hrp: &str, bytes: &[u8], uppercase: bool) -> String {
    const CHARSET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";
    let data = convert_bits(bytes, 8, 5, true).expect("valid 8-to-5 conversion");
    let mut values = data.clone();
    values.extend([0; 6]);
    let checksum_hrp = hrp.to_ascii_lowercase();
    let checksum = bech32_polymod(&bech32_hrp_expand(&checksum_hrp), &values) ^ 1;
    let mut output = String::with_capacity(hrp.len() + 1 + data.len() + 6);
    output.push_str(hrp);
    output.push('1');
    for value in data
        .into_iter()
        .chain((0..6).map(|index| ((checksum >> (5 * (5 - index))) & 31) as u8))
    {
        output.push(CHARSET[value as usize] as char);
    }
    if uppercase {
        output.to_ascii_uppercase()
    } else {
        output
    }
}

fn bech32_decode(value: &str, expected_hrp: &str, uppercase: bool) -> Result<Vec<u8>> {
    const CHARSET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";
    const MAX_BECH32_LENGTH: usize = 4096;
    anyhow::ensure!(!value.is_empty(), "invalid Bech32 encoding");
    anyhow::ensure!(
        value.len() <= MAX_BECH32_LENGTH,
        "Bech32 encoding is too long"
    );
    anyhow::ensure!(value.is_ascii(), "invalid Bech32 encoding");
    anyhow::ensure!(
        !(value
            .chars()
            .any(|character| character.is_ascii_lowercase())
            && value
                .chars()
                .any(|character| character.is_ascii_uppercase())),
        "mixed-case Bech32 encoding"
    );
    anyhow::ensure!(
        value
            .chars()
            .all(|character| !character.is_ascii() || character.is_ascii_graphic()),
        "invalid Bech32 encoding"
    );
    anyhow::ensure!(
        (uppercase && value == value.to_ascii_uppercase())
            || (!uppercase && value == value.to_ascii_lowercase()),
        "non-canonical Bech32 case"
    );
    let separator = value.rfind('1').context("invalid Bech32 encoding")?;
    let (hrp, encoded) = value.split_at(separator);
    anyhow::ensure!(hrp == expected_hrp, "incorrect Bech32 HRP");
    let encoded = &encoded[1..];
    anyhow::ensure!(encoded.len() >= 6, "invalid Bech32 checksum");
    let values = encoded
        .bytes()
        .map(|byte| {
            CHARSET
                .iter()
                .position(|candidate| *candidate == byte.to_ascii_lowercase())
                .map(|value| value as u8)
                .context("invalid Bech32 character")
        })
        .collect::<Result<Vec<_>>>()?;
    let checksum_hrp = hrp.to_ascii_lowercase();
    anyhow::ensure!(
        bech32_polymod(&bech32_hrp_expand(&checksum_hrp), &values) == 1,
        "invalid Bech32 checksum"
    );
    convert_bits(&values[..values.len() - 6], 5, 8, false)
}

fn convert_bits(data: &[u8], from: u32, to: u32, pad: bool) -> Result<Vec<u8>> {
    let mut accumulator = 0u32;
    let mut bits = 0u32;
    let max_value = (1u32 << to) - 1;
    let max_input = (1u32 << from) - 1;
    let max_accumulator = (1u32 << (from + to - 1)) - 1;
    let mut result = Vec::new();
    for &value in data {
        anyhow::ensure!(u32::from(value) <= max_input, "invalid Bech32 data");
        accumulator = ((accumulator << from) | u32::from(value)) & max_accumulator;
        bits += from;
        while bits >= to {
            bits -= to;
            result.push(((accumulator >> bits) & max_value) as u8);
        }
    }
    if pad {
        if bits > 0 {
            result.push(((accumulator << (to - bits)) & max_value) as u8);
        }
    } else {
        anyhow::ensure!(bits < from && ((accumulator << (to - bits)) & max_value) == 0);
    }
    Ok(result)
}

fn bech32_hrp_expand(hrp: &str) -> Vec<u8> {
    hrp.bytes()
        .map(|byte| byte >> 5)
        .chain(std::iter::once(0))
        .chain(hrp.bytes().map(|byte| byte & 31))
        .collect()
}

fn bech32_polymod(hrp: &[u8], values: &[u8]) -> u32 {
    const GENERATORS: [u32; 5] = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3];
    hrp.iter().chain(values).fold(1u32, |checksum, value| {
        let top = checksum >> 25;
        let mut checksum = (checksum & 0x1ffffff) << 5 ^ u32::from(*value);
        for (index, generator) in GENERATORS.iter().enumerate() {
            if (top >> index) & 1 == 1 {
                checksum ^= generator;
            }
        }
        checksum
    })
}

#[derive(Clone)]
struct PromptCallbacks {
    passphrase: Option<age::secrecy::SecretString>,
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
    ) -> Self {
        Self {
            state: Mutex::new(PassphraseIdentityState::Encrypted {
                bytes,
                filename,
                passphrase,
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
            } => match decrypt_passphrase_identity_file(&bytes, &filename, passphrase.as_ref()) {
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
) -> Result<()> {
    let bytes = read_identity_file(path)?;
    if bytes.starts_with(b"age-encryption.org/v1")
        || bytes.starts_with(b"-----BEGIN AGE ENCRYPTED FILE-----")
    {
        identities.push(Box::new(PassphraseIdentity::new(
            bytes,
            path.display().to_string(),
            passphrase,
        )));
        return Ok(());
    }
    identities.extend(parse_identity_file_bytes(&bytes, path, passphrase)?);
    Ok(())
}

fn parse_identity_file_bytes(
    bytes: &[u8],
    path: &Path,
    passphrase: Option<age::secrecy::SecretString>,
) -> Result<Vec<Box<dyn age::Identity + Send + Sync>>> {
    let mut identities = Vec::new();
    if std::str::from_utf8(bytes).is_ok_and(|text| text.contains("-----BEGIN")) {
        let ssh = age::ssh::Identity::from_buffer(
            BufReader::new(Cursor::new(bytes)),
            Some(path.display().to_string()),
        )
        .with_context(|| format!("parsing SSH identity file {}", path.display()))?;
        identities.push(Box::new(ssh.with_callbacks(PromptCallbacks { passphrase }))
            as Box<dyn age::Identity + Send + Sync>);
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
) -> std::result::Result<Vec<Box<dyn age::Identity + Send + Sync>>, age::DecryptError> {
    let decryptor =
        age::Decryptor::new_buffered(age::armor::ArmoredReader::new(Cursor::new(bytes)))?;
    let passphrase = passphrase
        .cloned()
        .or_else(|| {
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
    parse_identity_file_bytes(&plaintext, Path::new(filename), None)
        .map_err(|_| age::DecryptError::InvalidHeader)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    boot_stamp: String,
    records: Vec<RestorableRecord>,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            version: MANIFEST_VERSION,
            boot_stamp: boot_stamp(),
            records: Vec::new(),
        }
    }
}

struct SegmentBuffer {
    bytes: Vec<u8>,
    started_at: u64,
    sequence: u64,
}

/// The encrypted store below the private session directory.
pub struct PersistenceStore {
    directory: PathBuf,
    manifest_path: PathBuf,
    recipients: RecipientSet,
    manifest: Manifest,
    segments: HashMap<(u64, u64), SegmentBuffer>,
}

impl fmt::Debug for PersistenceStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PersistenceStore")
            .field("directory", &self.directory)
            .field("records", &self.manifest.records.len())
            .finish()
    }
}

impl PersistenceStore {
    /// Opens the store. With no recipients, returns `None` and does not create
    /// a persistence directory or manifest on a fresh base. An existing store
    /// is reopened from its saved public recipients for compatibility with
    /// callers that are explicitly recovering a daemon.
    pub fn open(base: &Path, values: &[String]) -> Result<Option<Self>> {
        Self::open_with_recovery(base, (!values.is_empty()).then_some(values))
    }

    /// Opens a store with an optional startup configuration. `Some(&[])` is an
    /// explicit "disk selected without recipients" choice and therefore does
    /// not reuse recipients saved by an earlier daemon. `None` is reserved for
    /// recovery paths such as `zmux resume`, where the daemon has no client
    /// configuration but may need the private saved recipient set.
    pub fn open_with_recovery(base: &Path, values: Option<&[String]>) -> Result<Option<Self>> {
        Self::open_with_recovery_state(base, values, false)
    }

    /// As [`Self::open_with_recovery`], preserving records as live during an
    /// in-process daemon upgrade. A normal daemon start is evidence that the
    /// previous daemon was lost, even when the operating-system boot stamp is
    /// unchanged.
    pub fn open_with_recovery_state(
        base: &Path,
        values: Option<&[String]>,
        replacing_daemon: bool,
    ) -> Result<Option<Self>> {
        let directory = base.join("persistence");
        if values.is_some_and(|values| values.is_empty()) {
            return Ok(None);
        }
        if values.is_none() && !directory.is_dir() {
            return Ok(None);
        }
        catalog::create_private_dir(&directory)?;
        let resolved_values = match values {
            None => read_saved_recipients(&directory)?.unwrap_or_default(),
            Some(values) => {
                let resolved = resolve_recipient_strings(values)?;
                write_saved_recipients(&directory, &resolved)?;
                resolved
            }
        };
        let recipients = RecipientSet::parse(&resolved_values)?;
        let manifest_path = directory.join("manifest.json");
        let mut manifest = match fs::read(&manifest_path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Manifest::default(),
            Err(error) => return Err(error).context("reading persistence manifest"),
        };
        if manifest.version != MANIFEST_VERSION {
            manifest = Manifest::default();
        }
        let current_boot = boot_stamp();
        let recovered = !replacing_daemon || manifest.boot_stamp != current_boot;
        if recovered {
            for record in &mut manifest.records {
                record.restorable = true;
            }
            manifest.boot_stamp = current_boot;
        }
        let mut store = Self {
            directory,
            manifest_path,
            recipients,
            manifest,
            segments: HashMap::new(),
        };
        if recovered {
            store.prune(&HashSet::new())?;
        }
        store.write_manifest()?;
        Ok(Some(store))
    }

    pub fn records(&self) -> &[RestorableRecord] {
        &self.manifest.records
    }

    pub fn save_session(&mut self, session: &PersistedSession) -> Result<()> {
        self.write_session(session, false)
    }

    /// Rewrites a still-restorable record after a failed resume attempt. It
    /// keeps the record available to the next client instead of treating the
    /// update like a live daemon detach.
    pub fn update_session(&mut self, session: &PersistedSession) -> Result<()> {
        self.write_session(session, true)
    }

    fn write_session(&mut self, session: &PersistedSession, restorable: bool) -> Result<()> {
        let (snapshots, snapshot_bytes) =
            self.write_snapshot_stream(session.id, &session.snapshots)?;
        let metadata = PersistedSessionMetadata {
            id: session.id,
            created_at: session.created_at,
            updated_at: session.updated_at,
            summary: session.summary.clone(),
            state: session.state.clone(),
            verifier: session.verifier.clone(),
            failed_authentications: session.failed_authentications,
            backoff_seconds: session.backoff_seconds,
            snapshots,
        };
        let plaintext =
            serde_json::to_vec(&metadata).context("serializing persisted session metadata")?;
        let ciphertext = self.recipients.encrypt(&plaintext)?;
        atomic_write(&self.session_path(session.id), &ciphertext)?;
        self.remove_unreferenced_snapshot_segments(session.id, &metadata.snapshots)?;
        let now = unix_now();
        let created_at = self
            .manifest
            .records
            .iter()
            .find(|record| record.id == session.id)
            .map_or(session.created_at, |record| record.created_at);
        let protected = session.verifier.is_some() || session.summary.authentication_required;
        if let Some(record) = self
            .manifest
            .records
            .iter_mut()
            .find(|record| record.id == session.id)
        {
            record.updated_at = now.max(session.updated_at);
            record.metadata_bytes = ciphertext.len() as u64;
            record.snapshot_bytes = snapshot_bytes;
            record.protected = protected;
            record.restorable = restorable;
        } else {
            self.manifest.records.push(RestorableRecord {
                id: session.id,
                created_at,
                updated_at: now.max(session.updated_at),
                metadata_bytes: ciphertext.len() as u64,
                snapshot_bytes,
                scrollback_bytes: 0,
                protected,
                restorable,
            });
        }
        self.prune(&HashSet::new())?;
        self.write_manifest()
    }

    pub fn load_session(&self, id: u64, identities: &IdentitySet) -> Result<PersistedSession> {
        load_session_from_persistence_directory(&self.directory, id, identities)
    }

    pub fn append_scrollback(&mut self, session_id: u64, pane_id: u64, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let now = unix_now();
        let sequence = self.next_sequence(session_id, pane_id);
        let segment = self
            .segments
            .entry((session_id, pane_id))
            .or_insert_with(|| SegmentBuffer {
                bytes: Vec::new(),
                started_at: now,
                sequence,
            });
        segment.bytes.extend_from_slice(bytes);
        if let Some(record) = self
            .manifest
            .records
            .iter_mut()
            .find(|record| record.id == session_id)
        {
            record.scrollback_bytes = record.scrollback_bytes.saturating_add(bytes.len() as u64);
        }
        let segment_to_flush = (segment.bytes.len() >= SEGMENT_BYTES
            || now.saturating_sub(segment.started_at) >= SEGMENT_INTERVAL.as_secs())
        .then(|| {
            self.segments
                .remove(&(session_id, pane_id))
                .expect("segment exists")
        });
        if let Some(mut segment) = segment_to_flush {
            self.flush_segment(session_id, pane_id, &mut segment)?;
        }
        Ok(())
    }

    pub fn flush_segments(&mut self) -> Result<()> {
        let segments = std::mem::take(&mut self.segments);
        for ((session_id, pane_id), mut segment) in segments {
            self.flush_segment(session_id, pane_id, &mut segment)?;
        }
        Ok(())
    }

    pub fn read_scrollback(&self, id: u64, identities: &IdentitySet) -> Result<Vec<u8>> {
        let prefix = format!("session-{id}-pane-");
        let mut paths = fs::read_dir(&self.directory)
            .with_context(|| format!("reading persistence directory {}", self.directory.display()))?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter_map(|path| {
                let name = path.file_name()?.to_str()?;
                let (pane_id, sequence) = name
                    .strip_prefix(&prefix)?
                    .strip_suffix(".age")?
                    .split_once("-segment-")?;
                Some((
                    pane_id.parse::<u64>().ok()?,
                    sequence.parse::<u64>().ok()?,
                    path,
                ))
            })
            .collect::<Vec<_>>();
        paths.sort_by_key(|(pane_id, sequence, _)| (*pane_id, *sequence));
        let mut output = Vec::new();
        for (_, _, path) in paths {
            let ciphertext = fs::read(path)?;
            output.extend_from_slice(&identities.decrypt(&ciphertext)?);
        }
        Ok(output)
    }

    /// Appends the current pane snapshots to the session's logical byte
    /// stream. Each physical segment is a complete age v1 stream: age files
    /// must be finalized before they can be reopened, so rotation is what
    /// makes the logical stream append-only without inventing a second age
    /// format.
    fn write_snapshot_stream(
        &mut self,
        session_id: u64,
        snapshots: &[PersistedSnapshot],
    ) -> Result<(Vec<SnapshotLocation>, u64)> {
        if snapshots.is_empty() {
            return Ok((Vec::new(), 0));
        }
        let mut sequence = self.next_snapshot_sequence(session_id);
        let mut segment = Vec::new();
        let mut locations = Vec::with_capacity(snapshots.len());
        let mut encrypted_bytes = 0;
        for snapshot in snapshots {
            anyhow::ensure!(
                snapshot.bytes.len() <= SEGMENT_BYTES,
                "snapshot is too large for an encrypted session byte stream segment"
            );
            if !segment.is_empty() && segment.len() + snapshot.bytes.len() > SEGMENT_BYTES {
                encrypted_bytes += self.flush_snapshot_segment(session_id, sequence, &segment)?;
                sequence = sequence.saturating_add(1);
                segment.clear();
            }
            let offset = segment.len() as u64;
            segment.extend_from_slice(&snapshot.bytes);
            locations.push(SnapshotLocation {
                pane_id: snapshot.pane_id,
                segment: sequence,
                offset,
                length: snapshot.bytes.len() as u64,
            });
        }
        encrypted_bytes += self.flush_snapshot_segment(session_id, sequence, &segment)?;
        Ok((locations, encrypted_bytes))
    }

    fn flush_snapshot_segment(&self, session_id: u64, sequence: u64, bytes: &[u8]) -> Result<u64> {
        let ciphertext = self.recipients.encrypt(bytes)?;
        let path = snapshot_path(&self.directory, session_id, sequence);
        atomic_write(&path, &ciphertext)?;
        Ok(ciphertext.len() as u64)
    }

    fn remove_unreferenced_snapshot_segments(
        &self,
        session_id: u64,
        locations: &[SnapshotLocation],
    ) -> Result<()> {
        let keep = locations
            .iter()
            .map(|location| location.segment)
            .collect::<HashSet<_>>();
        let prefix = format!("session-{session_id}-bytes-segment-");
        for entry in fs::read_dir(&self.directory)? {
            let path = entry?.path();
            let Some(sequence) = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix(&prefix))
                .and_then(|name| name.strip_suffix(".age"))
                .and_then(|sequence| sequence.parse::<u64>().ok())
            else {
                continue;
            };
            if !keep.contains(&sequence) {
                let _ = fs::remove_file(path);
            }
        }
        Ok(())
    }

    pub fn forget(&mut self, id: u64) -> Result<()> {
        self.segments.retain(|(session_id, _), _| *session_id != id);
        let _ = fs::remove_file(self.session_path(id));
        self.remove_unreferenced_snapshot_segments(id, &[])?;
        let prefix = format!("session-{id}-pane-");
        for entry in fs::read_dir(&self.directory)? {
            let path = entry?.path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix))
            {
                let _ = fs::remove_file(path);
            }
        }
        self.manifest.records.retain(|record| record.id != id);
        self.write_manifest()
    }

    /// Applies the fixed cleanup bounds. IDs in `live` are never pruned by
    /// this pass, even when they are older than the retention window.
    pub fn prune(&mut self, live: &HashSet<u64>) -> Result<()> {
        let now = unix_now();
        let mut candidates = self
            .manifest
            .records
            .iter()
            .filter(|record| record.restorable && !live.contains(&record.id))
            .cloned()
            .collect::<Vec<_>>();
        candidates.sort_by_key(|record| record.updated_at);
        let mut remove = HashSet::new();
        let mut total = self.storage_bytes()?;
        for record in &candidates {
            if now.saturating_sub(record.updated_at) > MAX_RECORD_AGE.as_secs()
                || self.manifest.records.len().saturating_sub(remove.len()) > MAX_RECORDS
                || total > MAX_STORAGE_BYTES
            {
                remove.insert(record.id);
                total = total.saturating_sub(
                    record.metadata_bytes + record.snapshot_bytes + record.scrollback_bytes,
                );
            }
        }
        if remove.is_empty() {
            return Ok(());
        }
        self.segments
            .retain(|(session_id, _), _| !remove.contains(session_id));
        for id in &remove {
            self.remove_files(*id)?;
        }
        self.manifest
            .records
            .retain(|record| !remove.contains(&record.id));
        self.write_manifest()
    }

    fn session_path(&self, id: u64) -> PathBuf {
        self.directory.join(format!("session-{id}.age"))
    }

    fn next_snapshot_sequence(&self, session_id: u64) -> u64 {
        let prefix = format!("session-{session_id}-bytes-segment-");
        fs::read_dir(&self.directory)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter_map(|name| {
                name.strip_prefix(&prefix)
                    .and_then(|name| name.strip_suffix(".age"))
                    .and_then(|sequence| sequence.parse::<u64>().ok())
            })
            .max()
            .unwrap_or(0)
            .saturating_add(1)
    }

    fn next_sequence(&self, session_id: u64, pane_id: u64) -> u64 {
        let prefix = format!("session-{session_id}-pane-{pane_id}-segment-");
        fs::read_dir(&self.directory)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter_map(|name| {
                name.strip_prefix(&prefix)?
                    .strip_suffix(".age")?
                    .parse()
                    .ok()
            })
            .max()
            .unwrap_or(0u64)
            .saturating_add(1)
    }

    fn flush_segment(
        &mut self,
        session_id: u64,
        pane_id: u64,
        segment: &mut SegmentBuffer,
    ) -> Result<()> {
        let ciphertext = self.recipients.encrypt(&segment.bytes)?;
        let path = self.directory.join(format!(
            "session-{session_id}-pane-{pane_id}-segment-{}.age",
            segment.sequence
        ));
        atomic_write(&path, &ciphertext)?;
        segment.bytes.clear();
        self.write_manifest()
    }

    fn storage_bytes(&self) -> Result<u64> {
        Ok(fs::read_dir(&self.directory)?
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.metadata().ok())
            .map(|metadata| metadata.len())
            .sum())
    }

    fn remove_files(&self, id: u64) -> Result<()> {
        let _ = fs::remove_file(self.session_path(id));
        self.remove_unreferenced_snapshot_segments(id, &[])?;
        let prefix = format!("session-{id}-pane-");
        for entry in fs::read_dir(&self.directory)? {
            let path = entry?.path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix))
            {
                let _ = fs::remove_file(path);
            }
        }
        Ok(())
    }

    fn write_manifest(&self) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(&self.manifest)
            .context("serializing persistence manifest")?;
        atomic_write(&self.manifest_path, &bytes)
    }
}

/// Reads the cleartext portion of the store without opening an encrypted
/// record. This is the data the list command may show before an identity is
/// given.
pub fn read_opaque_records(base: &Path) -> Result<Vec<RestorableRecord>> {
    let path = base.join("persistence").join("manifest.json");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let mut manifest: Manifest =
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))?;
    if manifest.version != MANIFEST_VERSION {
        return Ok(Vec::new());
    }
    let daemon_alive = crate::transport::Endpoint::read(&base.join("zmux.json"))
        .ok()
        .is_some_and(|endpoint| crate::transport::Stream::connect(&endpoint.socket_path).is_ok());
    let boot_changed = manifest.boot_stamp != boot_stamp();
    if !daemon_alive || boot_changed {
        for record in &mut manifest.records {
            record.restorable = true;
        }
    }
    Ok(manifest.records)
}

/// Returns the first session ID that cannot collide with a record in the
/// manifest. This is used even when disk mode is currently configured without
/// recipients, so old encrypted files are not accidentally shadowed if the
/// setting is enabled again later.
pub fn next_record_id(base: &Path) -> Result<u64> {
    let next = read_opaque_records(base)?
        .into_iter()
        .map(|record| record.id)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    Ok(next.max(1))
}

/// Removes one opaque record without decrypting it. This is used when the
/// daemon is not running; an age identity is required to resume a record, not
/// to explicitly forget it.
pub fn forget_opaque_record(base: &Path, id: u64) -> Result<bool> {
    let directory = base.join("persistence");
    let manifest_path = directory.join("manifest.json");
    let bytes = match fs::read(&manifest_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", manifest_path.display()));
        }
    };
    let mut manifest: Manifest = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing {}", manifest_path.display()))?;
    if manifest.version != MANIFEST_VERSION
        || !manifest.records.iter().any(|record| record.id == id)
    {
        return Ok(false);
    }
    let _ = fs::remove_file(directory.join(format!("session-{id}.age")));
    let prefix = format!("session-{id}-pane-");
    for entry in fs::read_dir(&directory)? {
        let path = entry?.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(&prefix))
        {
            let _ = fs::remove_file(path);
        }
    }
    manifest.records.retain(|record| record.id != id);
    let bytes = serde_json::to_vec_pretty(&manifest).context("serializing persistence manifest")?;
    atomic_write(&manifest_path, &bytes)?;
    Ok(true)
}

fn saved_recipients_path(directory: &Path) -> PathBuf {
    directory.join("recipients.json")
}

fn read_saved_recipients(directory: &Path) -> Result<Option<Vec<String>>> {
    let path = saved_recipients_path(directory);
    match fs::read(&path) {
        Ok(bytes) => {
            let options: DaemonOptionsFile = serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {}", path.display()))?;
            Ok(Some(options.recipients))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

fn write_saved_recipients(directory: &Path, recipients: &[String]) -> Result<()> {
    let bytes = serde_json::to_vec(&DaemonOptionsFile {
        recipients: recipients.to_vec(),
    })?;
    atomic_write(&saved_recipients_path(directory), &bytes)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    catalog::write_private_file(&temporary, bytes)
        .with_context(|| format!("writing private file {}", temporary.display()))?;
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path)
            .with_context(|| format!("replacing private file {}", path.display()))?;
    }
    fs::rename(&temporary, path)
        .with_context(|| format!("committing private file {}", path.display()))?;
    Ok(())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn boot_stamp() -> String {
    #[cfg(target_os = "linux")]
    if let Ok(stamp) = fs::read_to_string("/proc/sys/kernel/random/boot_id") {
        return stamp.trim().to_owned();
    }
    // Other platforms do not expose one portable boot identifier through the
    // standard library. A stable per-user store still gets cleanup and opaque
    // listing; Linux additionally detects a reboot explicitly.
    format!("{}-stable", std::env::consts::OS)
}

#[cfg(test)]
#[path = "tests/persistence.rs"]
mod tests;
