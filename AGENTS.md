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
- Code under `crates/` is maintained as part of Zetta and may be changed when
  the application needs corresponding terminal or platform behavior.
- Keep platform-specific behavior behind the existing `cfg` boundaries. Linux
  defaults to Wayland; the `x11` feature enables the X11 backend.
- Preserve unrelated working-tree changes. Do not rewrite or clean files that
  are outside the requested scope.

## Application architecture

Keep `src/main.rs` limited to crate wiring, actions, shared imports/constants,
and the process entry point. Put behavior in the module that owns it:

- `app.rs`: application state and core tab/pane orchestration
- `app_render.rs`: top-level application rendering
- `pane.rs`: pane layout, tab models, terminal creation, and pane focus
- `performance.rs`: frame collection and performance metrics
- `tab_search.rs`: cross-pane scrollback search
- `settings_editor.rs`: typed configuration/keymap forms and persistence
- `settings_ui.rs`: settings state and event handling
- `settings_view.rs`: settings rendering
- `command_palette.rs`: palette model and matching
- `command_palette_ui.rs`: palette interaction and rendering
- `window_frame.rs`: title bars, window controls, and resize edges
- `startup.rs`: argument parsing, startup configuration, themes, keybindings,
  profile launch directories, and WSL integration
- `theme_extensions.rs`: theme-extension discovery and installation
- `zetta_assets.rs`: embedded assets

Prefer extending these modules over growing `main.rs`. If a module becomes
difficult to navigate, split it by responsibility rather than creating a
generic helpers module. Keep rendering code separate from state transitions
where practical.

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

Remember that `include_str!` and `include_bytes!` paths are relative to the
file containing the macro; update such paths when moving tests or source.

## Validation

Use the smallest useful check while iterating, then validate the completed
change from the repository root:

```sh
cargo fmt --all --check
cargo check
cargo test
git diff --check
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

Do not run `make install`, uninstall targets, or system-cache refresh targets
as validation; they mutate the host system. `make build` produces the release
artifact and is only necessary for release, packaging, or installation work.

## Performance profiling

Use the built-in terminal-rendering workload for reproducible performance
checks on Linux, macOS, and Windows. Always use an optimized build when
recording or comparing results:

```sh
cargo run --release -- \
  --profile-terminal-rendering \
  --profile-report artifacts/zetta-performance.json \
  --profile-duration 10
```

`--profile-report` enables an automated timed run and defaults to ten seconds
when `--profile-duration` is omitted. The command creates missing report parent
directories, writes versioned JSON, and exits. Treat a non-zero exit status or
a missing report as a failed performance run. Preserve the JSON as a CI
artifact and compare like-for-like release builds, workload settings, and
platforms. Use the live `--profile-terminal-rendering` mode without report
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
- Preserve cross-platform behavior. Avoid assuming Unix paths, shells, or
  environment variables in shared code.
- Add focused regression tests for bug fixes and boundary-condition tests for
  pane layouts, WSL path handling, configuration parsing, and keybindings.
- Avoid broad dependency or `Cargo.lock` updates unless required by the task.
- Update `README.md` and example configuration/keymap files when user-visible
  behavior, installation steps, or defaults change.
