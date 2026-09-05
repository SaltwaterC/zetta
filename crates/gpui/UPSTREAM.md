# Zetta GPUI fork

This crate is synchronized with `zed/crates/gpui` at Zed revision
`2890c340e07a4c4c7e6778e99a49f5414115b250`. Zetta owns the fork so rendering
fixes can be carried without modifying the upstream submodule.

Retain these Zetta patches when synchronizing:

- sort the three sprite vectors in `Scene::finish` with `sort_unstable_by_key`
  rather than `sort_by_key`. A sprite's `(order, tile_id)` key is fully
  discriminating for paint — two sprites sharing an order and an atlas tile draw
  the same texture inside the same layer — so their relative order cannot be
  observed, while a stable sort both compares more and allocates a scratch
  buffer of half the slice. At a screenful of terminal glyphs that buffer is
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
