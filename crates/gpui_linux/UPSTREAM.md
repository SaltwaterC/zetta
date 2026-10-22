# Zetta GPUI Linux fork

This crate is synchronized with `zed/crates/gpui_linux` at Zed revision
`544780422affd11699fb531c49af5cc6e82a3fd7` (2026-10-21). Zetta owns the fork so Linux
platform fixes can be carried without modifying the upstream submodule.

Retain these Zetta patches when synchronizing:

- cap the GPUI background executor at eight worker threads;
- keep eligible Wayland press serials distinct from other protocol serials and
  choose them by observation order, so mouse-triggered selections and popup
  grabs remain valid across 32-bit serial wraparound;
- diagnose foreground tasks that block the event loop for more than two
  seconds (`linux/main_task_watchdog.rs`) and include the underlying
  event-loop error on termination. Upstream's `f37989fbdf` moved runnable
  dispatch into `LinuxCommon::register_sources` for every backend, so the
  watchdog now wraps that one place and covers X11 as well as Wayland;
- convert a `wl_keyboard` key through `wayland_keycode_to_xkb`, which drops
  keycodes that overflow the xkb offset, such as the `XKB_KEYCODE_INVALID`
  WSLg sends for a laptop's Fn key, instead of panicking on the addition;
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
- omit Zed's platform notification API (`linux/system_notifications.rs` and
  the `LinuxPlatform` notification methods) because Zetta owns notifications
  at the application layer;
- omit Wayland outbound drag (`c7aea6cbbd`: `start_external_drag`,
  `ExternalDrag`, the `DataSourceKind` user data on `wl_data_source`). Zetta
  has no drag source, and inbound file drops, including `87a1ea30e8`'s stale
  read generation, are upstream's;
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
  synchronous read now honours too. `DisplayConnection` dispatches both to the
  live connection; the headless connection answers empty. The captured X11
  connection opens the client's own display (`XContext::new(display)`), as
  upstream's reader now does. Native ownership regressions live in
  `src/tests/linux/x11/clipboard_capture.rs`; run the ignored
  `captured_selection_tests` with `--features test-support -- --ignored
  --test-threads=1` against an isolated X server.
- create a Wayland window's `WgpuRenderer` when the window is first painted
  rather than when it opens (`src/linux/wayland/deferred_renderer.rs`, a
  Zetta-authored module with tests in
  `src/linux/wayland/tests/deferred_renderer.rs`). Creating it is about 60 ms,
  mostly loading the Vulkan and EGL drivers, choosing an adapter and building
  the pipelines, and `open_window` used to spend it before the application had
  built the window's contents. Zetta starts its first terminal's child there,
  so the child started about 100 ms after launch instead of about 35 ms. GPUI
  takes the sprite atlas while creating the window, so the window hands out a
  `DeferredAtlas` that forwards to the renderer's own. The first tile inserted
  into it creates the renderer, because GPUI paints the window once inside
  `open_window`. Size, transparency and subpixel layout set before then are
  applied on creation, and so is the maximum window size the GPU's texture
  limit sets. A renderer that cannot be created closes the window at its first
  frame, where `open_window` used to return the error. The first draw now
  carries the renderer's creation, which raises the first second of
  `zetta benchmark`'s average draw time and not the later ones. The wgpu
  instance, about 28 ms of loading every Vulkan and EGL driver, needs only the
  display, so `prewarm_instance` starts creating it on a thread when the client
  connects. The first window builds the shared GPU context from it, choosing
  the adapter on a surface of its own that it drops before `WgpuRenderer::new`
  creates the renderer's, because `gpui_wgpu` cannot be patched to accept an
  instance. That moved the first renderer from about 95 ms after launch to
  about 80. X11 still creates its renderer with the window. The window's
  `Drop` calls `DeferredRenderer::release`, which drops the renderer or what
  would have created it, where upstream takes its `Option<WgpuRenderer>`: its
  GPU objects use the Wayland connection, which a display-mode switch tears
  down once no window is open.
- connect to the display server when the platform is created
  (`LinuxPlatform::connect_early`, called by `current_platform_with_path_prompt`),
  as the platform did before upstream's `f37989fbdf` moved connecting into
  `run`. The connection starts the wgpu instance prewarm, so it overlaps what
  Zetta does before `run` instead of following it; the first renderer, which
  waits for that instance, comes ~9 ms sooner. A `set_initial_windowing` made
  afterwards still applies: `run` then connects again in that mode.
- a client-driven resize asked for before a Wayland window's first configure
  is remembered, and that configure adopts it when it names no size
  (`first_configure_size`). A configure invalidates every queued resize, so
  this kind of resize used to race the first configure. It won only because
  creating the renderer had delayed the first commit; with the renderer
  deferred, the configure was dispatched first, and Zetta's `--geometry`
  windows stayed at their 1100x720 placeholder, a 153x42 grid. A size the
  compositor chose still wins.

The Wayland frame-callback lifecycle intentionally matches upstream. Do not
request callbacks from arbitrary foreground tasks or use empty surface commits
to implement idle rendering; that approach can latch redraw behind a delayed
compositor callback and put avoidable pressure on the Wayland connection.

See `../UPSTREAM_AUDIT.md` for the reviewed upstream commit list.
