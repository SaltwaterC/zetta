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
  refresh throttling;
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
  only after releasing the lock used by rendering and PTY parsing;
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
- clipboard and primary reads use the platform's asynchronous reads, and a
  paste that has to wait takes a `PasteTicket` (`paste_order.rs`, no upstream
  counterpart): keyboard input queued before `finish_paste` is written after
  the paste. Program replies and mouse or focus reports are not held;
- split each reader handover into a non-blocking retirement on the terminal's
  thread and an owned, `Send` `RetiredReader` whose `finish` joins the pty loop
  or drains the byte stream elsewhere (`reader_handover.rs`, no upstream
  counterpart). `stop_pty_loop`, `attach_byte_stream` and `attach_pty` keep
  their old semantics by finishing whatever is left inline. Input written
  between a retirement and the next backend is held and flushed to that
  backend, and `GridSnapshotSource` serializes the grid from a worker.

`Content::selection_text` was removed after auditing the local public API
consumers. Zed's agent UI and full terminal view use the upstream field, but
Zetta compiles its standalone `terminal_view` instead; upstream UI consumers
must request selection text explicitly if they are ever ported here.

The current local fork also contains the application-facing changes from
Zetta's file-path and scrollback-editing work on 2026-08-03.

See `../UPSTREAM_AUDIT.md` for the reviewed upstream commit list.
