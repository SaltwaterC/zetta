# Zetta GPUI Windows fork

This crate is synchronized with `zed/crates/gpui_windows` at Zed revision
`849ec5898a321eefbeb1d1beda130cc50ef43f10`. Zetta owns the fork so Windows
platform fixes can be carried without modifying the upstream submodule.

Retain this Zetta patch when synchronizing:

- `WindowsWindow::zoom` toggles between `SW_MAXIMIZE` and `SW_RESTORE` based
  on `IsZoomed`, instead of always issuing `SW_MAXIMIZE`. Upstream's version
  never restores a maximized window, unlike the Wayland, X11, and macOS
  backends, which do genuinely toggle. Zetta's pane-resize growth
  (`resize_window` in `src/app.rs`) relies on `zoom_window()` un-maximizing
  the window so a subsequent resize can grow it past its maximized bounds;
  without this patch, growing a pane after the window had been maximized (and
  then shrunk back to a floating size, which does not clear `WS_MAXIMIZE`)
  gets stuck calling `SW_MAXIMIZE` on an already-maximized window forever.

Also retain the synchronized upstream changes for DirectX scene annotations,
one-shot attention flashing, inactive popup activation, and input activation.
The manifest is local so the application can select the fork through
`gpui_platform`.

See `../UPSTREAM_AUDIT.md` for the reviewed upstream commit list.
