# Zetta GPUI fork

This crate is synchronized with `zed/crates/gpui` at Zed revision
`544780422affd11699fb531c49af5cc6e82a3fd7` (2026-10-21). Zetta owns the fork so rendering
fixes can be carried without modifying the upstream submodule.

Retain these Zetta patches when synchronizing:

- sort the three sprite vectors in `Scene::finish` with `sort_unstable_by_key`
  rather than `sort_by_key`, keeping upstream's key. Upstream's `b4ecc45422`
  changed that key from `(order, tile_id)` to `(order, texture_id.index)`, so
  `batches` splits sprites only where the atlas texture changes, and Zetta
  takes the new key. Within one order and texture an unstable sort may reorder
  sprites, which is observable only where two of them overlap inside one
  layer; the old key, sorted either way, already put such sprites in tile
  order rather than paint order, so this is no change for Zetta. A stable sort
  both compares more and allocates a scratch buffer of half the slice. At a screenful of terminal glyphs that buffer is
  large enough to reach the allocator's mmap threshold once per frame, which
  showed up as kernel page-zeroing in the profile. The other five vectors stay
  stable on purpose: every primitive inside one `push_layer` shares that layer's
  order, and reordering equal-order quads is only sound when they do not
  overlap, which a layer does not guarantee in general. Measured at −5.7% median
  draw time on the standard terminal workload.

- `ShapedLine::paint_in_layer`, and the `paint_line_glyphs` split that `paint`
  and it share. `paint` opens a scene layer per call, and a layer costs a
  `BoundsTree` insert — a spatial search against every layer already in the
  frame. A terminal pane shapes one line per run of same-styled cells, which
  measured 336.8 layers and 376.2 tree inserts per frame against 40.8 and 79.9
  once `terminal_element.rs` wrapped every run of a pane in one layer instead.
  Collapsing them is order-preserving rather than a trade: the runs tile a grid
  and `Bounds::intersects` is strict about touching edges, so no run's layer
  intersected its neighbour and all of them already resolved to the same order.
  Measured at −24.6% mean draw time and −14.3% process CPU, 8 of 8 paired runs.
  Upstream's later `paint_line` takes a `paint_underline` callback so a caller
  can record underlines (`Window::painted_underlines`); `paint_line_glyphs`
  takes the same callback, and `paint_in_layer` passes
  `Window::paint_underline`.

- `Window::glyph_sprites`, a per-frame memo of the glyph lookup that
  `paint_glyph` and `paint_emoji` both do, reached through `Window::glyph_sprite`.
  Upstream hashes the same eight-field `RenderGlyphParams` twice per glyph — the
  text system's raster-bounds map behind an `RwLock`, then the sprite atlas
  behind a `Mutex` — and a terminal screen paints ~3,300 glyphs drawn from a few
  hundred distinct keys, since a glyph repeats across columns at one of
  `SUBPIXEL_VARIANTS_X` phases. Cleared at the top of `Window::draw`, which is
  the whole invalidation story: no entry outlives the paint pass that made it,
  and both things that drop atlas tiles — the renderer's incremental recovery
  and device loss — run after paint. Measured at −10.6% mean and −11.2% median
  draw time, 7 of 8 paired runs (8 of 8 on the median).

- `Window::with_text_scale`, which stretches the bounds of every glyph, emoji,
  underline and strikethrough painted inside it away from one origin. It is how
  the terminal draws DEC double-width and double-height lines: a row's text is
  shaped and painted at its ordinary size and stretched as it goes into the
  scene. Stretching bounds rather than setting a sprite transformation means
  no renderer has to support it — polychrome sprites and underlines have no
  transformation at all — and a terminal row's stretch is always axis-aligned.
  Glyphs are rasterized at the part of the stretch that is the same both ways
  (`Window::text_scale_glyph`), so a double-height line is drawn from a raster
  at twice the font size rather than a magnified one; only double width's
  horizontal half is stretched. Upstream now builds every underline through
  `Window::underline`, so the stretch for underlines and strikethroughs is
  applied there (`scale_text_bounds`), once for every path that paints one.

- `Platform::read_from_primary_async` and `App::read_from_primary_async`, the
  primary-selection counterpart of upstream's `read_from_clipboard_async`, with
  the same synchronous default. Linux overrides both so a paste from another
  client's selection waits without blocking the GUI thread.

- `TestAppContext::defer_clipboard_reads` and `complete_clipboard_reads`, which
  hold the test platform's asynchronous clipboard reads as a slow owner would,
  each answered with what the clipboard held when it was requested. They are
  how the paste-order barrier is tested against a read that resolves late.

## Why this crate is forked at all

Unlike the platform forks, `gpui` is not a leaf. Twenty-two other `zed/` crates
in Zetta's graph depend on it, and Cargo resolves those edges from manifests
inside the submodule, which Zetta does not own. A path override in
`.cargo/config.toml` was tried and rejected: it rearranged the crate graph —
resolving `gpui_platform` to the upstream copy rather than Zetta's fork — and
Cargo documents that as unsupported and slated to become a hard error. Two
copies of `gpui` in one binary is not an option either, because `ui` and friends
take `gpui` types in their public API and because `gpui` holds process-wide
state. Forking `gpui` therefore required forking every crate between it and
Zetta; see `crates/UPSTREAM_AUDIT.md` for that set.

## Local adjustments made by the move, not by intent

These are mechanical consequences of the crate no longer living inside Zed's
workspace. They are not behavior changes, and a synchronization has to reapply
them rather than resolve them as conflicts:

- `Cargo.toml` spells out every dependency that was `workspace = true`, and the
  `[lints]` table, because neither can be inherited from outside
  `zed/Cargo.toml`. Re-resolve these against the new pin when syncing.
- `publish` is `false`.
- `include_bytes!` paths that reached Zed's `assets/` now spell
  `../../../zed/assets/...`, since the crate moved but the asset directory did
  not.
