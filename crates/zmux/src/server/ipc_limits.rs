//! What one local client may make the daemon hold at once, in aggregate.
//!
//! Each frame is already bounded ([`crate::transport::MAX_MESSAGE_BYTES`]) and
//! so is each snapshot ([`crate::retention::MAX_SNAPSHOT_BYTES`]). Neither
//! bounds how many of them arrive together: every accepted connection is a
//! thread, and a detach can describe a few thousand snapshots in one 64 KiB
//! frame. This module holds the bounds on those totals — connections, how long
//! a request may take to arrive, and the raw snapshot bytes being read — so the
//! numbers and the reasons for them live in one place.

use super::*;

/// How many connections the daemon serves at once.
///
/// A connection is a thread, and the long-lived ones are legitimate: one event
/// subscription per client process, one stream per shared or remote pane and
/// viewer, plus the short requests in flight. A heavy user with a few windows
/// of shared and remote panes holds a few hundred. Past this a connection is
/// answered with an error and closed, so a local process cannot make the
/// daemon start threads without bound.
pub(super) const MAX_CONNECTIONS: usize = 1024;

/// How long a connection has, from being accepted, to deliver its request —
/// and on Windows, the attestation that may precede it.
///
/// A client sends its request as soon as it connects. Remote clients reach the
/// daemon through an SSH forward, so this allows for a slow link rather than a
/// local socket. Only the request is bounded: a subscription or a shared
/// stream then stays open for as long as its client does.
pub(super) const REQUEST_DEADLINE: Duration = Duration::from_secs(10);

/// How many raw snapshots one request may carry.
///
/// A detach or resume carries one per pane of one session, and a window puts
/// at most 64 panes in a tab. This leaves room beyond that while keeping a
/// single request from describing thousands.
pub(super) const MAX_SNAPSHOTS_PER_REQUEST: usize = 256;

/// The raw snapshot bytes one request may carry.
///
/// Sixteen snapshots at the per-pane ceiling, or a full 64-pane tab at 2 MiB a
/// pane — about what 2,000 lines of densely coloured, wide output encode to. A
/// typical pane's screen is a few hundred kilobytes.
pub(super) const MAX_SNAPSHOT_BYTES_PER_REQUEST: usize = 128 * 1024 * 1024;

/// The raw snapshot bytes being read across every request at once.
///
/// Two maximal requests, or many ordinary ones — a window closing with all its
/// tabs detaches them together. A request that does not fit waits for room
/// rather than failing, for up to [`SNAPSHOT_BUDGET_WAIT`].
pub(super) const MAX_SNAPSHOT_BYTES_IN_FLIGHT: usize = 256 * 1024 * 1024;

/// How long a request waits for snapshot room before it is refused.
pub(super) const SNAPSHOT_BUDGET_WAIT: Duration = Duration::from_secs(10);

/// How long a request's snapshot bytes may take to arrive once reading them
/// starts. A full request crosses a local socket in well under a second; the
/// rest is for a loaded machine or a debug build. Without it a client that
/// stopped sending would hold its share of the budget forever.
pub(super) const SNAPSHOT_READ_DEADLINE: Duration = Duration::from_secs(60);

/// A claim on one of [`MAX_CONNECTIONS`], given back when dropped.
pub(super) struct ConnectionPermit(Arc<AtomicUsize>);

impl ConnectionPermit {
    pub(super) fn claim(in_use: &Arc<AtomicUsize>, capacity: usize) -> Option<Self> {
        in_use
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < capacity).then_some(count + 1)
            })
            .ok()
            .map(|_| Self(Arc::clone(in_use)))
    }
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Answers a connection there is no room for, without reading from it.
///
/// Done on the accepting thread, which is why it must not wait: a fresh
/// socket's send buffer takes the short reply at once, and the timeout only
/// guards against a platform where it would not.
pub(super) fn refuse_connection(stream: Stream) {
    let mut connection = Connection::new(stream);
    let _ = connection.set_write_timeout(Some(Duration::from_millis(100)));
    let _ = connection.send(&Response::Error {
        message: "the multiplexer is serving too many connections".to_owned(),
    });
}

/// The raw snapshot bytes being read across every request, against
/// [`MAX_SNAPSHOT_BYTES_IN_FLIGHT`].
pub(super) struct SnapshotBudget {
    capacity: usize,
    reserved: Mutex<usize>,
    released: Condvar,
}

impl Default for SnapshotBudget {
    fn default() -> Self {
        Self::with_capacity(MAX_SNAPSHOT_BYTES_IN_FLIGHT)
    }
}

impl SnapshotBudget {
    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity,
            reserved: Mutex::new(0),
            released: Condvar::new(),
        }
    }

    /// Reserves `bytes`, waiting until `deadline` for other requests to give
    /// room back.
    pub(super) fn reserve(
        &self,
        bytes: usize,
        deadline: Instant,
    ) -> Result<SnapshotReservation<'_>> {
        anyhow::ensure!(
            bytes <= self.capacity,
            "{bytes} bytes of pane snapshots exceed the {} the multiplexer reads at once",
            self.capacity
        );
        let mut reserved = self
            .reserved
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while *reserved + bytes > self.capacity {
            let remaining = deadline.saturating_duration_since(Instant::now());
            anyhow::ensure!(
                !remaining.is_zero(),
                "the multiplexer is already reading as many pane snapshots as it may"
            );
            reserved = self
                .released
                .wait_timeout(reserved, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
        *reserved += bytes;
        Ok(SnapshotReservation {
            budget: self,
            bytes,
        })
    }

    #[cfg(test)]
    pub(super) fn reserved(&self) -> usize {
        *self
            .reserved
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Room in a [`SnapshotBudget`], given back when dropped.
pub(super) struct SnapshotReservation<'a> {
    budget: &'a SnapshotBudget,
    bytes: usize,
}

impl Drop for SnapshotReservation<'_> {
    fn drop(&mut self) {
        let mut reserved = self
            .budget
            .reserved
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *reserved -= self.bytes;
        drop(reserved);
        self.budget.released.notify_all();
    }
}

/// The raw snapshots a request carried, keyed by pane, and the budget they
/// hold until the caller has stored them.
pub(super) struct ReceivedSnapshots<'a> {
    pub(super) bytes: HashMap<u64, Vec<u8>>,
    _reservation: SnapshotReservation<'a>,
}

/// Reads the raw snapshots that follow a request, described by `(pane, length)`
/// pairs in the order they were sent.
///
/// Everything is checked against what the request declared before a byte is
/// read: the count, each length, and their total; then the total is reserved
/// from the daemon-wide budget. A refusal therefore costs the daemon nothing
/// it would have to read first.
pub(super) fn read_snapshots<'a>(
    budget: &'a SnapshotBudget,
    connection: &mut Connection,
    snapshots: &[(u64, usize)],
) -> Result<ReceivedSnapshots<'a>> {
    anyhow::ensure!(
        snapshots.len() <= MAX_SNAPSHOTS_PER_REQUEST,
        "a request carried {} pane snapshots, more than the {MAX_SNAPSHOTS_PER_REQUEST} allowed",
        snapshots.len()
    );
    let mut total = 0_usize;
    for (_, length) in snapshots {
        anyhow::ensure!(
            *length <= crate::retention::MAX_SNAPSHOT_BYTES,
            "a pane snapshot exceeded the retention limit"
        );
        total = total
            .checked_add(*length)
            .filter(|total| *total <= MAX_SNAPSHOT_BYTES_PER_REQUEST)
            .context("a request's pane snapshots exceeded the per-request limit")?;
    }
    let reservation = budget.reserve(total, Instant::now() + SNAPSHOT_BUDGET_WAIT)?;
    let deadline = Instant::now() + SNAPSHOT_READ_DEADLINE;
    let mut bytes = HashMap::with_capacity(snapshots.len());
    for (pane_id, length) in snapshots {
        bytes.insert(*pane_id, connection.read_exact_by(*length, deadline)?);
    }
    Ok(ReceivedSnapshots {
        bytes,
        _reservation: reservation,
    })
}

#[cfg(test)]
#[path = "../tests/server/ipc_limits.rs"]
mod tests;
