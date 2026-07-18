# Zetta GPUI Linux fork

This crate is copied from `zed/crates/gpui_linux` at the revision recorded by
the `zed` submodule. Zetta owns the fork so Linux platform behavior can be
updated independently while retaining the same GPUI interfaces.

The Wayland backend requests compositor frame callbacks on demand. Foreground
GPUI work and window input/configuration request at most one callback, and a
rendered frame requests one successor so animations continue. When that
successor finds no drawing work, the callback chain stops instead of waking the
application once per monitor refresh indefinitely.
