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

Zetta also adds an opt-in eviction queue: `Screen::set_capture_evicted_rows`
keeps the rows that scroll off the top of the *normal* screen so a caller can
put them in a history of its own, `Screen::evicted_rows_total` names a position
in the session's output that only ever grows, and `Screen::take_evicted_rows`
collects them as self-contained `EvictedRow`s;
`Screen::visible_rows_as_evicted` renders what is still on screen the same way,
so the two can be compared. Upstream's `scrollback` is a
scrollable *view* and is bounded by `scrollback_len`; this is a queue that is
drained, which is what `zosh-server` needs to carry history to a client whose
Mosh state only ever describes one screen. It is off by default, so nothing
that does not ask for it pays for it, and a scroll region or an alternate
screen deliberately evicts nothing.

`rustfmt.toml` pins upstream's own `max_width`. Without it the workspace
default reformats every file the fork tracks, which buries these changes in
merge friction; it is a no-op against the unmodified source.

Regression tests are inline in perform.rs and screen.rs, plus the protocol/diff
test in crates/zosh/src/tests/display.rs.

The htop reproduction is server-side: forwarding macOS LC_CTYPE=UTF-8 to
a Linux host without that locale makes ncurses use ASCII output with REP
padding. An unpatched Rust server drops those repeats before the client
receives the screen. Updating only the client cannot recover the lost cells.
The server regression in terminal_state.rs covers the observed sequence.
