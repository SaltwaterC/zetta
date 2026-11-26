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

Unicode and VT output pass through as UTF-8; other code pages are converted.
The undocumented driver ABI is verified on Windows 11 x64.

## Where it runs

- Local panes: inside Zetta (`crates/terminal/src/hosted_console.rs`).
- Session panes: in the multiplexer's console host
  (`crates/zmux/src/hosted_console.rs`), which already outlives daemon
  upgrades for ConPTY. The host makes the output self-contained, as ConPTY's
  is (LF to CRLF where the console's mode says so, DECAWM for wrap changes,
  OSC for titles), so the daemon and its clients read the same byte pipes as
  for ConPTY, and keeps its own grid for screen queries. The host protocol's
  `Open` carries the backend, and a daemon refuses console-server panes on a
  host older than `CONSOLE_SERVER_HOST_PROTOCOL_VERSION` rather than letting
  it start ConPTY instead.
- WSL profiles: either backend, like any other console program.

## Design notes

- The server answers screen queries from the terminal's own grid after a
  parse fence, so there is one screen model, not two kept in step.
- Legacy calls are translated to VT the way OpenConsole's VtIo writer does it
  for ConPTY: SGR for attributes, CUP, DECTCEM, and cell writes inside
  DECSC/DECRC. Created screen buffers are kept as cells and drawn on the
  alternate screen when activated.
- Input follows OpenConsole's InputStateMachineEngine: win32 keys pass
  through, VT keys and SGR mouse reports become records, text becomes key
  presses, and VT input mode hands the application VT.
- The cooked-line editor, history and aliases are ports of
  `readDataCooked.cpp`, `history.cpp` and `alias.cpp`.
- The child runs in a kill-on-close job that its own children break away
  from, so a crash of the host never leaves the shell behind, but a program
  started from the shell is not tied to the pane.

## Compatibility

Each case below was run through the same terminal engine under ConPTY and
under console-server, with the final screens compared cell for cell
(scripted keys in win32-input-mode where the case types):

- cmd: startup, `cls`, `color`, `chcp`, `timeout`, `set /p`, Ctrl+C on a
  partial line, Up-arrow history, F7 history popup, Left/Right/Backspace/Home/
  End editing, Esc, Tab completion, `doskey` macros.
- PowerShell 5.1 and 7: interactive PSReadLine sessions, history recall,
  `Write-Host -ForegroundColor`, `Clear-Host`, `[Console]::ReadKey` for keys and
  pasted text.
- Python: `input()`, Ctrl+Z EOF, Ctrl+C and Ctrl+Break handlers,
  `ReadConsoleInput` key records, VT-input-mode reads, the cell APIs
  (Fill/Read/WriteConsoleOutput*, ScrollConsoleScreenBuffer), alternate screen
  buffers, `GetConsoleWindow`.
- Interactive WSL bash.
- A probe of console queries (modes, code pages, fonts, cursor, buffer info,
  palette, titles, process list, history settings) and mode changes, compared
  value for value.

The two values that still differ are ConPTY's own: the largest window size,
which OpenConsole derives from the desktop monitor while a pane reports its own
size, and `GetConsoleHistoryInfo`'s flags, which OpenConsole returns
uninitialized (`ServerGetConsoleHistory` never sets `dwFlags`).

Native tests cover what a screen comparison cannot: CTRL_CLOSE_EVENT on close
(which must not block), programs started from the console outliving it,
`GenerateConsoleCtrlEvent`, and the pseudo window.

## Building and testing

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

## Measurements

All on Windows 11 x64 (AMD Ryzen 7 PRO 5850U), release builds, 80x24 grid,
500 history lines, shitty's `compare_windows.py` relay running `cat`. Rounds
were interleaved; each figure is the best of three.

Through Zetta's window, with rendering (2026-10-14):

| Payload | ConPTY (OpenConsole 1.24) | console-server | Gain |
| --- | ---: | ---: | ---: |
| ASCII, 954 MiB | 4.44 s, 215 MiB/s | 1.42 s, 672 MiB/s | 3.1x |
| Random, 95 MiB | 4.80 s, 20 MiB/s | 0.89 s, 107 MiB/s | 5.4x |

Headless, through the same event loop and parser thread with only the host
swapped: ASCII 218 vs 656 MiB/s, random 20 vs 97 MiB/s. Random bytes are not
the same work under both: ConPTY converts them through the console code page
and re-encodes them, while console-server passes them to the parser as the
Unix PTY does. Output reaches Zetta's parser as the application wrote it,
instead of being parsed into OpenConsole's buffer and re-encoded, which is
where ConPTY's throughput went.

## Workspace and sources

This is an independent Cargo workspace. Windows modules and dependencies are
gated at their declarations. The wire declarations and native call sequences
derive from Microsoft's MIT-licensed Terminal/OpenConsole source, with the
permission notice retained in [LICENSE-MIT-Microsoft](LICENSE-MIT-Microsoft):
`dep/Console` layouts, `src/server` dispatch, `src/host` (getset, directio,
_stream, readDataCooked, history, alias, input),
`src/terminal/parser/InputStateMachineEngine.cpp` and
`src/interactivity/base/InteractivityFactory.cpp`.
