//! The receiver's replay filter: which crypto sequence numbers have already
//! been opened, and which one is the newest.
//!
//! Zetta-authored, kept out of `transport.rs` so a synchronisation with
//! upstream never has to merge inside it. Upstream remembered the last 512
//! sequence numbers it had opened in arrival order, so a captured datagram
//! older than that was accepted again as if it were new — and the receiver
//! treated it as fresh contact from wherever it came from. This is the usual
//! IPsec/DTLS shape instead: a high-water mark and a bitmap of the
//! `REPLAY_WINDOW` numbers below it. A number at or below the window's floor
//! cannot be told apart from a replay and is refused outright; SSP has long
//! since superseded whatever it carried.
//!
//! Only a number above the high-water mark is *in order*. Stock Mosh
//! (`network.cc`'s `recv_one`) uses exactly those packets, and only those, for
//! timestamps, for liveness and for roaming the peer's address; an
//! out-of-order packet's payload is still handed to SSP.

/// How many sequence numbers below the newest one the filter remembers.
/// Mosh peers number their datagrams contiguously, so this is how far a
/// datagram may be overtaken and still be accepted.
pub(crate) const REPLAY_WINDOW: u64 = 1024;

const WORDS: usize = (REPLAY_WINDOW / u64::BITS as u64) as usize;

/// What the filter makes of a sequence number before it is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Freshness {
    /// Newer than anything opened so far.
    InOrder,
    /// Inside the window and not yet opened: legitimate reordering.
    OutOfOrder,
    /// Already opened, or too old to know: drop it unopened.
    Replay,
}

impl Freshness {
    pub(crate) fn accepted(self) -> bool {
        self != Self::Replay
    }
}

/// High-water mark plus a ring of `REPLAY_WINDOW` bits indexed by
/// `seq % REPLAY_WINDOW`; bit set means "opened".
#[derive(Debug, Clone)]
pub(crate) struct ReplayWindow {
    highest: Option<u64>,
    bits: [u64; WORDS],
}

impl ReplayWindow {
    pub(crate) fn new() -> Self {
        Self {
            highest: None,
            bits: [0; WORDS],
        }
    }

    /// Classify `seq` without recording it. Call before decrypting, so a
    /// replay costs nothing; record with [`Self::commit`] only once the
    /// datagram authenticated, so a forged header cannot move the window.
    pub(crate) fn check(&self, seq: u64) -> Freshness {
        let Some(highest) = self.highest else {
            return Freshness::InOrder;
        };
        if seq > highest {
            return Freshness::InOrder;
        }
        if highest - seq >= REPLAY_WINDOW || self.is_set(seq) {
            return Freshness::Replay;
        }
        Freshness::OutOfOrder
    }

    /// Record an authenticated `seq`. Returns how it was classified, so a
    /// caller that checked first gets the same answer it acted on.
    pub(crate) fn commit(&mut self, seq: u64) -> Freshness {
        let freshness = self.check(seq);
        match freshness {
            Freshness::Replay => {}
            Freshness::OutOfOrder => self.set(seq),
            Freshness::InOrder => {
                match self.highest {
                    Some(highest) if seq - highest < REPLAY_WINDOW => {
                        // Forget the numbers that are about to be reused by
                        // the ring for the ones newly inside the window.
                        for stale in highest + 1..seq {
                            self.clear(stale);
                        }
                    }
                    _ => self.bits = [0; WORDS],
                }
                self.highest = Some(seq);
                self.set(seq);
            }
        }
        freshness
    }

    fn slot(seq: u64) -> (usize, u64) {
        let index = seq % REPLAY_WINDOW;
        (
            (index / u64::from(u64::BITS)) as usize,
            1 << (index % u64::from(u64::BITS)),
        )
    }

    fn is_set(&self, seq: u64) -> bool {
        let (word, mask) = Self::slot(seq);
        self.bits[word] & mask != 0
    }

    fn set(&mut self, seq: u64) {
        let (word, mask) = Self::slot(seq);
        self.bits[word] |= mask;
    }

    fn clear(&mut self, seq: u64) {
        let (word, mask) = Self::slot(seq);
        self.bits[word] &= !mask;
    }
}

#[cfg(test)]
#[path = "tests/replay.rs"]
mod tests;
