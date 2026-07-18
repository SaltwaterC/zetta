# Zetta GPUI Linux fork

This crate is synchronized with `zed/crates/gpui_linux` at Zed revision
`c9e8e611dbc279afa0914d28c4d37ad07f38c03b`. Zetta owns the fork so Linux
platform fixes can be carried without modifying the upstream submodule.

Retain these Zetta patches when synchronizing:

- cap the GPUI background executor at eight worker threads;
- choose Wayland clipboard serials by observation order and input kind, so
  mouse-triggered selections remain valid across 32-bit serial wraparound;
- diagnose foreground tasks that block the Wayland event loop for more than
  two seconds and include the underlying event-loop error on termination.

The Wayland frame-callback lifecycle intentionally matches upstream. Do not
request callbacks from arbitrary foreground tasks or use empty surface commits
to implement idle rendering; that approach can latch redraw behind a delayed
compositor callback and put avoidable pressure on the Wayland connection.

See `../UPSTREAM_AUDIT.md` for the reviewed upstream commit list.
