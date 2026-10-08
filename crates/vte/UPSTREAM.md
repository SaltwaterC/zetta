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
