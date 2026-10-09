//! Parsing PTY output on a thread of its own, so reading the next bytes overlaps parsing the
//! last ones.
//!
//! Reading a PTY is not cheap: Linux copies a pty's output out through a small stack buffer, so
//! a terminal receiving a flood of plain text spent two fifths of its reader thread in the
//! kernel, all of it time the parser sat idle. The event loop therefore only reads; it hands
//! what it read to this thread in chunks from a fixed pool, and this thread takes the terminal
//! lock and parses them. The pool is the backpressure: once every chunk is waiting to be parsed,
//! the event loop blocks until one comes back, so a PTY can be read ahead of the parser by at
//! most [`READ_BUFFER_SIZE`] — the same bound a single-threaded reader had before it forced a
//! terminal lock.
//!
//! Zetta-authored, with no upstream counterpart; `event_loop.rs` only drives it.

use std::io;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::thread::JoinHandle;
use std::time::Instant;

#[cfg(windows)]
use std::time::Duration;

use vte::ansi;

use crate::event::{Event, EventListener};
use crate::event_loop::{READ_BUFFER_SIZE, ReplayBarrier};
use crate::sync::FairMutex;
use crate::term::Term;
use crate::thread;

/// Bytes parsed under one terminal lock before it is released for the window to draw. Also the
/// size of one chunk, so a chunk is never split across two locks.
pub(crate) const MAX_LOCKED_READ: usize = u16::MAX as usize;

/// Chunks in flight between the reader and the parser.
const CHUNK_COUNT: usize = READ_BUFFER_SIZE / MAX_LOCKED_READ;

/// Bytes read from the PTY, waiting to be parsed.
pub(crate) struct Chunk {
    bytes: Box<[u8]>,
    len: usize,
}

impl Chunk {
    fn new() -> Self {
        Self { bytes: vec![0; MAX_LOCKED_READ].into_boxed_slice(), len: 0 }
    }

    /// What has been read into the chunk so far.
    pub(crate) fn filled(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    /// Where the next read goes; empty once the chunk is full.
    pub(crate) fn unfilled(&mut self) -> &mut [u8] {
        &mut self.bytes[self.len..]
    }

    pub(crate) fn advance(&mut self, read: usize) {
        self.len += read;
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// What the parser thread accumulated, handed back when it ends.
pub(crate) struct ParseOutcome {
    pub(crate) parser: ansi::Processor,
    #[cfg(windows)]
    pub(crate) parse_calls: u64,
    #[cfg(windows)]
    pub(crate) parse_time: Duration,
}

/// The event loop's handle on the parser thread.
pub(crate) struct ParserThread {
    filled: Option<SyncSender<Chunk>>,
    free: Receiver<Chunk>,
    /// A chunk taken from the pool that a read left empty, kept for the next read rather than
    /// sent through the parser for nothing.
    spare: Option<Chunk>,
    join: Option<JoinHandle<ParseOutcome>>,
}

impl ParserThread {
    pub(crate) fn spawn<U>(
        terminal: Arc<FairMutex<Term<U>>>,
        event_proxy: U,
        replay_barrier: ReplayBarrier,
        parser: ansi::Processor,
    ) -> Self
    where
        U: EventListener + Send + 'static,
    {
        let (filled_tx, filled_rx) = mpsc::sync_channel(CHUNK_COUNT);
        let (free_tx, free_rx) = mpsc::sync_channel(CHUNK_COUNT);
        for _ in 0..CHUNK_COUNT {
            free_tx.send(Chunk::new()).expect("the pool's receiver is still here");
        }
        let worker = Worker {
            outcome: ParseOutcome {
                parser,
                #[cfg(windows)]
                parse_calls: 0,
                #[cfg(windows)]
                parse_time: Duration::ZERO,
            },
            filled: filled_rx,
            free: free_tx,
            terminal,
            event_proxy,
            replay_barrier,
        };
        let join = thread::spawn_named("PTY parser", move || worker.run());
        Self { filled: Some(filled_tx), free: free_rx, spare: None, join: Some(join) }
    }

    /// An empty chunk to read into, waiting for the parser to finish one if all of them are
    /// queued.
    pub(crate) fn take_chunk(&mut self) -> io::Result<Chunk> {
        if let Some(chunk) = self.spare.take() {
            return Ok(chunk);
        }
        self.free.recv().map_err(|_| parser_stopped())
    }

    /// Queues a chunk for parsing. An empty one is kept for the next read instead.
    pub(crate) fn parse(&mut self, chunk: Chunk) -> io::Result<()> {
        if chunk.is_empty() {
            self.spare = Some(chunk);
            return Ok(());
        }
        let filled = self.filled.as_ref().ok_or_else(parser_stopped)?;
        filled.send(chunk).map_err(|_| parser_stopped())
    }

    /// Waits until everything queued has been parsed, and ends the thread. Whatever the PTY
    /// produced before this call is on the grid when it returns; `None` when it already ran.
    pub(crate) fn finish(&mut self) -> Option<ParseOutcome> {
        drop(self.filled.take());
        let join = self.join.take()?;
        match join.join() {
            Ok(outcome) => Some(outcome),
            Err(_) => {
                log::error!("the PTY parser panicked");
                None
            },
        }
    }
}

impl Drop for ParserThread {
    fn drop(&mut self) {
        self.finish();
    }
}

fn parser_stopped() -> io::Error {
    io::Error::other("the PTY parser stopped")
}

struct Worker<U: EventListener> {
    outcome: ParseOutcome,
    filled: Receiver<Chunk>,
    free: SyncSender<Chunk>,
    terminal: Arc<FairMutex<Term<U>>>,
    event_proxy: U,
    replay_barrier: ReplayBarrier,
}

impl<U: EventListener> Worker<U> {
    fn run(mut self) -> ParseOutcome {
        while let Some(chunk) = self.next_chunk() {
            self.parse_batch(chunk);
        }
        self.outcome
    }

    /// The next chunk to parse, ending a synchronized update whose timeout passes while waiting
    /// for it. `None` once the event loop has finished and everything it sent has been parsed.
    fn next_chunk(&mut self) -> Option<Chunk> {
        loop {
            let deadline = self.outcome.parser.sync_timeout().sync_timeout();
            let Some(deadline) = deadline else {
                return self.filled.recv().ok();
            };
            match self.filled.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(chunk) => return Some(chunk),
                Err(RecvTimeoutError::Timeout) => {
                    self.outcome.parser.stop_sync(&mut *self.terminal.lock());
                    self.event_proxy.send_event(Event::Wakeup);
                },
                Err(RecvTimeoutError::Disconnected) => return None,
            }
        }
    }

    /// Parses `first` and whatever else is already queued, up to [`MAX_LOCKED_READ`] bytes, under
    /// one terminal lock.
    fn parse_batch(&mut self, first: Chunk) {
        // The barrier only closes before a reader starts, which the event loop waits out before
        // its first read; what remains is teardown aborting it, after which nothing more may
        // reach the grid.
        if !self.replay_barrier.wait() {
            self.recycle(first);
            return;
        }

        // Reserve the next terminal lock, as a reader that parsed for itself did.
        let lease = self.terminal.lease();
        let mut terminal = self.terminal.lock_unfair();
        let mut processed = 0;
        let mut chunk = Some(first);
        while let Some(current) = chunk {
            #[cfg(windows)]
            let parse_started = Instant::now();
            self.outcome.parser.advance(&mut *terminal, current.filled());
            #[cfg(windows)]
            {
                self.outcome.parse_time += parse_started.elapsed();
                self.outcome.parse_calls += 1;
            }
            processed += current.len;
            self.recycle(current);
            if processed >= MAX_LOCKED_READ {
                break;
            }
            chunk = self.filled.try_recv().ok();
        }

        // Release the terminal before notifying the UI. Event listeners are permitted to apply
        // backpressure to hidden terminals, which must never extend the live grid lock duration.
        drop(terminal);
        drop(lease);

        // Queue terminal redraw unless all processed bytes were synchronized.
        if self.outcome.parser.sync_bytes_count() < processed {
            self.event_proxy.send_event(Event::Wakeup);
        }
    }

    fn recycle(&self, mut chunk: Chunk) {
        chunk.len = 0;
        // The pool holds every chunk there is, so this never blocks. It fails only once the event
        // loop has stopped taking chunks, and then nobody needs this one.
        let _ = self.free.try_send(chunk);
    }
}
