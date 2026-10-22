# Zetta GPUI Windows fork

This crate is synchronized with `zed/crates/gpui_windows` at Zed revision
`544780422affd11699fb531c49af5cc6e82a3fd7` (2026-10-21). Zetta owns the fork so Windows
platform fixes can be carried without modifying the upstream submodule.

Retain these Zetta patches when synchronizing:

- DirectWrite uses its cached system font catalog at startup and refreshes it
  when enumerating fonts or retrying a failed lookup. A native API comparison
  measured 50–54 ms for a forced refresh versus approximately 1 ms for the
  cached catalog, with the same 145 families. Font discovery still observes
  updates, and refreshing retains previously selected font IDs and metrics;
  `font_discovery_refreshes_a_stale_catalog_without_discarding_cached_fonts`
  exercises that with a deliberately stale catalog.

- `WindowsWindow::zoom` toggles between `SW_MAXIMIZE` and `SW_RESTORE` based
  on `IsZoomed`, instead of always issuing `SW_MAXIMIZE`. Upstream's version
  never restores a maximized window, unlike the Wayland, X11, and macOS
  backends, which do genuinely toggle. Zetta's pane-resize growth
  (`resize_window` in `src/app.rs`) relies on `zoom_window()` un-maximizing
  the window so a subsequent resize can grow it past its maximized bounds;
  without this patch, growing a pane after the window had been maximized (and
  then shrunk back to a floating size, which does not clear `WS_MAXIMIZE`)
  gets stuck calling `SW_MAXIMIZE` on an already-maximized window forever.

- `WindowsPlatform::prompt_for_paths_in`, the rooted path picker
  `crates/gpui_platform` routes `prompt_for_paths_in` to, with
  `Platform::prompt_for_paths` delegating to it with no directory. It goes
  through upstream's `dialog::show_dialog` like every other dialog, so it runs
  off the foreground thread and is owned by the active window's
  `DialogOwner`. `dialog_folder_item` resolves the starting directory for both
  the open and the save dialog; a directory that cannot be resolved is logged
  and dropped rather than failing the dialog.

- Non-client double-clicks: `handle_nc_mouse_down_msg` counts the click before
  taking the input callback and swallows the second `HTCAPTION` press, the
  non-client mouse-up reports the real click count rather than 1, and an
  `HTCAPTION` press/release pair counts as handled. `ClickState` accepts twice
  the system's double-click rectangle, which upstream's
  `test_double_click_spatial_tolerance` now pins (it still asserted the
  single rectangle, and failed on Windows from `bd7aa12` until the 2026-10-21
  sync). Together these let a double-click on
  Zetta's custom title bar (the tab strip in compact mode) reach GPUI instead
  of being eaten by the default window procedure.

Also retain the synchronized upstream changes for DirectX scene annotations,
one-shot attention flashing, inactive popup activation, and input activation.
The manifest is local so the application can select the fork through
`gpui_platform`.

Before the 2026-10-21 sync this fork lacked three upstream changes that the
previous pin already carried: the cross-window `DrawCoordinator` and
`force_render_pending` from `ec3d887507`, the `WM_ENDSESSION` shutdown from
`7f2a2c3c3e`, and `render_to_image` with the restart-argument test. They were
never Zetta patches — `ec3d887507` was deferred while `gpui` itself was not
forked — so the sync takes them from upstream.

See `../UPSTREAM_AUDIT.md` for the reviewed upstream commit list.
