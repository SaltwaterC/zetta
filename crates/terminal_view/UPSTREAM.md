# Zetta terminal view fork

The renderer is synchronized with the compiled portions of
`zed/crates/terminal_view` at Zed revision
`c9e8e611dbc279afa0914d28c4d37ad07f38c03b`.

This fork intentionally builds `src/standalone.rs` rather than Zed's workspace
terminal view. Retain Zetta's standalone focus, clipboard, search, broadcast
input, resize, theme, and rendering-performance behavior. Zed editor,
workspace, project, database, language, panel, and persistence integrations are
out of scope unless Zetta independently adopts the corresponding feature.

Files belonging only to Zed's uncompiled workspace view are reference material,
not a source of automatic imports. See `../UPSTREAM_AUDIT.md` for decisions on
such changes.
