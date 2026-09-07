#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! mosh-rs: a wire-compatible Rust client for mosh's State
//! Synchronization Protocol, interoperating with the stock C++
//! `mosh-server` (`MOSH_PROTOCOL_VERSION = 2`).
//!
//! The layers land bottom-up, each validated against a real
//! `mosh-server` before the next begins:
//!
//! 1. `key` + `crypto`  : the session key and the AES-128-OCB3
//!    datagram layer.
//! 2. `packet`          : sequence numbers, the direction bit, and the
//!    16-bit RTT timestamps carried inside every encrypted payload.
//! 3. `transport`       : fragmentation, zlib, protobuf instructions.
//! 4. `sender`          : the ack / retransmit / heartbeat timers.
//! 5. `statesync`       : the client's UserStream out, the host's
//!    terminal-state diffs in.
//! 6. `terminal`        : the client's own screen, painted by
//!    difference so a recomputed frame costs nothing.
//! 7. `prediction`      : predictive local echo. Guess what a keystroke
//!    will do, draw it at once, check it against the server later.
//! 8. `session`         : all of the above driven together, including
//!    the source-port rotation that is the whole of roaming.
//!
//! The library itself has no platform dependency and builds on
//! Windows; only the `mosh-rs` binary's terminal front end is
//! Unix-only.
//!
//! `SPEC.md` in the repo root records the wire details each layer
//! implements, extracted from the mosh C++ (which has no written
//! specification of its own).

pub mod crypto;
pub mod error;
pub mod key;
pub mod packet;
pub mod prediction;
pub mod screen;
pub mod sender;
pub mod session;
pub mod statesync;
pub mod terminal;
pub mod transport;

pub use crypto::{Direction, Session};
pub use error::{MoshError, Result};
pub use key::Base64Key;
pub use packet::{Packet, PacketState};
pub use prediction::{DisplayPreference, PredictionEngine};
pub use screen::{Cell, Color, DiffScreen, OverlayCell, Rendition, Screen};
pub use sender::{Received, TransportReceiver, TransportSender};
#[cfg(any(unix, windows))]
pub use session::SocketHandle;
pub use session::{LinkHealth, MoshSession};
pub use statesync::{HostEvent, UserStream};
pub use terminal::ClientTerminal;
pub use transport::{Fragment, FragmentAssembly, Fragmenter, Instruction};
