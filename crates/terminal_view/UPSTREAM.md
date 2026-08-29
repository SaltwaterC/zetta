# Zetta terminal view fork

The renderer's source baseline is the compiled portions of
`zed/crates/terminal_view` at Zed revision
`2890c340e07a4c4c7e6778e99a49f5414115b250`. Zed's workspace-only files are
not a synchronization source for this crate.

This fork intentionally builds `src/standalone.rs` rather than Zed's workspace
terminal view. Retain Zetta's standalone focus, clipboard, search, broadcast
input, pane resize, path-target, theme/font, literal-search, scrollback-edit,
inline sizing, alternate-screen anchoring, and rendering-performance behavior.
Custom block, quadrant, shade, and sextant glyphs are painted as pixel-snapped
subcell quads rather than shaped font text; retain the ordered merge path so a
dense image cannot turn terminal layout into a quadratic operation.
A hovered word is matched by id rather than by value, because the terminal
shifts a carried match's lines as output scrolls and comparing whole words made
the link blink out for the frames in which the two disagreed.
Zed editor, workspace, project, database,
language, panel, and persistence integrations are out of scope unless Zetta
independently adopts the corresponding feature.

Files belonging only to Zed's uncompiled workspace view are reference material,
not a source of automatic imports. See `../UPSTREAM_AUDIT.md` for decisions on
such changes.
