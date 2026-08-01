# Zetta GPUI Windows fork

This crate is synchronized with `zed/crates/gpui_windows` at Zed revision
`c9e8e611dbc279afa0914d28c4d37ad07f38c03b`. Zetta owns the fork so Windows
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

See `../UPSTREAM_AUDIT.md` for the reviewed upstream commit list.
