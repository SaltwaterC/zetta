# AGENTS.md

## Scope

These instructions apply to the entire repository unless a more specific
`AGENTS.md` exists below the file being changed.

## Project overview

Zetta is a standalone, cross-platform terminal emulator built with Rust,
GPUI, and Zed's terminal engine. The root package is the application. Local
forks and platform support live under `crates/`; `zed/` is an upstream Git
submodule used for dependencies.

Use the Rust toolchain pinned in `rust-toolchain.toml` (Rust 1.95.0 with
`rustfmt` and `clippy`). Initialize the submodule before the first build:

```sh
git submodule update --init
```

## Repository boundaries

- Treat `zed/` as upstream code. Do not modify it unless the task explicitly
  requires an upstream dependency change.
- Treat `busy-v/` as upstream code. Do not modify it unless the task
  explicitly requires an upstream dependency change.
- Code under `crates/` is maintained as part of Zetta and may be changed when
  the application needs corresponding terminal or platform behavior.
- Keep platform-specific behavior behind the existing `cfg` boundaries. Linux
  defaults to Wayland; the `x11` feature enables the X11 backend.
- Preserve unrelated working-tree changes. Do not rewrite or clean files that
  are outside the requested scope.

## Application architecture

Keep `src/main.rs` limited to crate wiring, actions, shared imports/constants,
and the process entry point. Put behavior in the module that owns it:

- `app.rs`: `Zetta` struct, tab/pane lifecycle, and state that doesn't belong
  to a narrower module below
- `terminal_spawn.rs`: terminal process spawning and its event wiring
- `configuration_reload.rs`: settings/keymap file editing and configuration
  reload
- `rename.rs`: tab and pane rename state
- `app_render.rs`: top-level `Render for Zetta` composition (action
  registration, overlay collection, and the tab-icon-picker/overlay-style-picker
  overlays); delegates to `title_bar_render.rs` and `tab_body_render.rs`
- `view_boundary.rs`: `ZettaSubview`, the entity wrapper that lets part of the
  render tree be cached and be the target of its own scroll/hover
  notifications; see "Render boundaries" below
- `title_bar_render.rs`: title bar composition, its menus (application,
  profile, reconnect), the layout predicates the bar shares with the tab bar,
  and `title_bar_chrome_height`, which sizes the cached chrome boundary
- `tab_bar_render.rs`: the measured tab row, individual tabs, and the bar
  that hosts them
- `tab_body_render.rs`: tab body composition (maximized-pane bar, minimized
  pane shelf, pane content wiring)
- `pane.rs`: pane layout, tab models, terminal creation, and pane focus
- `pane_resize.rs`: pane resize/move mode, keyboard and drag-based resizing
- `pane_render.rs`: pane layout and resize-gutter rendering
- `pane_view_state.rs`: pane maximize/minimize/restore and font size
- `pane_overlay.rs`: per-pane overlay text and style picker
- `pane_controls.rs`: per-pane control visibility and its idle timer
- `pane_theme_picker.rs`: per-pane theme picker model and overlay
- `background_session_ui.rs`: background-session detach/store/reconnect and
  the reconnect picker
- `byte_stream_pane.rs`: shared pane opener for byte-stream-backed panes
  (HTTP/TFTP server log panes, the serial console)
- `cli_service_stubs.rs`: disabled-build fallbacks for CLI-service actions
- `performance.rs`: frame collection, performance metrics, and the
  performance overlay
- `tab_search.rs`: cross-pane scrollback search and its overlay
- `tab_icon_picker.rs`: tab icon picker model, rendering, and the
  `Zetta` methods that drive it
- `settings_editor.rs`: typed configuration/keymap forms and persistence
- `settings_ui.rs`: settings state and event handling; a module directory —
  `settings_ui/keymap.rs` (capture, search cache), `settings_ui/controls.rs`
  (control list, focus/scroll navigation, dropdowns), and
  `settings_ui/theme_extensions_ui.rs` (fetch/download/remove)
- `settings_view.rs`: settings rendering; a module directory —
  `settings_view/pages.rs` (per-`SettingsPage` content),
  `settings_view/modals.rs` (font/profile/keymap-capture modals),
  `settings_view/form_widgets.rs` (the form's shared controls, held in a struct
  so the page and the modals can be built in separate passes), and
  `settings_view/widgets.rs` (shared widget building blocks)
- `command_palette.rs`: palette model and matching
- `command_palette_ui.rs`: palette interaction, rendering, and its overlay
- `window_frame.rs`: window decorations (`WindowFrameGeometry`), window
  controls, and resize edges
- `startup.rs`: `run()`, window/process lifecycle, and theme resolution
  (`resolve_profile_theme`); a module directory — `startup/cli_help.rs`
  (usage/help text), `startup/arg_parsing.rs` (`StartupMode`/`StartupArgs`
  parsing), `startup/keybindings.rs` (keybinding constants/constructors and
  macOS native menu construction), and `startup/wsl.rs` (WSL/MSYS2 profile
  and working-directory integration)
- `cli_services.rs`: CLI service dispatch; a module directory —
  `cli_services/serial.rs`, `cli_services/servers.rs` (HTTP + TFTP server),
  `cli_services/notify.rs`, `cli_services/clipboard.rs`, and
  `cli_services/raw_terminal.rs`
- `tftp.rs`: shared TFTP packet/opcode types; a module directory —
  `tftp/server.rs` and `tftp/client.rs`
- `theme_extensions.rs`: theme-extension discovery and installation
- `zetta_assets.rs`: embedded assets

Prefer extending these modules over growing `main.rs`. If a module becomes
difficult to navigate, split it by responsibility rather than creating a
generic helpers module. Keep rendering code separate from state transitions
where practical.

Prefer splitting a module by responsibility once it approaches roughly 1500
lines rather than letting it keep growing. Never let a single `render`
function or method own an entire screen or top-level function own an entire
CLI/state surface — extract per-section methods or functions (passing in
already-computed values instead of recomputing them) once that function is
hard to scan in one pass.

## Tests

Unit tests live in `src/tests/` and mirror their production module. Production
modules include their sidecar with this pattern:

```rust
#[cfg(test)]
#[path = "tests/pane.rs"]
mod tests;
```

Place new tests in the matching sidecar. Create a new sidecar when adding a
new module with testable behavior. Use `use super::*;` so unit tests can cover
private implementation details. Reserve Cargo's root `tests/` directory for
true public-API integration tests.

When a production module is a directory (for example `src/cli_services/` or
`src/startup/`), its sidecar becomes a matching directory under `src/tests/`
(for example `src/tests/startup/keybindings.rs`, referenced from
`src/startup/keybindings.rs` as `#[path = "../tests/startup/keybindings.rs"]`).
Only split a sidecar into a directory once its production module is actually
split; keep a single flat sidecar file otherwise.

Files under `crates/` that track an upstream Zed or Alacritty counterpart keep
their inline `mod tests` to minimize merge friction against that upstream.
Zetta-authored files in `crates/` with no upstream counterpart (for example
`crates/terminal_view/src/standalone.rs`) use the same sidecar pattern as
`src/`, under that crate's own `src/tests/`.

Remember that `include_str!` and `include_bytes!` paths are relative to the
file containing the macro; update such paths when moving tests or source.

### Windowed tests

`gpui` is a dev-dependency with `test-support`, so rendering behaviour can be
tested against a real window on GPUI's test platform — no display required.
Write these with `#[gpui::test]` and a `&mut TestAppContext`:

- `cx.add_window_view(..)` opens a window and hands back the root view plus a
  `VisualTestContext`; `cx.open_window(size, ..)` picks the window size when the
  test is layout-sensitive.
- `cx.run_until_parked()` drains the executor, which draws any dirty window. A
  view that should *not* have re-rendered is asserted by counting renders in the
  view itself (see `src/tests/view_boundary.rs`).
- `.debug_selector(|| ..)` on an element records its bounds in
  `cx.debug_bounds(..)`, which is how layout is asserted.

`src/tests/view_boundary.rs` is the worked example: it drives a stand-in parent
view through the render-boundary contract below. Prefer a stand-in over a real
`Zetta` — `Zetta::new` opens a tab, which spawns a shell.

Assertions about caching are easy to write vacuously. Check a new one fails when
the property it names is removed before trusting it.

## Validation

Use the smallest useful check while iterating, then validate the completed
change from the repository root:

```sh
cargo fmt --all --check
cargo check
cargo test
git diff --check
```

Zetta has no library target, so do not use `cargo test --lib` (including for
focused tests). Run a focused test with its filter against the binary target,
for example:

```sh
cargo test pane_controls
```

Run Clippy for broader Rust changes when practical:

```sh
cargo clippy --all-targets
```

For changes touching Linux platform selection, also check the relevant
feature combination, for example:

```sh
cargo check --no-default-features --features x11
```

For changes touching a CLI service (`cli_services.rs`/`tftp.rs` and their
gating), also check every CLI service disabled, since that combination
exercises every `cli_services`/`servers_enabled`/`tftp_enabled` gate:

```sh
cargo check --no-default-features --features wayland
```

Do not run `make install`, uninstall targets, or system-cache refresh targets
as validation; they mutate the host system. `make build` produces the release
artifact and is only necessary for release, packaging, or installation work.

## Render boundaries

GPUI re-renders the root view on every frame it draws, so anything built
directly inside `Zetta::render` is rebuilt and re-laid-out even when nothing it
displays changed. Scrolling an overlay is the pathological case: one notify per
wheel step otherwise redraws the title bar, the tab bar, the pane chrome and the
whole settings page for a frame in which none of them moved.

`ZettaSubview` (`view_boundary.rs`) wraps part of the tree in its own entity,
which does two things: GPUI can cache the subtree with `Entity::cached`, and
GPUI's interaction handlers notify *that* view rather than the root, because
they notify `window.current_view()`.

The current boundaries are the title bar chrome (title bar plus, outside compact
mode, the tab bar row — cached), the settings dialog and the tab icon picker
(boundaries only), and the settings page inside the dialog (cached).

When adding one:

- **Cache a sibling of what changes, never an ancestor of it.** GPUI re-renders
  a missing cached view's subtree with `Window::refreshing` set, and reuse
  requires `!window.refreshing`, so a cache that misses suppresses every cache
  below it. `Zetta::render` composes the window column directly for this reason:
  wrapping it in a cache that terminal output always dirties both rebuilt the
  chrome every frame *and* stopped the per-pane caches from ever hitting.
- Invalidation is the observer in `ZettaSubview::new`: every `cx.notify()` on
  `Zetta` marks the subview dirty. Keep subviews rendering purely from `Zetta`
  state so that stays a complete contract. Descendants are GPUI's job —
  notifying a view marks its whole ancestor chain dirty, which is why terminal
  output still repaints through the pane it happened in.
- State a cached boundary displays but does not own needs a route back to a
  notify on `Zetta`. The title bar reports the active pane's grid size, which
  the terminal owns; `Event::GridSizeChanged` exists to carry exactly that and
  nothing else, because reporting it on ordinary output would put the chrome
  back into every frame.
- A cached view is laid out from the style passed to `cached`, not measured from
  its contents, so that style has to give it a definite size — see
  `title_bar_chrome_height`. Position it from the composing side: an `absolute`
  root inside a cached view has no containing block to resolve against and
  collapses to its content size.
- Cache a boundary only when it can actually be reused. A cached view that
  misses pays an extra layout pass, which measurably costs more than it saves
  for whichever overlay the pointer is currently scrolling.

The contract is pinned by `src/tests/view_boundary.rs`; extend it when adding a
boundary whose invalidation or layout differs from the ones there.

## Scene layers

A primitive painted outside a scene layer has to work out its own paint order,
which GPUI does by inserting its bounds into the frame's bounds tree
(`Scene::insert_primitive` → `BoundsTree::insert`). That is a tree search
against everything already inserted, so it is superlinear in the number of
primitives a frame emits.

Anything that paints many non-overlapping quads in a loop should paint them
inside one `window.paint_layer(..)`, which gives them a single shared order for
one insertion — see `paint_grid_layer` in `terminal_element.rs`. Ordering
against everything else is unaffected as long as the layer's bounds cover the
primitives, because later primitives that intersect the layer still sort above
it.

The terminal reached this the hard way: a screen where no two neighbouring cells
share a background emitted one quad per cell, and `BoundsTree::insert` measured
59-66% of the process's samples. Note also that `ShapedLine::paint` opens a
layer per call, so avoid emitting one text run per cell — see
`paints_only_background`.

## Performance profiling

Every change must consider its performance impact. Before completing a change,
carry out a performance-focused code review of the completed diff, paying
particular attention to hot render and input paths, algorithmic scaling,
allocations, repeated I/O or process spawning, locking, and unnecessary work.
Record any material findings and address them when the task includes
implementation; use profiling or benchmarks when static review is not enough
to establish the impact.

Use the built-in terminal-rendering workload for reproducible performance
checks on Linux, macOS, and Windows. Always use an optimized build when
recording or comparing results:

```sh
cargo run --release -- \
  benchmark \
  --profile-report artifacts/zetta-performance.json \
  --profile-duration 10
```

`zetta benchmark --profile-report` enables an automated timed run and defaults to ten seconds
when `--profile-duration` is omitted. The command creates missing report parent
directories, writes versioned JSON, and exits. Treat a non-zero exit status or
a missing report as a failed performance run. Preserve the JSON as a CI
artifact and compare like-for-like release builds, workload settings, and
platforms. Use the live `zetta benchmark` mode without report
arguments for interactive investigation.

Automated runs require a graphical session and the platform's normal GPU
backend; do not compare a headless/software-rendered run with an interactive
hardware-rendered baseline.

The JSON contains portable frame timing summaries and per-second samples. Use
`perf` on Linux, Instruments or `sample` on macOS, and Windows Performance
Recorder/Analyzer when native stack traces are also needed; keep those traces
as separate artifacts associated with the JSON report.

## Change guidelines

- Keep changes behavior-preserving unless the task requests a behavior change.
- Follow existing Rust formatting and naming conventions; let `rustfmt` format
  Rust files.
- Prefer typed configuration changes through the structures in `config.rs`
  and `settings_editor.rs`; update `config.example.json`, schemas, UI forms,
  and tests together when adding a user-facing setting.
- Keep action registration, keybindings, command-palette availability, and
  settings UI behavior synchronized when adding or renaming actions.
- Resolve accelerator labels from the effective keybinding at render time; do
  not hardcode them, because users can remap actions in their keymap.
- When adding a command-line flag, provide and document both a long form and
  a non-conflicting short form; update shell completions and parser tests.
- Preserve cross-platform behavior. Avoid assuming Unix paths, shells, or
  environment variables in shared code.
- Add focused regression tests for bug fixes and boundary-condition tests for
  pane layouts, WSL path handling, configuration parsing, and keybindings.
- Avoid broad dependency or `Cargo.lock` updates unless required by the task.
- Update `README.md` and example configuration/keymap files when user-visible
  behavior, installation steps, or defaults change.
- Gate platforms and features at the `mod` declaration rather than on every
  item inside a module, so a module compiled only under one feature/platform
  doesn't need to respell that predicate throughout its body.
- Prefer the `cfg` aliases `build.rs` emits over respelling a repeated
  platform/feature predicate: `linux_like` for
  `any(target_os = "linux", target_os = "freebsd")`, `servers_enabled` for
  `any(feature = "http-server", feature = "tftp-server")`, `tftp_enabled` for
  `any(feature = "tftp-server", feature = "tftp-client")`, `byte_stream_panes`
  for `any(feature = "serial-console", feature = "http-server", feature =
  "tftp-server")`, and `cli_services` for "any CLI service feature is
  enabled". Add a new alias in `build.rs`
  (with a matching `cargo::rustc-check-cfg` line) rather than adding another
  ad hoc multi-clause predicate.
- Embedded non-Rust payloads (shell integration scripts, grammar queries) live
  in a data directory beside their module under `src/` (see
  `src/shell_integration/`, `src/grammar_extensions/`), loaded with
  `include_str!`/`include_bytes!` — not inline in Rust string literals, and
  not under `assets/`, which `ZettaEmbeddedAssets` embeds wholesale.

## Command line integration design

Always create both long and short command line arguments. Expose only the long
versions in autocomplete to declutter the completion interface and aid with
readability.

For the short command line arguments prefer lowercase version. If there is a
conflict, prioritise the lowercase version for the more commonly used arguments
such as mandatory arguments and reserve upper case versions for optional
arguments.

Every subcommand must have a help section that describes how to use the CLI. Do
not assume that the user knows everything, so if an argument accepts input that
is only known at runtime, such as auto detected profiles, list these explicitly
and offer them in the tab auto-complete for their respective command line
argument. Arguments that depend on both runtime knowledge and a specific state,
such as a serial console emulator being plugged, in must offer a way to
dynamically enumerate these values via CLI and offer these via auto complete.
