# Zetta terminal fork

The source baseline is `zed/crates/terminal` at Zed revision
`2890c340e07a4c4c7e6778e99a49f5414115b250`. The local crate uses a
standalone manifest and is not a drop-in copy of Zed's workspace terminal.

Retain these Zetta-specific behaviors when synchronizing:

- allow scrollback up to Alacritty's signed line-coordinate range instead of
  Zed's 100,000-line product limit;
- expose PTY metadata, startup signaling, process tracking, and shell markers
  required by standalone profiles, WSL/MSYS2/PowerShell CWD tracking, pane
  output export, serial consoles, and tab titles;
- preserve immediate first-event processing with bounded PTY drains and add
  resize requests, Win32 input records, shell quoting, and Zetta environment
  identity;
- provide literal, incremental scrollback search and foreground-process
  refresh throttling. Queue only the live terminal handle on the foreground;
  capture a grid snapshot on the low-priority worker under a short grid lock,
  release it before scanning, and yield after capture and between scan chunks
  so obsolete jobs can be cancelled. The search engine is `alacritty/search.rs`
  (no upstream counterpart): it reads rows as text through the Alacritty fork's
  `Grid::row_text`, which never decodes compact history, matches ASCII queries
  with `memmem` and others with a Unicode smart-case regex, splits a large
  snapshot across parallel low-priority workers, and reports results
  (`SearchJob`, `SearchUpdate`) newest first while the exact count is still
  being taken. Each update carries only the matches found since the previous
  one; `Terminal::matches` is a `SearchRanges`, stored newest first so those
  append, and looked up by binary search for the lines on screen;
- capture and terminate both the shell and foreground process groups during
  PTY teardown, including application shutdown. Upstream reverted its own
  version of this in `492acd6c81`; do not import that revert. The regression it
  was reverting for came from reading a *stale* pty master descriptor, which
  `ProcessIdGetter::close` and the `child_process_ended` guard address directly;
- release the pty event loop when the child ends
  (`Terminal::release_pty_resources`), so an exited pane that stays open does
  not hold the pty master, the poller and the loop's buffers. Note that
  `PtyIo`'s `JoinHandle` is what owns them: the loop thread returns its
  `EventLoop` instead of dropping it;
- resolve path targets against the working directory that was current when the
  line was printed (`cwd_history`/`cwd_at_line`), rather than the one current
  when the click happens;
- allow Shift-drag to start selection while an application owns mouse
  tracking;
- diagnose terminal grid-lock and renderable-snapshot stalls without logging
  from the UI thread.
- export scrollback from a terminal cloned on the background worker under a
  short live-grid lock. Keep sealed history shared and traverse the snapshot
  only after releasing the lock used by rendering and PTY parsing, through
  `bounds_to_string_releasing_history` so compact history is decoded a step at
  a time;
- parse restored replay and normalize fresh-shell screens on a private grid
  after layout supplies geometry (`replay.rs`, no upstream counterpart).
  Apply subsequent resize requests and injected output on the worker before
  swapping grids, then release the existing reader barrier and fresh-shell
  prompt input. Parser events wait for publication before inspecting the grid;
  an empty non-fresh replay releases its reader immediately at layout;
- render snapshots retain selection coordinates only. Selection clipboard
  requests clone the grid at their event-stream position and serialize on a
  background worker. Application-wide clipboard versions reject stale results;
  terminal clipboard reads wait for pending copies. Empty selections retain
  primary ownership, including after copy-and-clear;
- enable OSC 52 copy for interactive byte-stream panes when their PTY
  controller is installed, matching local PTYs. Display-only logs keep OSC 52
  disabled, and interactive panes do not accept OSC 52 clipboard reads;
- clipboard and primary reads use the platform's asynchronous reads, and a
  paste that has to wait takes a `PasteTicket` (`paste_order.rs`, no upstream
  counterpart): keyboard input queued before `finish_paste` is written after
  the paste. Program replies and mouse or focus reports are not held;
- serve remote clipboard (zclip) requests off the terminal's event path:
  `clipboard_channel.rs` (no upstream counterpart) queues each printed frame,
  in order and boundedly, for one task that runs the zclip host on the
  background executor and writes its answer to the pty, and stops a host
  `zcopy`/`zpaste` helper that outlives `HELPER_TIMEOUT`. `vte` is a path
  dependency on `crates/vte`, whose `std` OSC buffer is bounded;
- split each reader handover into a non-blocking retirement on the terminal's
  thread and an owned, `Send` `RetiredReader` whose `finish` joins the pty loop
  or drains the byte stream elsewhere (`reader_handover.rs`, no upstream
  counterpart). `stop_pty_loop`, `attach_byte_stream` and `attach_pty` keep
  their old semantics by finishing whatever is left inline. Input written
  between a retirement and the next backend is held and flushed to that
  backend, and `GridSnapshotSource` serializes the grid from a worker;
- give each WSL/MSYS2/Cygwin shell a random nonce in
  `__ZETTA_COMMAND_MARKER_NONCE` and let only a `zetta-cmd;<nonce>:` marker
  carrying it reach `foreground_process_command_line_now`, which image paste
  turns into an SSH command. Unauthenticated `zetta-cmd:` markers still set
  the title. Attached terminals have no nonce and trust no marker.
- deliver bells one at a time: the listener sends a bell only when none is on
  its way (`WakeupGate::begin_bell`), and the event drain collapses queued
  bells the way it collapses wakeups. Binary output rings one every 256 bytes
  or so, and each was a channel allocation on the reader and a system bell on
  the UI.
- start a terminal at a real size before its first layout. The placeholder
  bounds hold 80x24 rather than upstream's 100x6, which a program that asked
  early took for its terminal, and `new_with_console_palette(_for_restore)`
  take `initial_bounds` for a pane whose grid is known in advance (Zetta's
  `--geometry`): the `Term`, a local pty, and a provider's
  `PtySpawnRequest::initial_size` all start there. Pinned by
  `the_placeholder_grid_is_the_conventional_terminal_size`.

`Content::selection_text` was removed after auditing the local public API
consumers. Zed's agent UI and full terminal view use the upstream field, but
Zetta compiles its standalone `terminal_view` instead; upstream UI consumers
must request selection text explicitly if they are ever ported here.

The current local fork also contains the application-facing changes from
Zetta's file-path and scrollback-editing work on 2026-08-03.

See `../UPSTREAM_AUDIT.md` for the reviewed upstream commit list.
