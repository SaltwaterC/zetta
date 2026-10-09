# vte

Forked from the crates.io vte 0.15.0 source (Apache-2.0 OR MIT), upstream
https://github.com/alacritty/vte at `3b3da71c34cc1256c7e20981cf03f8eb95e08ffc`.
Source, licenses, rustfmt configuration and package documentation are
preserved. `Cargo.toml` is upstream's `Cargo.toml.orig` plus `publish = false`;
crates.io packaging metadata, the example, the CI configuration and the
`doc/`/`tests/` fixtures (which no test reads) are omitted.

`alacritty_terminal` and `terminal` name this fork by path, and the root
manifest patches crates.io `vte` to it, so `vt100` and `mosh_rs` resolve to
the same copy in Zetta's own build. Their standalone workspaces (`crates/vt100`,
`crates/mosh_rs`, `crates/zosh`) still resolve crates.io vte.

## Zetta patch: bounded OSC accumulation

Upstream's `std` build collects an OSC into a `Vec` with no bound; the
1,024-byte `MAX_OSC_RAW` applies only to the `no_std` `ArrayVec`. Every byte a
pane prints between `ESC ]` and a terminator is therefore held, so an OSC that
never ends grows the parser until Zetta runs out of memory.

`action_osc_put` stops collecting at `MAX_OSC_RAW_STD` (8 MiB) and marks the
sequence overflowed; `osc_end` then discards it instead of dispatching it, and
parsing resumes normally after the terminator (BEL, ST, CAN or SUB, as
upstream). A truncated dispatch, which is what `no_std` does, would hand
`alacritty_terminal` a corrupted clipboard write or link, so the whole sequence
is dropped instead. The limit leaves room for the large OSC that is legitimate:
an OSC 52 clipboard write is base64, so 8 MiB carries about 6 MiB of copied
text. Titles, OSC 8 links, colors and zclip frames (at most 46 KiB) are far
smaller. The check is one comparison per OSC byte, the same cost as the
`no_std` path's `is_full`.

`osc_end` also shrinks the buffer to 64 KiB of capacity after a larger
sequence, so one large OSC 52 copy does not leave every parser that saw it
holding megabytes. `shrink_to` is a comparison when the buffer is smaller.

Regression tests are inline in `src/lib.rs`:
`osc_beyond_std_limit_is_discarded_through_its_terminator`,
`unterminated_osc_is_bounded` and `osc_at_std_limit_is_dispatched`.

## Zetta patch: ground-state text in runs

Upstream dispatches ground-state text with one `Perform::print` call per
character, and handles invalid UTF-8 by returning after each invalid sequence,
so the next call searches for the escape and validates the rest of the text
again. Output throughput spent most of its time there: a line of plain text
cost a call per character, and binary output, which has an invalid sequence
every couple of bytes, paid for the bytes between two escapes once per invalid
sequence among them.

- `Perform::print_ascii` and `ansi::Handler::input_ascii` take a run of
  printable ASCII in one call. `ASCII_REPLACEMENT` (DEL, which a printable run
  cannot otherwise contain) stands for U+FFFD in a run, and `ascii_run_char`
  decodes a byte. Both default to a call per character, so a performer that
  does not override them behaves exactly as upstream.
- `advance_ground` validates text with `str::from_utf8` once; on an error it
  decodes the rest in a single pass (`ground_dispatch_lossy`, `decode_utf8`),
  consuming exactly the bytes `Utf8Error::error_len` reports, so replacement
  characters and C1 executes come out as upstream produced them.
- `Perform::IGNORED_EXECUTES` (a `control_bits` set) lets that pass skip a
  control the performer does nothing for instead of ending the run at it.
  `ansi::Performer` names every control but the ones `execute` matches; those
  bytes then go unlogged at debug level when they arrive inside invalid text.
  It is an associated constant so the pass can build its 256-entry `LossyByte`
  table at compile time: random bytes defeated branch prediction on every byte,
  so printable text, bytes that never start a character, ignored controls and
  lead bytes whose successor rules out a character are table lookups without a
  branch on the byte. Only the rest reach `decode_utf8`. That doubled binary
  output parsing.

Regression tests: `invalid_utf8_dispatches_as_upstream_did` in `src/lib.rs`
compares the parser against upstream's algorithm on seeded binary noise, for a
performer that ignores no controls and one that ignores most, and
`ignored_controls_are_exactly_the_ones_execute_does_nothing_for` in
`src/ansi.rs` pins the two control lists together.
