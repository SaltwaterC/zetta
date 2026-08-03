# Zetta GPUI macOS fork

The source baseline is `zed/crates/gpui_macos` at Zed revision
`849ec5898a321eefbeb1d1beda130cc50ef43f10`. Zetta owns this fork for macOS
input-source and menu behavior without modifying the upstream submodule.

Retain these Zetta changes when synchronizing:

- tolerate input-source agent restarts and missing keyboard-layout data;
- gate AppKit text-input contexts while the application or input source is
  inactive, avoiding stale-context recursion and use-after-free paths;
- observe authoritative keyboard-source notifications and defer safe mapper
  replacement to the main queue;
- retain pasteboard objects and copy custom pasteboard data before its
  autorelease pool expires;
- honor GPUI's per-window resizable/minimizable state when performing a
  titlebar double-click action;
- keep the current appearance/blur behavior synchronized with upstream;
- install native macOS menu behavior and Ctrl-Shift profile shortcuts;
- keep the regression tests under `src/tests`.

The fork was introduced by Zetta commit `38b9185`.
