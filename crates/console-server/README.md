# Windows console server

Applications embed the `console_server::Session` library and implement
`ConsoleOutput`. A session launches a child in its own console, serves requests
on a thread, accepts keyboard/paste input, and joins its owned threads on close.
It starts no OpenConsole or separate console-server process.

Console-server is Zetta's default Windows console backend. Select ConPTY with
`"windows_console_backend": "conpty"` at the top level or on an individual profile.
The library has no GPUI, Zed, or terminal-engine dependency when built with
`default-features = false`; the default `harness` feature builds the standalone
parser/transport probe.

Output hooks carry semantic newline/wrap modes with unchanged bytes.
`flush` is a parser-completion fence for screen queries and title changes, and
`screen`/`read_cells` let the server answer them from the embedder's grid; the
`grid` feature provides both for an `alacritty_terminal::Term`. Blocking output
consumers must provide a cancellation handle that wakes both write and flush
without acquiring their output lock. Input accepts the terminal's stream:
win32-input-mode keys (`CSI Vk;Sc;Uc;Kd;Cs;Rc_`), VT keys, SGR mouse and focus
reports, and text.

What OpenConsole does, and where it lives here:

| Module | Owns |
| --- | --- |
| `server.rs`, `api.rs` | ConDrv dispatch: connections, objects, the API table |
| `write.rs`, `legacy.rs`, `codepage.rs` | Output: VT passthrough, VT-off semantics, code pages |
| `screen.rs`, `buffers.rs`, `cells.rs` | Screen info, cursor, attributes; cell reads/writes, fills, scrolling, extra screen buffers |
| `modes.rs`, `state.rs`, `info.rs` | Modes, code pages and the fixed answers a ConPTY console gives |
| `input.rs`, `keys.rs`, `read.rs` | The input queue, terminal input decoding, raw and cooked reads |
| `cooked.rs`, `history.rs`, `alias.rs`, `doskey.rs` | cmd's line editor, command history, doskey aliases and their APIs |
| `title.rs`, `signals.rs`, `window.rs`, `control.rs` | Titles, control events, the pseudo window |
| `session.rs`, `native.rs` | Launch, the child's job, and close with CTRL_CLOSE_EVENT |

See [the plan](../../docs/windows-console-server-plan.md) for measurements and
the verified compatibility matrix. Unicode and VT output pass through as UTF-8;
other code pages are converted. The undocumented driver ABI is verified on
Windows 11 x64.

Run from a Visual Studio developer shell with the pinned Rust toolchain:

~~~powershell
cargo test --manifest-path crates/console-server/Cargo.toml
cargo clippy --manifest-path crates/console-server/Cargo.toml --all-targets -- -D warnings
cargo build --manifest-path crates/console-server/Cargo.toml --release
.\crates\console-server\target\release\console-server.exe --command 'cmd.exe /d /c echo hello'
.\crates\console-server\target\release\console-server.exe --ctrl-c --report artifacts/condrv-control.json
~~~

The probe uses an 80x24 grid and 500 history lines by default. `--drain`
discards output; `--capture` captures the unchanged UTF-8 stream.
The watchdog terminates only the private child job. The server drains until
attached clients disconnect. Nonzero child exit and timeout fail the probe.
Native regression tests require Python on PATH and cover console writes,
ANSI/Unicode title round trips, nested clients, input, final drain, Ctrl+C,
and timeout cleanup.

`make conformance-windows` runs Microsoft's own console feature tests against
the server and fails if one that inbox ConPTY passes does not; see
[conformance](conformance/README.md).

For reproducible historical workload comparisons:

~~~powershell
python crates/console-server/scripts/benchmark.py --binary crates/console-server/target/release/console-server.exe --relay ../shitty/dev/compare_windows.py --payload "$env:TEMP/shitty-compare-windows/ascii.bin" --cat 'C:/Program Files/coreutils/bin/cat.exe' --runs 3 --history 500 --report artifacts/condrv-ascii-comparison.json
~~~

The harness measures headless transport/parsing, including startup and exit.
Add `--pipeline` to the benchmark script to measure the embedded parser-thread
overlap alongside the serial parser and drain. It does not measure GUI rendering.
Use the same release build and workload settings for comparisons.

This is an independent Cargo workspace. Windows modules and dependencies are
gated at their declarations. The wire declarations and native call sequences
derive from Microsoft's MIT-licensed Terminal/OpenConsole source, with the
permission notice retained in [LICENSE-MIT-Microsoft](LICENSE-MIT-Microsoft).
