# Zetta terminal fork

This crate is synchronized with `zed/crates/terminal` at Zed revision
`c9e8e611dbc279afa0914d28c4d37ad07f38c03b` and uses a standalone manifest.

Retain these Zetta-specific behaviors when synchronizing:

- allow scrollback up to Alacritty's signed line-coordinate range instead of
  Zed's 100,000-line product limit;
- expose PTY metadata and startup signaling required by standalone profiles,
  WSL working-directory tracking, pane output export, and tab titles;
- preserve immediate first-event processing with bounded four-millisecond PTY
  drains;
- diagnose terminal grid-lock and renderable-snapshot stalls without logging
  from the UI thread.

See `../UPSTREAM_AUDIT.md` for the reviewed upstream commit list.
