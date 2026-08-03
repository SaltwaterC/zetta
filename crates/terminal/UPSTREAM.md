# Zetta terminal fork

The source baseline is `zed/crates/terminal` at Zed revision
`849ec5898a321eefbeb1d1beda130cc50ef43f10`. The local crate uses a
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
  PTY teardown, including application shutdown;
- allow Shift-drag to start selection while an application owns mouse
  tracking;
- diagnose terminal grid-lock and renderable-snapshot stalls without logging
  from the UI thread.

The current local fork also contains the application-facing changes from
Zetta's file-path and scrollback-editing work on 2026-08-03.

See `../UPSTREAM_AUDIT.md` for the reviewed upstream commit list.
