//! The snapshot serializer's home is `alacritty_terminal`, beside the grid it
//! reads, because the multiplexer serializes its own grids with the same code —
//! see that module for what a snapshot contains and why.
//!
//! What stays here is the test harness: building a terminal by feeding it bytes
//! needs this crate's listener and configuration, so the contract is pinned
//! where that is cheap.

pub(crate) use alacritty_terminal::snapshot::ansi_snapshot;

#[cfg(test)]
use crate::alacritty::AlacrittyTerm;
#[cfg(test)]
use alacritty_terminal::{
    index::{Column, Line},
    snapshot::RENDERED_FLAGS,
    term::{
        TermMode,
        cell::{Flags, LineLength as _},
    },
    vte::ansi::{Color, NamedColor},
};

#[cfg(test)]
#[path = "tests/snapshot.rs"]
mod tests;
