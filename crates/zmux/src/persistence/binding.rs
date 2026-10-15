//! What an encrypted payload says it is, so that a file copied into another
//! file's place is refused rather than read as if it belonged there.
//!
//! Every file the store writes starts its *plaintext* with one header line
//! naming the slot it was written for: its payload kind, record, pane and
//! sequence where those apply, and which write of the record it belongs to.
//! The loader compares that line with the slot it actually read the file from.
//! Inside the age payload the header is authenticated along with everything
//! else, so it cannot be changed without the file failing to decrypt.
//!
//! | kind | names | written |
//! | --- | --- | --- |
//! | `metadata` | record, generation | `session-R.age` |
//! | `authentication` | record, SHA-256 of the metadata file | `session-R-auth.age` |
//! | `snapshots` | record, sequence, generation | `session-R-bytes-segment-S.age` |
//! | `scrollback` | record, pane, sequence | `session-R-pane-P-segment-S.age` |
//!
//! The generation is chosen at random for each complete write of a record, so
//! a snapshot segment from an earlier write of the same record does not match
//! the metadata beside it. The daemon cannot read a record's metadata to learn
//! its generation, so the authentication sidecar names the metadata file it
//! updates by digest instead: a sidecar left over from an earlier metadata
//! file — the one a complete write removes — no longer matches, and is
//! ignored rather than allowed to replace the newer counters.
//!
//! **What this stops.** Copying or replaying a valid file this store wrote into
//! a slot it was not written for: another record's metadata, sidecar or
//! segments; a segment from another pane or under another sequence number; a
//! payload of one kind in another kind's slot; a snapshot segment from an
//! earlier write; an authentication sidecar from before the current metadata.
//!
//! **What it does not.** age encrypts to a public recipient and authenticates
//! only under a key the writer chose, so it says nothing about *who* wrote a
//! file: a same-user process that knows the recipients — they are saved beside
//! the records — can write a brand-new, correctly bound file for any slot. Nor
//! is there rollback protection for the store as a whole: an earlier metadata
//! file together with its own sidecar and segments is a consistent earlier
//! state, and nothing in a writable directory can say it is not the latest.
//! Both need a trusted anchor outside the store, which this does not have.
//! Among sidecars for the same metadata file, an older one cannot be told from
//! the newest for the same reason.
//!
//! **Files without a header.** Files written before bindings existed have none
//! and are still read, because refusing them would strand every existing store
//! and a forger can write an unbound file anyway. Their slot is checked where
//! the payload itself says something: legacy metadata must name the requested
//! record. A bound metadata file's snapshot segments must be bound too, as
//! both come from one write; an unbound sidecar beside bound metadata is stale.
//! Scrollback segments outlive the write that started them, so an unbound one
//! is accepted beside bound metadata — copying a *legacy* segment between
//! slots is the gap that remains, until such records are rewritten or age out
//! of the store. A record is rewritten bound the next time the daemon saves
//! it. Records written with bindings cannot be read by builds without them.

use std::fmt::Write as _;

use super::*;

/// The start of every bound plaintext. The leading NUL is what no unbound
/// payload begins with: metadata is JSON and segments are terminal output.
const MAGIC: &[u8] = b"\0zmux-binding/1 ";
/// The longest header accepted, magic and newline included. The longest one
/// written is under 160 bytes.
const MAX_HEADER_BYTES: usize = 256;

/// Which of a record's files a payload is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PayloadKind {
    Metadata,
    Authentication,
    Snapshots,
    Scrollback,
}

impl PayloadKind {
    fn name(self) -> &'static str {
        match self {
            Self::Metadata => "metadata",
            Self::Authentication => "authentication",
            Self::Snapshots => "snapshots",
            Self::Scrollback => "scrollback",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        [
            Self::Metadata,
            Self::Authentication,
            Self::Snapshots,
            Self::Scrollback,
        ]
        .into_iter()
        .find(|kind| kind.name() == name)
    }
}

/// The slot a payload was written for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Binding {
    pub(super) kind: PayloadKind,
    pub(super) record: u64,
    pub(super) pane: Option<u64>,
    pub(super) sequence: Option<u64>,
    /// Metadata and snapshots: the complete write of the record they are from.
    pub(super) generation: Option<u128>,
    /// Authentication: the SHA-256 of the metadata file it updates.
    pub(super) metadata: Option<[u8; 32]>,
}

impl Binding {
    pub(super) fn metadata(record: u64, generation: u128) -> Self {
        Self {
            generation: Some(generation),
            ..Self::bare(PayloadKind::Metadata, record)
        }
    }

    pub(super) fn authentication(record: u64, metadata: [u8; 32]) -> Self {
        Self {
            metadata: Some(metadata),
            ..Self::bare(PayloadKind::Authentication, record)
        }
    }

    pub(super) fn snapshots(record: u64, sequence: u64, generation: u128) -> Self {
        Self {
            sequence: Some(sequence),
            generation: Some(generation),
            ..Self::bare(PayloadKind::Snapshots, record)
        }
    }

    pub(super) fn scrollback(record: u64, pane: u64, sequence: u64) -> Self {
        Self {
            pane: Some(pane),
            sequence: Some(sequence),
            ..Self::bare(PayloadKind::Scrollback, record)
        }
    }

    /// A record's metadata slot, whichever write fills it.
    pub(super) fn metadata_slot(record: u64) -> Self {
        Self::bare(PayloadKind::Metadata, record)
    }

    /// A record's authentication slot. Which metadata the sidecar is for is
    /// compared separately: a mismatch there makes it stale, not misplaced.
    pub(super) fn authentication_slot(record: u64) -> Self {
        Self::bare(PayloadKind::Authentication, record)
    }

    fn bare(kind: PayloadKind, record: u64) -> Self {
        Self {
            kind,
            record,
            pane: None,
            sequence: None,
            generation: None,
            metadata: None,
        }
    }

    /// The header line written before the payload, fields in a fixed order.
    pub(super) fn header(&self) -> Vec<u8> {
        let mut line = format!("kind={} record={}", self.kind.name(), self.record);
        if let Some(pane) = self.pane {
            let _ = write!(line, " pane={pane}");
        }
        if let Some(sequence) = self.sequence {
            let _ = write!(line, " sequence={sequence}");
        }
        if let Some(generation) = self.generation {
            let _ = write!(line, " generation={generation:032x}");
        }
        if let Some(metadata) = self.metadata {
            line.push_str(" metadata=");
            for byte in metadata {
                let _ = write!(line, "{byte:02x}");
            }
        }
        let mut header = Vec::with_capacity(MAGIC.len() + line.len() + 1);
        header.extend_from_slice(MAGIC);
        header.extend_from_slice(line.as_bytes());
        header.push(b'\n');
        header
    }

    /// Parses the line between the magic and the newline. Each kind has
    /// exactly its own fields, in the order [`Self::header`] writes them.
    fn parse(line: &[u8]) -> Result<Self> {
        let line = std::str::from_utf8(line).context("record binding is not text")?;
        let mut fields = line.split(' ');
        let mut field = |name: &str| -> Result<&str> {
            fields
                .next()
                .and_then(|field| field.strip_prefix(name)?.strip_prefix('='))
                .with_context(|| format!("record binding has no {name}"))
        };
        let kind = PayloadKind::parse(field("kind")?).context("unknown record binding kind")?;
        let record = field("record")?
            .parse()
            .context("invalid record in binding")?;
        let mut binding = Self::bare(kind, record);
        if kind == PayloadKind::Scrollback {
            binding.pane = Some(field("pane")?.parse().context("invalid pane in binding")?);
        }
        if matches!(kind, PayloadKind::Snapshots | PayloadKind::Scrollback) {
            binding.sequence = Some(
                field("sequence")?
                    .parse()
                    .context("invalid sequence in binding")?,
            );
        }
        if matches!(kind, PayloadKind::Metadata | PayloadKind::Snapshots) {
            let generation = field("generation")?;
            anyhow::ensure!(generation.len() == 32, "invalid generation in binding");
            binding.generation = Some(
                u128::from_str_radix(generation, 16).context("invalid generation in binding")?,
            );
        }
        if kind == PayloadKind::Authentication {
            binding.metadata = Some(parse_digest(field("metadata")?)?);
        }
        anyhow::ensure!(
            fields.next().is_none(),
            "record binding has unexpected fields"
        );
        Ok(binding)
    }

    /// Refuses this payload unless it was written for `expected`'s slot. A
    /// generation is compared only when `expected` names one; a sidecar's
    /// metadata digest is the caller's to compare, because a mismatch there
    /// means stale rather than misplaced.
    pub(super) fn check(&self, expected: &Self, path: &Path) -> Result<()> {
        anyhow::ensure!(
            self.kind == expected.kind
                && self.record == expected.record
                && self.pane == expected.pane
                && self.sequence == expected.sequence,
            "refusing {}: it holds {self}, not {expected}; it was copied from another file",
            path.display(),
        );
        anyhow::ensure!(
            expected.generation.is_none() || self.generation == expected.generation,
            "refusing {}: it holds {self} from a different write of that record than its \
             metadata; it was copied from an earlier one",
            path.display(),
        );
        Ok(())
    }
}

impl fmt::Display for Binding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let record = self.record;
        match (self.kind, self.pane, self.sequence) {
            (PayloadKind::Scrollback, Some(pane), Some(sequence)) => write!(
                formatter,
                "scrollback segment {sequence} of pane {pane} of record {record}"
            ),
            (PayloadKind::Snapshots, _, Some(sequence)) => {
                write!(formatter, "snapshot segment {sequence} of record {record}")
            }
            (kind, ..) => write!(formatter, "the {} of record {record}", kind.name()),
        }
    }
}

/// A fresh generation for one complete write of a record.
pub(super) fn new_generation() -> Result<u128> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).context("choosing a record generation")?;
    Ok(u128::from_be_bytes(bytes))
}

/// What a sidecar names its metadata file by.
pub(super) fn metadata_digest(ciphertext: &[u8]) -> [u8; 32] {
    use sha2::Digest as _;

    sha2::Sha256::digest(ciphertext).into()
}

fn parse_digest(text: &str) -> Result<[u8; 32]> {
    anyhow::ensure!(text.len() == 64, "invalid metadata digest in binding");
    let mut digest = [0; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
            .context("invalid metadata digest in binding")?;
    }
    Ok(digest)
}

/// How far the start of a plaintext has decided whether it is bound.
enum Start {
    /// Too short to tell yet.
    Undecided,
    /// Does not start with the magic: a file from before bindings.
    Unbound,
    /// Bound as this, with the payload starting at this offset.
    Bound(Binding, usize),
}

fn start(bytes: &[u8]) -> Result<Start> {
    let compared = bytes.len().min(MAGIC.len());
    if bytes[..compared] != MAGIC[..compared] {
        return Ok(Start::Unbound);
    }
    if bytes.len() < MAGIC.len() {
        return Ok(Start::Undecided);
    }
    let searched = &bytes[..bytes.len().min(MAX_HEADER_BYTES)];
    match searched.iter().position(|byte| *byte == b'\n') {
        Some(end) => Ok(Start::Bound(
            Binding::parse(&bytes[MAGIC.len()..end])?,
            end + 1,
        )),
        None if bytes.len() >= MAX_HEADER_BYTES => {
            anyhow::bail!("record binding is longer than {MAX_HEADER_BYTES} bytes")
        }
        None => Ok(Start::Undecided),
    }
}

/// Splits a whole decrypted payload into its binding, when it has one, and
/// the payload after it.
pub(super) fn split(plaintext: &[u8]) -> Result<(Option<Binding>, &[u8])> {
    match start(plaintext)? {
        Start::Bound(binding, offset) => Ok((Some(binding), &plaintext[offset..])),
        Start::Unbound => Ok((None, plaintext)),
        // Shorter than the magic and a prefix of it: an unbound payload that
        // happens to start that way. Any longer and it is a cut-off header.
        Start::Undecided if plaintext.len() < MAGIC.len() => Ok((None, plaintext)),
        Start::Undecided => anyhow::bail!("record binding is cut off"),
    }
}

/// Checks the binding at the start of a payload decrypted a chunk at a time,
/// passing on everything after it — or all of it, for an unbound payload.
///
/// Scrollback is read this way so its history is never assembled in memory,
/// which is why the header may arrive split across chunks.
pub(super) struct StreamCheck<'a> {
    expected: Binding,
    path: &'a Path,
    /// What has arrived before the start was decided; `None` once it has been.
    pending: Option<Vec<u8>>,
}

impl<'a> StreamCheck<'a> {
    pub(super) fn new(expected: Binding, path: &'a Path) -> Self {
        Self {
            expected,
            path,
            pending: Some(Vec::new()),
        }
    }

    pub(super) fn feed(
        &mut self,
        chunk: &[u8],
        mut emit: impl FnMut(&[u8]) -> Result<()>,
    ) -> Result<()> {
        let Some(pending) = self.pending.as_mut() else {
            return emit(chunk);
        };
        pending.extend_from_slice(chunk);
        let offset = match start(pending).with_context(|| self.context())? {
            Start::Undecided => return Ok(()),
            Start::Unbound => 0,
            Start::Bound(binding, offset) => {
                binding.check(&self.expected, self.path)?;
                offset
            }
        };
        let pending = self.pending.take().expect("pending was just borrowed");
        if offset < pending.len() {
            emit(&pending[offset..])?;
        }
        Ok(())
    }

    /// Ends the stream, passing on a short unbound payload that never grew
    /// long enough to decide on.
    pub(super) fn finish(mut self, emit: impl FnOnce(&[u8]) -> Result<()>) -> Result<()> {
        let Some(pending) = self.pending.take() else {
            return Ok(());
        };
        // Undecided until the end means shorter than a header, so this is
        // either an unbound payload or a header cut off part way.
        let (_, payload) = split(&pending).with_context(|| self.context())?;
        if payload.is_empty() {
            Ok(())
        } else {
            emit(payload)
        }
    }

    fn context(&self) -> String {
        format!("reading the binding of {}", self.path.display())
    }
}

#[cfg(test)]
#[path = "../tests/persistence/binding.rs"]
mod tests;
