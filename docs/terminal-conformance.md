# Imported terminal conformance tests

The `shitty` Git submodule pins the suite from
[SaltwaterC/shitty](https://github.com/SaltwaterC/shitty/tree/master/tst).
Zetta's adapter runs its Python assertions unchanged against the local
`crates/alacritty_terminal` and `crates/vte` forks used by Zetta. It runs
headlessly and needs no graphical session or Shitty executable.

```sh
git submodule update --init shitty
make test-shitty
```

Python 3.10 or newer and the repository's pinned Rust toolchain are required.
On Windows the Makefile runs `python` rather than `python3`, which there is the
Microsoft Store shim; override it with `make test-shitty PYTHON=py` if needed.
The runner builds `tools/shitty-adapter` with its checked-in lockfile into
`target/shitty-adapter`. This is a separate Cargo workspace so it avoids
compiling the GUI and doesn't add a test protocol to the production binary.
Use `PYTHON` and `CARGO` to select their executables.

## Selecting tests and keeping reports

The default selection covers cursor movement, editing, parser streaming,
attributes, tabs, reset, shell integration, terminal protocols, and resize.
It also replays every `tst/xtermjs/*.in` recording, including Bash, Zsh, and Vim,
against the corresponding `.text` reference. These use the upstream adapter's
80-column, 25-row geometry and LF expansion to match default PTY output
processing. Recordings named in `tst/xtermjs/xfail.txt` run as expected
failures: each records an xterm.js behaviour the file explains is contrary to
the standards, so Zetta failing to match it is the expected outcome. One that
matches anyway passes, with a note in the report, rather than counting as an
unexpected success.

```sh
# Save failures, skips, and the imported revision as JSON.
make test-shitty SHITTY_ARGS='--report artifacts/shitty.json'

# Select modules or individual unittest names. Repeated --module flags combine.
python3 scripts/shitty/run.py --module test_cursor --module test_editing
python3 scripts/shitty/run.py --module test_cursor.CursorAndMovementTest.test_carriage_return_backspace_and_tab

# Select a recording, or list available modules and recordings.
python3 scripts/shitty/run.py --module xtermjs/t0500-bash_long_line
python3 scripts/shitty/run.py --list

# Discover all top-level tst/test_*.py modules, plus all xterm.js recordings.
python3 scripts/shitty/run.py --all --report artifacts/shitty-all.json

# Reuse a binary you built yourself; the runner will not rebuild this path.
python3 scripts/shitty/run.py --binary target/shitty-adapter/debug/zetta-shitty-adapter
```

The bridge bounds each dimension to 4096 cells and a page to one million cells;
it also caps retained history at 100,000 lines. Oversized geometry requests
return adapter errors. These limits can affect extreme-size imported cases and
should be distinguished from an engine failure when reviewing the report.

Adapter replies have a ten-second timeout; use `--timeout` to adjust it.
A timed-out adapter is terminated and recorded as an error.
Every runner option also has a short form, shown by `--help`. The default
invocation always builds first, so source changes cannot silently exercise a
stale adapter. On Windows the adapter executable ends in `.exe`.

The exit code is nonzero for assertion failures, errors, unexpected successes,
or a selection that only skips tests. JSON reports include individual failure
tracebacks and unified diffs for recording mismatches. Unittest subtests have
separate outcome records, so counts of failures or skips can exceed the number
of parent test methods.

Upstream Python `expectedFailure` annotations mark Shitty's own gaps, not
Zetta's, so the runner turns them back into ordinary tests. The exception is
listed in `RETAINED_EXPECTED_FAILURES` in `run.py`: an xterm.js reflow behaviour
that only appears when xterm.js is configured for it. Tests that assert the
exact identity Shitty advertises (its DA1 and DA2 replies, product and version
strings) skip, because Zetta reports the device attributes it implements
instead: a VT420-class DA1 with no sixel, printer or user-defined keys.

## What this exercises

Writes pass through the real local VTE parser and terminal grid. Observations
include screen cells, basic attributes and colors, cursor position, scrollback
viewport, resizing, hyperlinks, and terminal replies emitted as `PtyWrite`
events. Stored tab markers used for copying are exposed as blank visual cells.
Replies requiring frontend color or clipboard services are reported as
unsupported. Pixel-size reports (`CSI 14 t` and friends) follow the harness's
own headless convention of one-pixel glyphs and no border. Each cell also
reports its row's DEC line size (`ESC # 3` to `ESC # 6`) as `line_attribute`. Color assertions use the harness's pinned VGA palette
and the engine's dynamic color overrides; they don't read the user's theme or configuration.

This detects differences in handling the terminal output used by interactive
shells, such as cursor movement, wrapping, erase, scrolling, alternate screens,
and redraw after resize. A failure is evidence of a difference from the imported
assertion, and may represent an unsupported protocol or a different terminal
policy rather than a Zetta regression.

Two assertions cover a policy Zetta deliberately differs on. They are listed
in `DELIBERATE_DIFFERENCES` in `run.py` and run as expected failures: the
reason is noted in the JSON report, and a change of behaviour shows up as an
unexpected success.

- `test_osc.OscProtocolTest.test_hyperlink_control_preserves_arbitrary_uri_bytes`
  expects an OSC 8 URI's invalid UTF-8 to be kept as raw bytes. Zetta
  percent-encodes it, because the URI is later handed to a browser or file
  opener as a string.
- `test_notifications.NotificationProtocolTest.test_capability_reply_declares_only_implemented_payloads`
  (run by `--all`) expects OSC 99 queries to offer `p=close`. Zetta shows
  notifications through `zntfy`, which cannot withdraw one already shown, so
  its reply offers only `title`, `body` and `?`. An unfinished notification
  can still be closed.

Tests that read the actions a notification asks for skip, since the adapter
does not model a frontend.

This adapter does not launch Zetta's GUI or a real shell, test the production
PTY/event loop, or route GPUI keyboard events. It replays shell recordings.
Shitty frontend configuration, renderer/font/GPU checks, synthetic PTY fault
injection, and engine internals without a corresponding observation are
explicit skips. In particular, per-cell OSC 133 semantic regions are not
exposed by this core, so the imported shell-integration assertions skip.
Zetta's own shell integration is covered by its existing native tests.
Product/version assertions specific to Shitty also skip.

`--all` reports modules that do not import `harness.Shitty` as unsupported without
executing their Shitty build or packaging tests. The C embedding-example and
executable-options modules also skip because they invoke Shitty binaries directly.
Other imported suites with separate build adapters under `tst/` are outside this Python runner. A missing
Python dependency or an adapter crash is an error rather than a passing test.
Neither the submodule's assertions nor production terminal behavior are changed.

## Validating the adapter

```sh
cargo build --locked --manifest-path tools/shitty-adapter/Cargo.toml --target-dir target/shitty-adapter
python3 -m unittest discover -s scripts/shitty -p 'test_*.py' -v
cargo clippy --locked --manifest-path tools/shitty-adapter/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path tools/shitty-adapter/Cargo.toml --check
```

The bridge checks cover fragmented UTF-8 and escapes, replies drained once,
resize rejection and scrollback, explicit skips, a recorded Bash redraw,
tab snapshot normalization, adapter timeout cleanup, and failure propagation
through the runner and JSON report. A deliberately incorrect screen expectation ensures that failure reporting cannot pass
vacuously if the bridge stops exercising the engine.
