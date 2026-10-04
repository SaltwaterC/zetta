# Zetta GPUI Linux fork

This crate is synchronized with `zed/crates/gpui_linux` at Zed revision
`2890c340e07a4c4c7e6778e99a49f5414115b250`. Zetta owns the fork so Linux
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
- keep `set_input_focus` in `X11Window::activate`, which upstream dropped in
  `f4178619ac`; without it a window manager that ignores `_NET_ACTIVE_WINDOW`
  never focuses the activated window;
- route compositor-issued activation tokens through a local pending-token
  helper consumed by the next Wayland window activation, keeping this behavior
  in Zetta's platform fork rather than changing GPUI's upstream trait;
- omit Zed's platform notification API because Zetta owns notifications at the
  application layer;
- implement `read_from_clipboard_async` and `read_from_primary_async`. Cached
  and self-owned reads answer at once, as the synchronous read does. Otherwise
  Wayland sends `receive` and flushes on the GUI thread, for the offer current
  at the request, then reads the pipe from a calloop source as data arrives;
  X11 subscribes a dedicated connection to XFixes selection notifications and
  captures the owner at invocation (`linux/x11/clipboard/capture.rs`), then
  converts on a worker. Replacing ownership, including a write by the same
  owner window, invalidates the read across TARGETS, fallback and INCR instead
  of pasting the successor's data. External asynchronous reads require XFixes;
  setup failure returns an empty read. Setup still makes X-server round trips
  on the foreground, but never waits there for an external owner's payload.
  Both end a read once the owner goes quiet (`PIPE_READ_TIMEOUT`, X11's
  `LONG_TIMEOUT_DUR`) or after `CLIPBOARD_READ_DEADLINE` overall, which X11's
  synchronous read now honours too. Native ownership regressions live in
  `src/tests/linux/x11/clipboard_capture.rs`; run the ignored
  `captured_selection_tests` with `--features test-support -- --ignored
  --test-threads=1` against an isolated X server.

The Wayland frame-callback lifecycle intentionally matches upstream. Do not
request callbacks from arbitrary foreground tasks or use empty surface commits
to implement idle rendering; that approach can latch redraw behind a delayed
compositor callback and put avoidable pressure on the Wayland connection.

See `../UPSTREAM_AUDIT.md` for the reviewed upstream commit list.
