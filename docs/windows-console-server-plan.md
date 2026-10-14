# Windows console server

`crates/console-server` replaces ConPTY's OpenConsole. It opens a private
ConDrv server, launches the child onto it with the pseudoconsole process
attribute, and serves the console API itself. Output reaches Zetta's parser as
the application wrote it, instead of being parsed into OpenConsole's buffer
and re-encoded, which is where ConPTY's throughput went.

Profiles opt in with `"windows_console_backend": "console-server"`; ConPTY
remains the default.

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
Unix PTY does.

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

## Where it runs

- Local panes: inside Zetta (`crates/terminal/src/hosted_console.rs`).
- Session panes: in the multiplexer's console host
  (`crates/zmux/src/hosted_console.rs`), which already outlives daemon
  upgrades for ConPTY. The host makes the output self-contained, as ConPTY's
  is (LF to CRLF where the console's mode says so, DECAWM for wrap changes,
  OSC for titles), so the daemon and its clients read the same byte pipes as
  for ConPTY, and keeps its own grid for screen queries. Host protocol 2 adds
  the backend to `Open`; a daemon refuses console-server panes on an older
  host rather than letting it start ConPTY instead.
- WSL profiles: either, like any other console program.

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

## Primary references

Microsoft's MIT-licensed Terminal source (the permission notice is retained
in the crate): `dep/Console` layouts, `src/server` dispatch, `src/host`
(getset, directio, _stream, readDataCooked, history, alias, input),
`src/terminal/parser/InputStateMachineEngine.cpp` and
`src/interactivity/base/InteractivityFactory.cpp`.
