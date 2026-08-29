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

The current local fork also contains the application-facing changes from
Zetta's file-path and scrollback-editing work on 2026-08-03.

See `../UPSTREAM_AUDIT.md` for the reviewed upstream commit list.
