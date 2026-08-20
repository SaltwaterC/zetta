//! What the multiplexer keeps of a pane it is reading.
//!
//! A pane nobody is showing still has to be *read* — otherwise its child blocks
//! on a full terminal buffer — and what is kept of what was read is what a
//! reattach shows. Keeping it is a choice: on a host where memory matters more
//! than scrollback, holding the running commands and the geometry is the point.
//!
//! What is kept is a *grid*, not the bytes. Bytes cannot work for a full-screen
//! program: its output repaints parts of a screen, so a bounded buffer of recent
//! bytes is a pile of fragments describing a screen that has been dropped to make
//! room for them. A reattached `htop` came back as pieces of itself over a blank
//! terminal, and the buffer's cut landed mid-escape-sequence for good measure. A
//! grid is what a terminal keeps for the same reason, and the screen it holds is
//! serialized on demand by [`alacritty_terminal::snapshot::ansi_snapshot`].

use serde::{Deserialize, Serialize};

#[cfg(feature = "scrollback-buffer")]
use alacritty_terminal::{
    event::VoidListener,
    grid::Dimensions,
    term::{Config, Term},
    vte::ansi::{Processor, StdSyncHandler},
};

/// A snapshot larger than this is refused rather than read, so a client cannot
/// make the daemon parse an unbounded buffer per pane.
pub const MAX_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;

/// The memory a pane's retained scrollback is allowed to cost, as a rough figure
/// the line budget below is derived from.
///
/// Kept as bytes because that is the shape of the question a host with little
/// memory asks. It is not a user-facing number — `--retention` takes a mode, not
/// a size — so deriving lines from it changes no setting.
pub const DEFAULT_RING_BYTES: usize = 256 * 1024;

/// Upper bound for the configured in-memory retention budget. The daemon owns
/// this allocation, so reject implausibly large values before a pane exists.
pub const MAX_RING_BYTES: usize = 64 * 1024 * 1024;

/// What a retained line is assumed to cost, turning the budget above into the
/// scrollback a grid keeps. A cell costs more than a byte, so this is
/// deliberately pessimistic rather than exact.
#[cfg(feature = "scrollback-buffer")]
const ASSUMED_LINE_BYTES: usize = 256;

/// How much of a retained grid a reattach is sent: the screen, and scrollback
/// above it. The window bounds what it keeps again on its own side.
#[cfg(feature = "scrollback-buffer")]
const SNAPSHOT_LINES: usize = 2_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Retention {
    /// Read and discard. Reattaching shows a cleared screen and whatever the
    /// running program redraws.
    None,
    /// Keep the pane's screen, and a bounded scrollback above it.
    Memory { bytes: usize },
}

impl Default for Retention {
    fn default() -> Self {
        Self::Memory {
            bytes: DEFAULT_RING_BYTES,
        }
    }
}

impl Retention {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "none" => Ok(Self::None),
            "memory" => {
                #[cfg(feature = "scrollback-buffer")]
                return Ok(Self::default());
                #[cfg(not(feature = "scrollback-buffer"))]
                anyhow::bail!(
                    "retention \"memory\" needs the scrollback-buffer feature, which this \
                     multiplexer was built without"
                )
            }
            "persist" => anyhow::bail!(
                "retention \"persist\" needs the session-persistence feature, which this \
                 multiplexer was built without"
            ),
            unknown => anyhow::bail!(
                "unknown retention {unknown:?}; expected \"none\", \"memory\" or \"persist\""
            ),
        }
    }

    /// Rejects a retention mode that this binary cannot actually provide.
    ///
    /// Keeping this separate from [`new_retained`] is intentional: the latter
    /// is used by the daemon's hot path, while configuration errors need to be
    /// reported before a daemon starts holding any terminals. In particular,
    /// a memory request must never silently turn into `none` just because a
    /// constrained build omitted the scrollback implementation.
    pub fn validate(&self) -> anyhow::Result<()> {
        if let Self::Memory { bytes } = self {
            anyhow::ensure!(
                (4 * 1024..=MAX_RING_BYTES).contains(bytes),
                "memory retention must be between 4096 and {MAX_RING_BYTES} bytes"
            );
        }
        #[cfg(not(feature = "scrollback-buffer"))]
        if matches!(self, Self::Memory { .. }) {
            anyhow::bail!(
                "retention \"memory\" needs the scrollback-buffer feature, which this \
                 multiplexer was built without"
            );
        }
        Ok(())
    }

    /// The stable configuration spelling used when starting a daemon.
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Memory { .. } => "memory",
        }
    }

    /// Whether a client should serialize and send its current screen during a
    /// handover. The daemon still has to receive the handover request itself,
    /// but `none` must not make the application perform snapshot work that it
    /// immediately discards.
    pub const fn keeps_snapshot(self) -> bool {
        matches!(self, Self::Memory { .. })
    }

    /// A pane's retained screen, at the size that pane is running at.
    pub fn new_retained(&self, columns: u16, lines: u16) -> Retained {
        let _ = (columns, lines);
        match self {
            Self::None => Retained::Discarded,
            #[cfg(feature = "scrollback-buffer")]
            Self::Memory { bytes } => Retained::Screen(Box::new(Screen::new(
                columns,
                lines,
                bytes / ASSUMED_LINE_BYTES,
            ))),
            // Without the buffer compiled in there is nothing to keep it in.
            #[cfg(not(feature = "scrollback-buffer"))]
            Self::Memory { .. } => Retained::Discarded,
        }
    }
}

/// One pane's retained screen.
pub enum Retained {
    Discarded,
    #[cfg(feature = "scrollback-buffer")]
    Screen(Box<Screen>),
}

impl Retained {
    /// Feeds the pane's output to the retained screen.
    pub fn push(&mut self, chunk: &[u8]) {
        let _ = chunk;
        match self {
            Self::Discarded => {}
            #[cfg(feature = "scrollback-buffer")]
            Self::Screen(screen) => screen.advance(chunk),
        }
    }

    /// Replays a screen a client handed over into this one.
    ///
    /// A client's snapshot is escape sequences, so seeding is simply reading
    /// them: from here on the multiplexer's grid holds what that window was
    /// showing, and everything the pane prints next lands on top of it. That is
    /// what makes a later reattach show the session rather than the fragments of
    /// it that happen to have arrived since.
    pub fn seed(&mut self, snapshot: Vec<u8>) {
        let _ = snapshot;
        match self {
            Self::Discarded => {}
            #[cfg(feature = "scrollback-buffer")]
            Self::Screen(screen) => {
                screen.reset();
                screen.advance(&snapshot);
            }
        }
    }

    /// The screen as escape sequences, for a client about to show this pane.
    pub fn snapshot(&self) -> Vec<u8> {
        match self {
            Self::Discarded => Vec::new(),
            #[cfg(feature = "scrollback-buffer")]
            Self::Screen(screen) => screen.snapshot(),
        }
    }

    /// Forgets what is retained, for a pane whose terminal has gone to a client
    /// that reads it directly. Nothing here describes that pane any more, and a
    /// stale screen must not be served as though it did.
    pub fn clear(&mut self) {
        match self {
            Self::Discarded => {}
            #[cfg(feature = "scrollback-buffer")]
            Self::Screen(screen) => screen.reset(),
        }
    }

    /// Follows the pane's size, so the retained screen wraps where the window
    /// showing it wraps.
    pub fn resize(&mut self, columns: u16, lines: u16) {
        let _ = (columns, lines);
        match self {
            Self::Discarded => {}
            #[cfg(feature = "scrollback-buffer")]
            Self::Screen(screen) => screen.resize(columns, lines),
        }
    }
}

/// A pane's screen as the multiplexer keeps it: an off-screen terminal, fed the
/// same bytes the pane produces.
#[cfg(feature = "scrollback-buffer")]
pub struct Screen {
    term: Term<VoidListener>,
    parser: Processor<StdSyncHandler>,
    history: usize,
}

#[cfg(feature = "scrollback-buffer")]
impl Screen {
    fn new(columns: u16, lines: u16, history: usize) -> Self {
        let size = GridSize::new(columns, lines);
        Self {
            term: Term::new(Self::config(history), &size, VoidListener),
            parser: Processor::new(),
            history,
        }
    }

    fn config(history: usize) -> Config {
        Config {
            scrolling_history: history,
            ..Config::default()
        }
    }

    fn advance(&mut self, chunk: &[u8]) {
        self.parser.advance(&mut self.term, chunk);
    }

    fn snapshot(&self) -> Vec<u8> {
        alacritty_terminal::snapshot::ansi_snapshot(&self.term, SNAPSHOT_LINES)
    }

    fn resize(&mut self, columns: u16, lines: u16) {
        self.term.resize(GridSize::new(columns, lines));
    }

    /// Starts again from a blank screen of the same size, which is what both
    /// seeding and forgetting need: a parser part-way through a sequence, or a
    /// grid holding what somebody else was showing, would otherwise leak into
    /// the next thing this pane shows.
    fn reset(&mut self) {
        let size = GridSize {
            columns: self.term.columns(),
            screen_lines: self.term.screen_lines(),
        };
        self.term = Term::new(Self::config(self.history), &size, VoidListener);
        self.parser = Processor::new();
    }
}

/// A pane's size, in the shape a grid asks for it.
#[cfg(feature = "scrollback-buffer")]
struct GridSize {
    columns: usize,
    screen_lines: usize,
}

#[cfg(feature = "scrollback-buffer")]
impl GridSize {
    fn new(columns: u16, lines: u16) -> Self {
        // A zero-sized grid is not a grid, and a pane's size is unknown until the
        // window showing it reports one.
        Self {
            columns: (columns as usize).max(1),
            screen_lines: (lines as usize).max(1),
        }
    }
}

#[cfg(feature = "scrollback-buffer")]
impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }

    fn screen_lines(&self) -> usize {
        self.screen_lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

#[cfg(test)]
#[path = "tests/retention.rs"]
mod tests;
