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
Pastes never read the clipboard synchronously, including the right-click
decision between pasting and the context menu. A paste's target is fixed when
it is asked for, and one that waits holds both the pane's input and the input
events it rebroadcasts behind it.
A hovered word is matched by id rather than by value, because the terminal
shifts a carried match's lines as output scrolls and comparing whole words made
the link blink out for the frames in which the two disagreed.
`src/grid_fit.rs`, a Zetta-authored module (tests in `src/tests/grid_fit.rs`),
owns how a standalone element turns its space into a grid and back: the
terminal font as settings resolve it, the snap of the height to whole rows of
the device-rounded line height, and the element size that lays out a given
grid. The element's prepaint calls `TerminalFont::from_settings` and
`snap_to_rows` instead of resolving them inline, so sizing a window for a
grid before it has a terminal (Zetta's `--geometry`) cannot drift from what
the element then lays out; `default_terminal_font_features` and
`resolve_cell_width` are `pub(crate)` for it.
Zed editor, workspace, project, database,
language, panel, and persistence integrations are out of scope unless Zetta
independently adopts the corresponding feature.

Search query edits notify the view immediately, independently of debounce,
snapshot capture, history reflow, or match updates. The windowed regression
checks the rendered field after resize before the debounce timer can fire.
The view delegates search jobs, invalidation, result application and navigation
to `terminal::search_session::SearchSession`, also used by tab-wide search.
Keep only query editing, focus and a display-status snapshot in this view.
`search_overlay.rs` draws the pane field; its sidecar keeps the windowed
repaint and narrow-layout regressions with that presentation.
The match counter wraps below the query in narrow panes instead of squeezing
the input to zero width; a windowed layout test covers large match counts.

`src/line_size.rs`, a Zetta-authored module (tests in `src/tests/line_size.rs`),
draws DEC double-width and double-height rows. Layout stays in grid columns:
the element leaves a doubled row's hidden second half out of layout, doubles
cell positions and widths as it paints, and paints the row's text through
gpui's `Window::with_text_scale` (see `../gpui/UPSTREAM.md`). The cursor, the
selection and the highlighted ranges follow the same scale. The view drops
typed and committed text while the terminal reports a locked keyboard (KAM),
and its context menu offers **Reset**.

Files belonging only to Zed's uncompiled workspace view are reference material,
not a source of automatic imports. See `../UPSTREAM_AUDIT.md` for decisions on
such changes.

## Zetta patch: physical cell geometry

The process-local `PhysicalCellSize` override specifies backing-pixel cells, converted using each window scale for both initial grid sizing and rendering. Glyph font size fits inside that cell. Normal font-derived layout remains the default.
