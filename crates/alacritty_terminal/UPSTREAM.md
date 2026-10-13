# Zetta Alacritty terminal fork

Upstream base: `zed-industries/alacritty@4c129667ce56611becdc82de6e28218c80e2e88f`.
That revision remains the current upstream `master` as of 2026-08-29.

Retain these Zetta changes when synchronizing:

- the inline Unix PTY tests continue reading output while waiting for a zsh
  foreground job or child exit. macOS can wait for terminal output to drain
  at either point, so polling only the process group or exit status deadlocks
  the test instead of exercising Ctrl-C handling.

- Rust 1.99 manifest and lint compatibility: let chains, `is_multiple_of`,
  and fixed-size slice chunks. `ShellUser::from_env` retains its matches with
  a documented `question_mark` expectation because Clippy's suggested rewrite
  moves the passwd result before later environment fallbacks can borrow it.
- `src/snapshot.rs`, a Zetta-authored module with no upstream counterpart:
  serializing a `Term`'s grid back into the escape sequences that would
  reproduce it. It lives here rather than in `crates/terminal` because both
  sides of a session need it — the window snapshots a screen when it hands a
  session over, and the multiplexer, which keeps a grid per pane it reads,
  snapshots one when it hands a session back. The only upstream file it touches
  is `lib.rs`, which declares the module; its tests live in
  `crates/terminal/src/tests/snapshot.rs`, where the harness for building a
  terminal from bytes already is.
- hybrid scrollback storage using a small ring buffer and chunked archive.
  Sealing a uniform chunk keeps its spare rows (bounded, never copied by a
  snapshot) as the next rows scrolled in, and dropping the oldest row of a full
  history does not copy a row a uniform chunk shares: the grid resets every row
  it scrolls in, so only the allocation is reused. Without that, repeated
  output allocated, initialised, compared and freed a row per line;
- compact sealed history: `src/grid/archive.rs`, a Zetta-authored module (tests
  in `src/tests/grid/archive.rs`), encodes every sealed chunk that is not one
  row repeated as attribute runs plus UTF-8 text, about 90 bytes for an
  80-column line instead of 24 bytes a cell, and hands the chunk's rows back as
  the next rows scrolled in. Unlimited scrollback had grown about 3 GB per
  100 MiB of distinct output, and faulting that memory in dominated throughput.
  It touches upstream code in three places: `GridCell` gains three provided
  methods (`archive_char`, `same_archive_attributes`, `with_archive_char`;
  the default keeps a cell type's history as rows) which `Cell` implements;
  compact rows are decoded a chunk at a time into a cache that only a mutable
  borrow releases, so `&Row`/`&Cell` borrowed from the grid stay valid; and
  `Grid::release_history_cache`, `Term::release_history_cache` and
  `Term::bounds_to_string_releasing_history` let a reader walking all of
  history on a snapshot keep one step of it decoded. `row_storage_id` of a
  compact row is an address inside its chunk's encoded bytes, never the
  decoded row's, so it stays stable when the cache is released. Serde writes
  compact chunks out as rows;
- rows read as text: `src/grid/text.rs`, a Zetta-authored module (tests in
  `src/tests/grid/text.rs`), gives `Grid<Cell>::row_text`/`row_wraps` and
  `RowText`, which read a compact row's encoded runs directly instead of
  decoding its chunk into cells. Scrollback search over unique output spent
  three quarters of its time decoding cells it then read only the characters
  of. It touches upstream code only by declaring the module in `grid/mod.rs`;
  `Storage::stored_row` and `CompactRows<Cell>::append_text`/`wraps` are in
  Zetta-authored code;
- scrollback allocator, large-history, and benchmark fixes;
- Windows ConPTY read-ahead and terminal-hangup handling. The output kernel
  pipe holds 128 KiB instead of the system default, while the existing bounded
  user-space pipe retains its capacity. `UnblockedReader` publishes available
  bytes immediately: waiting up to 2 ms for the entire read-ahead pipe to fill
  delayed reads that could consume only 65,535 bytes, and added a mutex and
  condition-variable notification to every source read. The source reader's
  pause/cancel protocol still protects exclusive-client handovers. Inline
  reader tests cover short reads, EOF, byte ordering, and fragmented bursts;
  `src/tests/event_loop_windows.rs` checks final output before the child-exit
  report is consumed and provides ignored optimized-build ConPTY benchmarks.
  `src/event_loop/windows.rs` drains the source queue across gather batches
  until the output pipe ends, or else for a bounded quiet period, before
  joining the parser. A single final read lost output when the child exited
  while parsing was blocked. The native PTY policy remains in
  `src/event_loop/direct.rs`;
- a terminal's own pseudoconsole is released after its child is spawned
  (`Conpty::release_reference`, `ReleasePseudoConsole` from conpty.dll or
  kernel32), as Windows Terminal does, so it exits with its last client and
  its output pipe ends. `conpty::new` closes its copies of the console's pipe
  ends after `CreatePseudoConsole` duplicates them; upstream leaked them, which
  alone kept the output pipe from ever ending. The source reader records an
  end it reached by itself (`UnblockedReader::source_ended`, including a
  `BrokenPipe`, which is no longer logged as an error) and wakes the event loop,
  and `EventedPty::output_ended` lets the final drain stop there instead of
  waiting out the quiet period. Exit-to-close fell from 0.18 to 0.15 seconds
  against Windows Terminal's 0.14. A resize that reaches an exited console is
  logged rather than asserted. The multiplexer host keeps its consoles
  referenced. `the_end_of_the_source_wakes_an_idle_reader`,
  `the_final_drain_stops_at_the_end_of_output_without_a_quiet_period` and
  `a_released_console_ends_its_output_when_the_child_exits` each fail with
  their part removed;
- shell integration, resize, and sequence handling needed by Zetta's PTY
  lifecycle;
- Unix PTY teardown reaps owned and reclaimed children, escalating from SIGHUP
  to SIGKILL after a short grace period. A shell that ignores hangup
  must not block the multiplexer session lock or application shutdown.
  Teardown discards the master's pending output while it waits, because on
  macOS a child — even a `SIGKILL`ed one — cannot finish exiting while its
  terminal output is unread, and the master only closes after the wait.
- Unix `Pty::try_wait` exposes a direct status poll for the multiplexer daemon;
  its `SIGCHLD` pipes remain wakeup mechanisms rather than prerequisites for
  calling `waitpid`, so a notification race cannot strand a pane's exit.
- attached PTYs (`tty::unix::attach`), where the master file descriptor is
  passed in by the `zmux` multiplexer and the child belongs to that process.
  Upstream's `Pty` assumes it spawned the child, so four things diverge and
  must survive a synchronization: `PtyChild` distinguishes an owned child from
  an attached one and from one *reclaimed* across the multiplexer's own
  `execv` (still this process's child, so still reaped here); `Drop` must not
  hang up or reap an attached child (detaching a session is exactly that drop);
  `next_child_event` reads the exit status from the multiplexer's socket because
  `waitpid` is only available to the real parent; and `EventedPty` gains
  `child_is_foreign`, which is how the event loop tells the two apart.
- the event loop's handling of a hung-up master, which follows from the above.
  Upstream loops back round for "the inevitable `Exited` event", and for a child
  it spawned that event really is inevitable. For a foreign child it is not: the
  only route is a report over the multiplexer's control channel, so a broken
  channel used to mean a pane that accepted no input, showed no exit and could
  not be closed — while spinning a core, because the poller is level-triggered.
  `hungup_too_long` bounds that wait for a foreign child only, and paces the
  poll; an owned child's path is byte-for-byte upstream's.
- `ChildEvent::WatcherDisconnected` must not call `Term::exit`. The two genuine
  exit events may, because the child really has ended; a lost watcher says
  nothing about the child, and `Term::exit` sends `Event::Exit`, which the
  consumer reads as "ended with no usable status". Sending it anyway overruled
  whatever the consumer had decided a disconnect meant.
- Windows: `tty::windows::attach` unblocks the duplicated console pipes through
  `conpty::PIPE_CAPACITY`, which is why that constant is `pub(super)`. It must
  not be re-declared beside `attach`; `piper::pipe` asserts a positive capacity,
  and a second constant that drifted to zero panicked every attached pane.
- The event loop accepts a `ReplayBarrier` supplied by Zetta. Attached PTY
  readers wait behind retained-screen replay until the first real layout, and
  an abort wakes them when a terminal is dropped or its backend is replaced.
- `EventedPty::redraw` and the event loop's `Redraw` message let an attached
  Unix PTY signal its foreground process after replay. Reapplying an unchanged
  PTY size does not emit `SIGWINCH`, but a differential TUI must still discard
  the screen it drew into the previous terminal emulator.
- The event loop reports a child exit only after its configured final PTY
  drain. Zetta releases an exited PTY as soon as it receives that report, so
  reporting first could abort the drain and discard the child process's final
  output.
- Unix PTY master reads and writes treat `EIO` as a slave hangup awaiting the
  child exit notification. A queued terminal reply can reach the master after
  the child closes its slave, before `SIGCHLD` is handled; reporting a backend
  failure then left comparison windows open on an error pane. The read and write
  paths retain the existing bounded wait for a foreign child's exit report.
  Only nonempty output clears that deadline: repeated EOF must not restart it.
  `eof_does_not_restart_a_foreign_childs_missing_exit_deadline` covers a hung-up
  slave whose watcher never reports the exit.
  `a_terminal_reply_after_slave_hangup_waits_for_the_child_exit` forces that
  ordering with real readiness and child notifications, and fails without the
  write-side handling.
- The event loop only does I/O; `src/pty_parser.rs`, a Zetta-authored module,
  parses on a thread of its own. Linux copies pty output out through a small
  stack buffer, and plain-text floods spent two fifths of the single reader
  thread in that copy with the parser idle. The loop reads into 64 KiB chunks
  from a pool of 16 (the old `READ_BUFFER_SIZE` bound) and blocks once all are
  queued; the parser thread takes the lease and lock per batch, sends
  `Wakeup`, and owns the synchronized-update timeout that the poll timeout
  used to carry, so `PeekableReceiver::peek` is gone. The resize-request and
  clipboard-frame scanners stay on the reading thread. Every exit, and the end
  of the loop, joins the parser first, so the final drain above still lands on
  the grid before the exit is reported and before `PtyIo::join` returns.
  `EventLoop::spawn` therefore needs a `Clone` listener. Regression test:
  `the_last_output_is_on_the_grid_when_the_exit_is_reported`; manual
  benchmarks `ascii_through_the_event_loop_throughput_benchmark` and
  `random_through_the_event_loop_throughput_benchmark` run `cat` through a
  real pty into an 80x24 grid.
- macOS: `src/pty_read/macos.rs` bridges the kernel's roughly 1 KiB PTY refill
  gaps with bounded nonblocking retries. Small bursts get sixteen retries;
  continuous bulk output gets up to 256, classified across chunks until a 1 ms
  quiet gap. A 3 ms per-batch deadline is checked every sixteen failed reads,
  without restarting it on refills. Empty reads, EOF and errors end immediately.
  Four 256 KiB gather buffers retain the 1 MiB read-ahead limit; the parser splits
  them into the existing 65,535-byte lock budget before recycling them. One-shot
  kqueue readiness is rearmed after each poll, removing per-write filter work
  while gathering. Other platforms retain their original gather size, pool and
  level-triggered readiness via `src/pty_read/direct.rs`. Escape scanners use
  `memchr`'s SIMD search. Tests cover burst classification, retry/deadline bounds,
  rearming after idle, final-output drain and parse boundaries.
- Windows: `src/tty/program_search.rs`, a Zetta-authored module (tests in
  `src/tests/tty/program_search.rs`), resolves a pane's program the way
  `CreateProcessW` would but never from the current directory or a relative
  `PATH` entry. `conpty` passes the result as `lpApplicationName`, and
  `cmdline` quotes the program and spells it as that absolute path, because
  the palette bootstrap re-launches the line from the pane's directory, where a
  planted `cmd.exe` would otherwise win. `zmux`'s bootstrap calls
  `tty::resolve_application` and `program_search` too, so both ends share one
  policy.
- `Term` implements `Handler::input_ascii` (see `crates/vte/UPSTREAM.md`),
  writing a run of text a row segment at a time. Insert mode, a line-drawing
  charset, disabled autowrap, and overwriting half of a wide character take
  upstream's per-character path; `input_ascii_matches_input_per_character`
  pins the result against it. `binary_output_throughput_benchmark` is the
  matching manual benchmark.
- Writing a line onto a row that has just scrolled in costs no more than
  writing it. A segment that starts at or past the row's `occ`, which upstream
  already keeps as the bound past which every cell is blank, is written field
  by field without reading the cells first: no wide-character scan, no `extra`
  to drop. If the uniform blank suffix already has the cursor's attributes,
  writing only its characters avoids redundant attribute stores. `GridCell`
  gains a provided `reset_all`, which `Row::reset`
  calls; `Cell` overrides it to copy one blank cell whole, two wide stores in
  place of five narrow ones. Both loops were store-bound, not waiting on the
  recycled row's memory; prefetching that row measured slower.
  `ascii_output_throughput_benchmark` (parse only) went from about 390 to
  610 MiB/s; `input_ascii_onto_recycled_rows_matches_a_fresh_terminal` and
  `resetting_cells_together_matches_resetting_each` pin the results.
- Windows source-reader cancellation is retried until the reader finishes.
  A single `CancelIoEx` could return `ERROR_NOT_FOUND` just before the source
  entered `ReadFile`, leaving a paused handover blocked until the child wrote
  again. The completion condition variable is notified under its mutex;
  normal reads need only the stop flag. A delayed native-read regression
  reproduces the race and releases the broken implementation before failing.
- `Row` remembers when only its characters have changed since a complete
  reset. `write_ascii_characters` preserves that proof; every accessor that
  exposes mutable cells invalidates it. With the same erase background,
  `Cell::reset_characters` clears characters without rewriting unchanged
  attributes. Partial resets of imported rows do not establish the proof,
  and serialization omits it. Alternating optimized Windows parser runs
  improved from 493â€“495 to 739â€“747 MiB/s. Row tests cover all mutable accessors,
  replacement characters, changed backgrounds and imported row suffixes.
- `Storage::shrink_lines` hands every row it removes to the recycled-row pool,
  past `RECYCLED_ROWS_LIMIT` (`RecycledRows::keep_all`): they are rows the
  storage held a moment before, so a pane never holds more rows than it did. A
  full reset (`ESC c`) clears history through it, and binary output carries one
  every 64 KiB or so. With the pool capped at 256 rows, a 500-line history freed
  some 250 rows at each reset and allocated them again as output refilled it;
  keeping them made the parse-only random benchmark about 9% faster (from about
  169 to 185 MiB/s). Pinned by `clearing_history_keeps_every_row_it_removes`.
- `vte` is a path dependency on Zetta's fork in `crates/vte`, which bounds the
  OSC buffer that crates.io vte 0.15.0 grows without limit under `std`. See
  `crates/vte/UPSTREAM.md`.

The eight Zetta commits carrying these changes are `d6aa84b`, `d7b896f`,
`57ecffe`, `d83beb7`, `1f6b1f7`, `9de38c6`, `31c3303`, and `7ba5a85`.

## Zetta patch: physical cell geometry

CSI 16 t uses the existing size-request event to report character cell height and width (CSI 6;HEIGHT;WIDTH t).
