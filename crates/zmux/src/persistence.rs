//! Encrypted detached-session persistence.
//!
//! The daemon owns the recipient side of this module. Identities are a client
//! concern: a daemon can write encrypted records without ever receiving the
//! private keys that can read them back. The age files produced here are
//! ordinary age v1 files, so the store does not introduce a second encrypted
//! file format for metadata, snapshots, or scrollback.

use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    error::Error as StdError,
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

use crate::retention::Retention;
use crate::{auth::SessionSecret, catalog, protocol::BackgroundSessionSummary, secret_prompt};

mod identity;
mod postquantum;
mod recipients;

// The module was one file before it was split by responsibility; these keep
// every name reachable as `crate::persistence::…`, which is how the daemon,
// the client and the CLI all refer to them.
pub use identity::*;
pub use postquantum::*;
pub use recipients::*;

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

/// The conventional local SSH identity used as the age fallback when a client
/// has no configured identity. This is deliberately client-side discovery: the
/// daemon never receives the path or the private key.
pub fn default_identity_path() -> Option<PathBuf> {
    let home =
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)?;
    let path = home.join(".ssh").join("id_ed25519");
    path.is_file().then_some(path)
}

/// Why resolving a configured recipient failed.
///
/// A temporary failure means that the configuration is valid but the network
/// lookup could not be completed right now.  The application may retain a
/// session in memory and try the same disk configuration again later.  A
/// permanent failure is part of the configuration itself and must remain an
/// actionable error for callers such as `zmux resume` and the CLI.
#[derive(Debug)]
pub enum RecipientResolutionError {
    Temporary(anyhow::Error),
    Permanent(anyhow::Error),
}

impl RecipientResolutionError {
    pub fn is_temporary(&self) -> bool {
        matches!(self, Self::Temporary(_))
    }

    pub fn into_anyhow(self) -> anyhow::Error {
        match self {
            Self::Temporary(error) | Self::Permanent(error) => error,
        }
    }
}

impl fmt::Display for RecipientResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Temporary(error) | Self::Permanent(error) => error.fmt(formatter),
        }
    }
}

impl StdError for RecipientResolutionError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Temporary(error) | Self::Permanent(error) => error.source(),
        }
    }
}

fn permanent_recipient_error(error: impl Into<anyhow::Error>) -> RecipientResolutionError {
    RecipientResolutionError::Permanent(error.into())
}

fn temporary_recipient_error(error: impl Into<anyhow::Error>) -> RecipientResolutionError {
    RecipientResolutionError::Temporary(error.into())
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
    /// The sealed session key that goes with `verifier`, when the secret was
    /// generated rather than typed. See
    /// [`crate::protocol::BackgroundSessionSummary::key_envelope`].
    ///
    /// Kept inside the record's own ciphertext, which is doubly encrypted and
    /// deliberately so: the record and the envelope are sealed to the same
    /// recipients, so one routine opens a session key wherever it is found and
    /// the disk path needs no special case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_envelope: Option<String>,
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
    /// The terminal size used when this screen was captured. Old records did
    /// not carry it, so both values are optional as one backward-compatible
    /// pair.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub columns: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<u16>,
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
    /// As [`PersistedSession::key_envelope`]. Defaulted so a record written
    /// before automatic protection existed still loads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key_envelope: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    columns: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lines: Option<u16>,
}

/// Authentication backoff is updated by the daemon after a client has already
/// decrypted a record. Keep that small mutable part in its own encrypted age
/// file so a failed resume does not have to reconstruct snapshot locations (and
/// accidentally lose their optional dimensions) through the wire schema.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedAuthentication {
    updated_at: u64,
    failed_authentications: u32,
    backoff_seconds: u64,
}

fn snapshot_path(directory: &Path, session_id: u64, sequence: u64) -> PathBuf {
    directory.join(format!("session-{session_id}-bytes-segment-{sequence}.age"))
}

fn authentication_path(directory: &Path, session_id: u64) -> PathBuf {
    directory.join(format!("session-{session_id}-auth.age"))
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
    let authentication = match fs::read(authentication_path(directory, id)) {
        Ok(ciphertext) => Some(
            serde_json::from_slice::<PersistedAuthentication>(&identities.decrypt(&ciphertext)?)
                .context("parsing encrypted session authentication")?,
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("reading encrypted session authentication"),
    };
    let mut snapshots = load_snapshots(directory, id, &metadata.snapshots, identities)?;
    restore_scrollback(directory, id, &mut snapshots, identities)?;
    Ok(PersistedSession {
        id: metadata.id,
        created_at: metadata.created_at,
        updated_at: authentication
            .as_ref()
            .map_or(metadata.updated_at, |authentication| {
                authentication.updated_at
            }),
        summary: metadata.summary,
        state: metadata.state,
        verifier: metadata.verifier,
        key_envelope: metadata.key_envelope,
        failed_authentications: authentication
            .as_ref()
            .map_or(metadata.failed_authentications, |authentication| {
                authentication.failed_authentications
            }),
        backoff_seconds: authentication
            .as_ref()
            .map_or(metadata.backoff_seconds, |authentication| {
                authentication.backoff_seconds
            }),
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
                columns: location.columns,
                lines: location.lines,
            })
        })
        .collect()
}

/// Rebuilds a disk snapshot from its saved screen and the output segments the
/// daemon flushed for that pane.
///
/// The screen is deliberately bounded by [`Retention::Disk`]. A segment is
/// decrypted and fed to that screen before the next one is opened, so a large
/// history costs one bounded terminal grid plus one age segment rather than a
/// `Vec` containing the whole history.
fn restore_scrollback(
    directory: &Path,
    session_id: u64,
    snapshots: &mut [PersistedSnapshot],
    identities: &IdentitySet,
) -> Result<()> {
    let mut retained = Vec::new();
    let mut retained_by_pane = HashMap::new();
    for (index, snapshot) in snapshots.iter().enumerate() {
        let (Some(columns), Some(lines)) = (snapshot.columns, snapshot.lines) else {
            continue;
        };
        let retained_index = retained.len();
        retained_by_pane
            .entry(snapshot.pane_id)
            .or_insert(retained_index);
        retained.push((index, Retention::Disk.new_retained(columns, lines)));
    }
    if retained.is_empty() {
        // Records written before pane dimensions were persisted retain their
        // exact old behavior: the saved screen is returned untouched.
        return Ok(());
    }

    for (index, screen) in &mut retained {
        // Move the saved screen into the bounded emulator. Cloning it here
        // would briefly double the largest per-pane allocation before the
        // first scrollback segment is even read.
        screen.seed(std::mem::take(&mut snapshots[*index].bytes));
    }

    for path in scrollback_paths(directory, session_id)? {
        let Some((pane_id, _)) = scrollback_path_parts(&path, session_id) else {
            continue;
        };
        let Some(&retained_index) = retained_by_pane.get(&pane_id) else {
            continue;
        };
        identities.decrypt_file(&path, |chunk| {
            retained[retained_index].1.push(chunk);
            Ok(())
        })?;
    }

    for (index, screen) in retained {
        snapshots[index].bytes = screen.snapshot();
    }
    Ok(())
}

fn scrollback_path_parts(path: &Path, session_id: u64) -> Option<(u64, u64)> {
    let prefix = format!("session-{session_id}-pane-");
    let name = path.file_name()?.to_str()?;
    let (pane_id, sequence) = name
        .strip_prefix(&prefix)?
        .strip_suffix(".age")?
        .split_once("-segment-")?;
    Some((pane_id.parse().ok()?, sequence.parse().ok()?))
}

fn scrollback_paths(directory: &Path, session_id: u64) -> Result<Vec<PathBuf>> {
    let mut paths = fs::read_dir(directory)
        .with_context(|| format!("reading persistence directory {}", directory.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter_map(|path| scrollback_path_parts(&path, session_id).map(|parts| (parts, path)))
        .collect::<Vec<_>>();
    paths.sort_by_key(|((pane_id, sequence), _)| (*pane_id, *sequence));
    Ok(paths.into_iter().map(|(_, path)| path).collect())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    /// A compatibility artifact. Nothing in this image reads it; it is kept
    /// current so an older image that still compares it sees this boot. It
    /// cannot be dropped: `deny_unknown_fields` plus the `unwrap_or_default`
    /// read in [`PersistenceStore::open_with_recovery_state`] means an older
    /// image reading a manifest without the field would silently reset the
    /// manifest and orphan every encrypted record. See [`boot_stamp`].
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
    /// in-process daemon upgrade. A record's restorability follows the daemon
    /// that wrote it no longer answering, full stop: any start that is not an
    /// in-process handoff is evidence the previous daemon was lost, so it
    /// recovers every record.
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
        // Kept current for an older image that still compares it; see
        // `boot_stamp`. This one never branches on it, so the refresh is
        // unconditional rather than part of the recovery below.
        manifest.boot_stamp = boot_stamp();
        let recovered = !replacing_daemon;
        if recovered {
            for record in &mut manifest.records {
                record.restorable = true;
            }
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

    /// Persists only authentication backoff after a failed resume. The main
    /// record is encrypted to recipients the daemon can write to but cannot
    /// decrypt, so a small encrypted sidecar is the only way to update these
    /// counters without dropping snapshot geometry that was supplied by an
    /// older client record.
    pub fn update_authentication(
        &mut self,
        id: u64,
        updated_at: u64,
        failed_authentications: u32,
        backoff_seconds: u64,
    ) -> Result<()> {
        let Some(record_index) = self
            .manifest
            .records
            .iter()
            .position(|record| record.id == id)
        else {
            anyhow::bail!("persisted session {id} does not exist");
        };
        let authentication = PersistedAuthentication {
            updated_at,
            failed_authentications,
            backoff_seconds,
        };
        let plaintext =
            serde_json::to_vec(&authentication).context("serializing session authentication")?;
        let ciphertext = self.recipients.encrypt(&plaintext)?;
        let path = authentication_path(&self.directory, id);
        let previous_bytes = fs::metadata(&path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        atomic_write(&path, &ciphertext)?;
        let new_bytes = ciphertext.len() as u64;
        let record = &mut self.manifest.records[record_index];
        record.metadata_bytes = record
            .metadata_bytes
            .saturating_sub(previous_bytes)
            .saturating_add(new_bytes);
        record.updated_at = record.updated_at.max(updated_at);
        self.write_manifest()
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
            key_envelope: session.key_envelope.clone(),
            failed_authentications: session.failed_authentications,
            backoff_seconds: session.backoff_seconds,
            snapshots,
        };
        let plaintext =
            serde_json::to_vec(&metadata).context("serializing persisted session metadata")?;
        let ciphertext = self.recipients.encrypt(&plaintext)?;
        atomic_write(&self.session_path(session.id), &ciphertext)?;
        // A complete session write carries the authoritative counters in its
        // metadata, so an older failed-resume sidecar must not override it on
        // the next load.
        let _ = fs::remove_file(authentication_path(&self.directory, session.id));
        self.remove_unreferenced_snapshot_segments(session.id, &metadata.snapshots)?;
        let now = unix_now();
        let created_at = self
            .manifest
            .records
            .iter()
            .find(|record| record.id == session.id)
            .map_or(session.created_at, |record| record.created_at);
        let protected = session.verifier.is_some() || session.summary.authentication_required;
        let auto_protected = session.key_envelope.is_some();
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
            record.auto_protected = auto_protected;
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
                auto_protected,
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
        let mut output = Vec::new();
        for path in scrollback_paths(&self.directory, id)? {
            identities.decrypt_file(&path, |chunk| {
                output.extend_from_slice(chunk);
                Ok(())
            })?;
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
                columns: snapshot.columns,
                lines: snapshot.lines,
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
        let _ = fs::remove_file(authentication_path(&self.directory, id));
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
        let _ = fs::remove_file(authentication_path(&self.directory, id));
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
    if !daemon_alive {
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
    let _ = fs::remove_file(authentication_path(&directory, id));
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

/// The value written to [`Manifest::boot_stamp`], which this image records but
/// never compares. It is retained, with its values byte-identical to what
/// older images wrote, because rollback to an older build is supported
/// (`--upgrade` crosses a version boundary) and such a build both reads the
/// field and rejects a manifest that has lost it.
///
/// Do not finish the removal by deleting this. Do not "complete" it with a
/// macOS or Windows source either: the comparison it fed could not change an
/// outcome on any platform, because both former consumers already had a
/// stronger signal beside it — a live daemon answering, and an in-process
/// handoff that cannot span a reboot. Worse, in the handoff path a stamp that
/// wrongly reported a reboot *causes* recovery, which prunes against an empty
/// live set; an approximate stamp there would delete the records of sessions
/// that are still running. This becomes a question worth asking again only if
/// a recorded pid ever becomes load-bearing.
fn boot_stamp() -> String {
    #[cfg(target_os = "linux")]
    if let Ok(stamp) = fs::read_to_string("/proc/sys/kernel/random/boot_id") {
        return stamp.trim().to_owned();
    }
    format!("{}-stable", std::env::consts::OS)
}

#[cfg(test)]
#[path = "tests/persistence.rs"]
mod tests;
