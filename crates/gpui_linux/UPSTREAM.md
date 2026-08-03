# Zetta GPUI Linux fork

This crate is synchronized with `zed/crates/gpui_linux` at Zed revision
`849ec5898a321eefbeb1d1beda130cc50ef43f10`. Zetta owns the fork so Linux
platform fixes can be carried without modifying the upstream submodule.

Retain these Zetta patches when synchronizing:

- cap the GPUI background executor at eight worker threads;
- keep eligible Wayland press serials distinct from other protocol serials and
  choose them by observation order, so mouse-triggered selections and popup
  grabs remain valid across 32-bit serial wraparound;
- diagnose foreground tasks that block the Wayland event loop for more than
  two seconds and include the underlying event-loop error on termination;
- invalidate stale programmatic resizes when compositor configures supersede
  them, while preserving the upstream Wayland frame-callback lifecycle;
- use physical-key ASCII mappings for number-row accelerators and provide a
  background Zenity save-file fallback when the portal cannot create a file;
- request a repaint after X11 exposure events rather than during the blocked
  event loop;
- omit Zed's platform notification API because Zetta owns notifications at the
  application layer.

The Wayland frame-callback lifecycle intentionally matches upstream. Do not
request callbacks from arbitrary foreground tasks or use empty surface commits
to implement idle rendering; that approach can latch redraw behind a delayed
compositor callback and put avoidable pressure on the Wayland connection.

See `../UPSTREAM_AUDIT.md` for the reviewed upstream commit list.
