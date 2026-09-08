# vt100

Forked from the crates.io vt100 0.16.2 source (MIT), upstream
https://github.com/doy/vt100-rust. Source, license and package documentation
are preserved; crates.io packaging metadata is omitted.
Upstream's development dependencies are omitted because their integration
tests are not included in the published crate; the local regression tests
require no additional dependencies.

Zetta adds CSI Ps b (REP), repeating the preceding printed character through
the ordinary text path. The remembered character belongs to Screen so Mosh
protocol-state clones retain it; RIS resets it. This preserves repeated padding
and cursor columns in Mosh updates. Root, zosh and the standalone Rust server
patch crates.io to this fork so their parser behavior stays consistent.

Regression tests are inline in perform.rs, plus the protocol/diff test in
crates/zosh/src/tests/display.rs.

The htop reproduction is server-side: forwarding macOS LC_CTYPE=UTF-8 to
a Linux host without that locale makes ncurses use ASCII output with REP
padding. An unpatched Rust server drops those repeats before the client
receives the screen. Updating only the client cannot recover the lost cells.
The server regression in terminal_state.rs covers the observed sequence.
