# Zetta GPUI macOS fork

The source baseline is `zed/crates/gpui_macos` at Zed revision
`544780422affd11699fb531c49af5cc6e82a3fd7` (2026-10-21). Zetta owns this fork for macOS
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
- let `prompt_for_paths_in` open the panel in a starting directory, which
  `crates/gpui_platform` calls; `prompt_for_paths` delegates to it with `None`;
- normalize mouse-down modifiers so a Shift held at the time of the click is
  reported even when AppKit's event omits it;
- keep the regression tests under `src/tests`.

### Patch shapes after the objc2 sync

Upstream moved most of this crate from `cocoa`/`objc` to `objc2`. The Zetta
patches that touch code upstream migrated follow it; the rest still use the
legacy bindings, which upstream keeps beside `objc2`:

- `prompt_for_paths_in` is upstream's `objc2` panel code (`NSOpenPanel`,
  `RcBlock`, the stored `MainThreadMarker`) plus a `setDirectoryURL` from
  `NSURL::from_directory_path`.
- The input-source property read in `keyboard.rs` takes `*mut AnyObject` and
  reads the value as an `objc2_foundation::NSString`; the null checks stay.
- Mouse-down modifiers are read with `NSEvent::modifierFlags_class()` and the
  `objc2` `NSEventModifierFlags` names.
- The text-input-context gate (`GPUITextInputContext`, the view's
  `inputContext` override and ivars, `forward_event_to_input_context`), the
  distributed TIS observers, the deferred layout change and the profile-menu
  aliases are unchanged and still use `objc`/`cocoa`.
- `dispatcher.rs`, `open_type.rs` and `text_system.rs` moved to `gpui_apple`
  upstream; Zetta never patched them, so they are not carried here.
- Upstream's simple fullscreen (`toggle_simple_fullscreen`) and external drag
  (`start_external_drag`, `FileDropEvent::Ended`) are now taken as-is. The
  previous fork omitted them; Zetta calls neither, so they are inert, and
  carrying them keeps `window.rs` close to upstream.

The Metal renderer is deliberately *not* forked. `metal_renderer.rs`,
`metal_atlas.rs`, `shaders.metal` and the `cbindgen` build script were carried
here only because upstream kept them in this crate; upstream `52b2418110` moved
them to `gpui_apple`, and since Zetta had never modified them, this crate now
depends on `gpui_apple` instead (the routing-only fork in `crates/gpui_apple`). Do not reintroduce local copies —
renderer changes belong upstream.

The fork was introduced by Zetta commit `38b9185`.

## Cargo package identity

Keep the local package version distinct from upstream (`0.1.0-zetta` rather
than `0.1.0`). Cargo reads upstream GPUI manifests through transitive
dev-dependencies while reconstructing path packages from the lockfile. With
identical names and versions, it can attach the lock entry to the upstream
copy, leaving the local fork's patched `block` dependency unlocked and
refreshing the crates.io index on every invocation. Preserve this version
distinction when synchronizing, and regenerate the affected lockfiles.
