//! Embedded Windows console hosting, independent of any terminal UI.
//! The executable is a probe; applications embed Session and provide output hooks.
#[cfg(windows)]
mod alias;
#[cfg(windows)]
mod cells;
#[cfg(windows)]
pub mod codepage;
#[cfg(windows)]
mod connect;
#[cfg(windows)]
pub mod control;
#[cfg(windows)]
mod cooked;
#[cfg(windows)]
mod editing;
#[cfg(all(windows, feature = "grid"))]
pub mod grid;
#[cfg(windows)]
mod history;
#[cfg(windows)]
mod input;
#[cfg(windows)]
mod keys;
#[cfg(windows)]
mod launch_control;
#[cfg(windows)]
mod legacy;
#[cfg(windows)]
pub mod native;
#[cfg(windows)]
pub mod output;
#[cfg(windows)]
pub mod protocol;
#[cfg(windows)]
pub mod server;
#[cfg(windows)]
mod session;
#[cfg(windows)]
pub mod state;
#[cfg(windows)]
mod window;
#[cfg(windows)]
pub use output::{ConsoleOutput, OutputMode, Pen, PenColor, Screen};
#[cfg(windows)]
pub use session::{Launch, Session};
#[cfg(windows)]
pub use state::Palette;
#[cfg(all(windows, test, feature = "harness"))]
#[path = "sink.rs"]
mod sink;
