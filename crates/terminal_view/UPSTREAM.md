# Zetta terminal view fork

The renderer's source baseline is the compiled portions of
`zed/crates/terminal_view` at Zed revision
`849ec5898a321eefbeb1d1beda130cc50ef43f10`. Zed's workspace-only files are
not a synchronization source for this crate.

This fork intentionally builds `src/standalone.rs` rather than Zed's workspace
terminal view. Retain Zetta's standalone focus, clipboard, search, broadcast
input, pane resize, path-target, theme/font, literal-search, scrollback-edit,
inline sizing, alternate-screen anchoring, and rendering-performance behavior.
Custom block, quadrant, shade, and sextant glyphs are painted as pixel-snapped
subcell quads rather than shaped font text; retain the ordered merge path so a
dense image cannot turn terminal layout into a quadratic operation.
Zed editor, workspace, project, database,
language, panel, and persistence integrations are out of scope unless Zetta
independently adopts the corresponding feature.

Files belonging only to Zed's uncompiled workspace view are reference material,
not a source of automatic imports. See `../UPSTREAM_AUDIT.md` for decisions on
such changes.
