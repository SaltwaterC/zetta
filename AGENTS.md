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

`docs/ARCHITECTURE.md` is the routing table for `src/`: which module owns what.
Consult it to place a change, but treat each module's own `//!` documentation as
authoritative — several record constraints that are not visible from the code.

## Non-negotiable

Everything below this section is guidance you may apply with judgement. These
eight are not, because each is expensive or impossible to undo:

1. **Never edit `zed/` or `busy-v/`.** They are upstream. A change to a `zed/`
   crate is made by forking it under `crates/` (see "Repository boundaries").
2. **Never `pkill zetta` or kill a process you did not start.** Zetta is
   self-hosting: the editor running this session is this repository's binary.
3. **Cache a sibling of what changes, never an ancestor of it** (see "Render
   boundaries"). Getting this backwards silently disables every cache below.
4. **Gate platforms and features at the `mod` declaration**, and prefer the
   `cfg` aliases `build.rs` emits over respelling a predicate.
5. **Suppress lints with `#[expect(..)]`, not `#[allow(..)]`**, except where a
   lint fires only on some platform or feature arm.
6. **Take a `Mutex` with `unwrap_or_else(|poisoned| poisoned.into_inner())`**
   unless the mutex guards an invariant a panic can break — then `unwrap` and
   say so at the lock.
7. **Preserve unrelated working-tree changes.** Do not rewrite or clean files
   outside the requested scope.
8. **Do not run `make install`, uninstall targets, or system-cache refresh
   targets as validation.** They mutate the host system.

## Repository boundaries

- Treat `zed/` and `busy-v/` as upstream code. Do not modify them unless the
  task explicitly requires an upstream dependency change.
- Code under `crates/` is maintained as part of Zetta and may be changed when
  the application needs corresponding terminal or platform behavior.
- `crates/zetta_profiles` holds the half of the configuration that answers
  "what does this profile run *here*": the shell discovery, the profile entries
  of `config.json`, the environment every Zetta pty is given, and the shell
  integration line. It exists because two processes start Zetta's ptys — the
  application, and `zmux` when a pane is added to a shared session — and the one
  that starts a pane is the one that has to resolve it. It must stay free of
  `zed/` and `gpui` dependencies so the daemon can link it; icons, themes and
  everything else that needs a window stay in `src/config.rs`, which layers them
  back on.
- A change to a `zed/` crate is made by forking it under `crates/`, never by
  editing the submodule. Cargo resolves a dependency edge from the manifest that
  declares it, so forking a crate that other `zed/` crates depend on means
  forking those too — `gpui` needed twenty-two of them. Those carry no patches
  and exist only to route the edge; `crates/UPSTREAM_AUDIT.md` lists them under
  "Routing-only forks" and each says so in its own `UPSTREAM.md`. Do not add
  behavior to a routing-only fork, and do not try to avoid one with a
  `.cargo/config.toml` path override: that was tried, it silently rerouted
  `gpui_platform` to the upstream copy, and Cargo documents it as slated to
  become a hard error.
- Keep platform-specific behavior behind the existing `cfg` boundaries. Linux
  defaults to Wayland; the `x11` feature enables the X11 backend.

## Code organisation

Keep `src/main.rs` limited to crate wiring, actions, shared imports/constants,
and the process entry point. Put behavior in the module that owns it —
`docs/ARCHITECTURE.md` says which. Prefer extending an existing module over
growing `main.rs`. If a module becomes difficult to navigate, split it by
responsibility rather than creating a generic helpers module. Keep rendering
code separate from state transitions where practical.

New modules carry a `//!` doc saying what they own and, where it is not obvious,
why they exist separately. That doc is what the next reader routes from.

**Size.** Split a module by responsibility once it approaches roughly 1500
lines. Keep functions under 200 lines — most are far below it. Both are rules
for new and edited code, not descriptions of the tree: nine files and six
production functions currently exceed them (largest: `terminal_spawn.rs` at 2976
lines and its `spawn_shared_terminal_now` at 336). Those are backlog, not
precedent — do not cite them to justify a new one.

Never let a single `render` function or top-level function own an entire screen
or CLI surface. Splitting a long function is only worth doing when the pieces
get smaller: moving nine tenths of a body into one new function leaves the same
function under a new name. The shapes that get a function over the limit, and
what each wants instead:

- **A render tree.** One builder per section, taking a `Copy` context bundle;
  see `pane_overlay.rs`'s `OverlayPickerContext` or `settings_view.rs`'s
  `SessionPromptView`. A section that differs from its neighbour only in a
  label or a target is a table, not a copy — `SETTINGS_PAGE_TABS`.
- **A `match` dispatch.** Keep the match exhaustive and make every arm one
  line, either by extracting the fat arms into named methods
  (`activate_settings_control`) or by grouping the arms into per-family
  functions the dispatcher routes to (`process_control/decode.rs` and
  `process_control/server.rs`, which use the same five groups so a new command
  is added to the same-named function in both).
- **A phased state transition.** One method per phase, in the order they run,
  with the ordering constraints stated: `submit_session_authentication`,
  `apply_pane_split_template_with_profile`.
- **A closure awaited off the render path.** Bundle what has to cross into it
  and give the callback a name; see `SpawnedTerminal` in `terminal_spawn.rs`.

Where a phase returns "nothing happened" as distinct from "succeeded", say so
in the type rather than with an early `return` the caller cannot see —
`apply_pane_template_control` returns `Result<Option<()>>` for exactly that,
and `a_control_this_page_does_not_own_leaves_the_form_untouched` pins it.

Where a section needs more than about seven values, give it a borrowed `Copy`
bundle rather than a longer parameter list — `PaneLayoutContext`, `PageWidgets`,
`PaneNodeContext` and `KeymapBindingRow` are the existing ones.

## Lints

`Cargo.toml`'s `[lints.clippy]` denies four pedantic lints the tree is clean
of, so they cannot come back one call site at a time. The section records which
pedantic lints were considered and deliberately *not* adopted, with the reason
for each; read it before adding another, and add the reason there rather than
sprinkling `#[allow]`s.

Suppress a lint with `#[expect(<lint>, reason = "...")]`, not `#[allow(..)]`.
An `expect` is reported by `unfulfilled_lint_expectations` once the lint stops
firing, so a suppression cannot outlive what it was for; an `allow` that has
become unnecessary is invisible and accumulates. Three
`#[allow(clippy::too_many_arguments)]` in this tree were found suppressing
nothing at all. Use `allow` only where the lint genuinely fires sometimes and
not others across the platform or feature matrix, and say which in the reason.
`cfg_attr`-gated suppressions stay `allow`, because whether they fire is exactly
what the `cfg` decides.

`clippy::too_many_arguments` is not a suppression of first resort. Reach for the
borrowed `Copy` bundle described above first, and check whether the bundle
already exists — `PaneLayoutContext`, `PageWidgets`, `MuxPaneIds`,
`TabBodyCorners` and `TabCorners` all had call sites still passing their
contents one at a time. Suppress only once the remaining arguments are genuinely
independent, and note in the reason why they cannot be bundled. The threshold
counts `self`, `&mut Window` and `&mut Context<Self>`, so a GPUI method with
five real parameters trips it; that is a legitimate reason, and naming it is the
point.

`#[allow(dead_code)]` is a last resort. If only tests reach an item, say
`#[cfg(test)]`; if only one platform constructs it, say
`#[cfg_attr(not(windows), allow(dead_code))]` and why. A bare allow on
something nothing reaches at all means the item should be deleted.

`make lint` lints only the host's `cfg` arms and the host's feature set, so the
denied lints above are *not* verified anywhere else by it — twelve violations of
them sat in `#[cfg(windows)]` arms because no local run ever compiled those
arms. `make clippy-linux`, `clippy-windows`, `clippy-macos`, `clippy-features`
and `clippy-platforms` run the matching `check-*` target's exact command with
`clippy -- -D warnings` instead, which also fails an `#[expect(..)]` that has
stopped firing on that platform. Run the one covering what you changed.

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

Add focused regression tests for bug fixes and boundary-condition tests for
pane layouts, WSL path handling, configuration parsing, and keybindings.

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

Match the checks to what the change touches; run from the repository root.
Each row includes the rows above it.

| Change | Run |
| --- | --- |
| Docs, comments, non-Rust assets | `cargo fmt --all --check`, `git diff --check` |
| One module, no signature change | `cargo test <filter>` |
| Several modules, signatures, config | `cargo check && cargo test` |
| `cfg`- or feature-gated code | `make check-features`, plus the platform targets below |
| Anything under `crates/` | that crate's own suite (below) |
| Render, input, or another hot path | the performance review below |

Zetta has no library target, so do not use `cargo test --lib` (including for
focused tests). Run a focused test with its filter against the binary target,
for example `cargo test pane_controls`.

`make check-features` is what covers Linux platform selection and the CLI
services (`cli_services.rs`/`tftp.rs` and their gating): it builds a
combination with no CLI service enabled, which is the only thing that exercises
every `cli_services`/`servers_enabled`/`tftp_enabled` gate.

Run `cargo clippy --all-targets` for broader Rust changes when practical.

**Crates.** The crates under `crates/` are their own Cargo workspaces, so a root
`cargo test` does **not** run their tests and `cargo test -p <crate>` refuses:

```sh
(cd crates/alacritty_terminal && cargo test)
(cd crates/terminal && cargo test)
(cd crates/zetta_profiles && cargo test)
(cd crates/zmux && cargo build --bin zmux && cargo test)
```

`cargo build --bin zmux` first is not optional there: those tests start the
binary rather than linking it, so a stale one silently tests the previous
implementation — which is how an assertion becomes vacuous without anyone
noticing. Note also that `crates/zmux` builds a `zmux` binary of its own, while
the root package builds one from `src/bin/zmux.rs`. The one that actually runs
is the root's, because a client resolves the multiplexer beside its own
executable — so `crates/zmux`'s tests passing says nothing about the binary a
user runs.

The root's own `tests/` directory holds the one thing no unit test can reach:
a remote pane carried over Mosh, end to end. It is ignored by default:

```sh
cargo build --bin zmux --bin zosh-server
cargo test --test zosh_pane -- --ignored
```

Cargo serialises on the target-directory lock, so `make build` run alongside
`make test` blocks until the tests finish rather than building immediately.
Before trusting a manual run of `target/debug/zetta` or `target/debug/zmux`,
check the binary is newer than the sources — `cargo check` and `cargo test` do
not refresh either one.

`make build` produces the release artifact and is only necessary for release,
packaging, or installation work.

### Checking every platform

Zetta is developed from Linux, macOS and Windows, and a local `cargo test`
compiles only the host's `cfg` arms. A change to `#[cfg(windows)]` or
`#[cfg(target_os = "macos")]` code can therefore pass every check on one
machine and still fail to build on another. One target per platform:

```sh
make check-linux      # native on Linux, else x86_64-unknown-linux-gnu
make check-windows    # native on Windows, else x86_64-pc-windows-gnu
make check-macos      # native on macOS, else x86_64-apple-darwin
make check-platforms  # check-features plus each platform this machine can check
```

Each checks natively when it *is* the host and cross-checks otherwise. All pass
`--all-targets`, so the tests behind those `cfg`s are compiled as well — a plain
`cargo check` skips them, which is how test code that does not build under a
feature combination goes unnoticed. They check, they do not link or run: a green
`make check-windows` is not a green Windows test suite.

A cross check needs more than the Rust target: `aws-lc-sys`, `ring`,
`tree-sitter` and `wasmtime` all compile C or assembly against the target's own
headers, so there has to be a C toolchain that can produce them. **Each target
probes for one and prints what to install** rather than failing several screens
into a build script, so run it and read the message instead of guessing.
`make check-platforms` skips a platform it has no toolchain for, but still fails
one it can check. Set `CC_<target with underscores>` (for example
`CC_x86_64_apple_darwin`) to point at a toolchain that is not on `PATH` under
its usual name.

Cross-checking **macOS** from Linux needs osxcross with an Apple SDK, and the
Rust target alone cannot be made to work: a Linux `cc` rejects
`-arch`/`-mmacosx-version-min` outright, and a bare clang gets past that only to
fall back to `/usr/include` and fail on glibc headers. Do not spend time on it
without the SDK.

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

A layer costs an insert even when it is nested inside another, so a *count* of
layers matters as much as a count of primitives. `paint_batched_text_runs`
covers the other half of the terminal's grid the same way `paint_grid_layer`
covers its backgrounds: it paints every shaped run of a pane inside one layer,
through `ShapedLine::paint_in_layer` rather than `paint`, taking a text screen
from 336.8 layers and 376.2 tree inserts per frame to 40.8 and 79.9. That is
order-preserving rather than a trade, and the reason is worth knowing before
collapsing anything else: `Bounds::intersects` is strict about a shared edge, so
runs tiling a grid never intersected one another and had all resolved to the
same order already — pinned by
`grid_tiled_bounds_that_only_touch_share_one_order` in `crates/gpui`. Collapsing
elements that genuinely overlap is a different question, because the sprite
vectors sort unstably within an order.

## Performance profiling

When a change touches a hot path — render, input, algorithmic scaling,
allocation in a loop, repeated I/O or process spawning, or locking — review the
completed diff for its performance impact, and profile or benchmark when static
review is not enough to establish it. Record any material findings and address
them when the task includes implementation. Changes that touch none of those do
not need a performance pass, and should not narrate one.

Use the built-in terminal-rendering workload for reproducible checks on Linux,
macOS, and Windows. Always use an optimized build when recording or comparing:

```sh
cargo run --release -- \
  benchmark \
  --profile-report artifacts/zetta-performance.json \
  --profile-duration 10
```

`--profile-report` enables an automated timed run and defaults to ten seconds
when `--profile-duration` is omitted. The command creates missing report parent
directories, writes versioned JSON, and exits. Treat a non-zero exit status or
a missing report as a failed performance run. Preserve the JSON as a CI
artifact and compare like-for-like release builds, workload settings, and
platforms. Use the live `zetta benchmark` mode without report arguments for
interactive investigation.

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
- Preserve cross-platform behavior. Avoid assuming Unix paths, shells, or
  environment variables in shared code.
- Avoid broad dependency or `Cargo.lock` updates unless required by the task.
- Update `README.md` and example configuration/keymap files when user-visible
  behavior, installation steps, or defaults change.
- Prefer the `cfg` aliases `build.rs` emits over respelling a repeated
  platform/feature predicate: `linux_like` for
  `any(target_os = "linux", target_os = "freebsd")`, `servers_enabled` for
  `any(feature = "http-server", feature = "tftp-server")`, `tftp_enabled` for
  `any(feature = "tftp-server", feature = "tftp-client")`, `byte_stream_panes`
  for `any(feature = "serial-console", feature = "http-server", feature =
  "tftp-server")`, and `cli_services` for "any CLI service feature is
  enabled". Add a new alias in `build.rs` (with a matching
  `cargo::rustc-check-cfg` line) rather than adding another ad hoc multi-clause
  predicate.
- Embedded non-Rust payloads (shell integration scripts, grammar queries) live
  in a data directory beside their module under `src/` (see
  `src/shell_integration/`, `src/grammar_extensions/`), loaded with
  `include_str!`/`include_bytes!` — not inline in Rust string literals, and
  not under `assets/`, which `ZettaEmbeddedAssets` embeds wholesale.

## Command line integration design

Always create both long and short command line arguments. Expose only the long
versions in autocomplete to declutter the completion interface and aid with
readability. Update shell completions and parser tests when adding a flag.

For the short arguments prefer the lowercase version. If there is a conflict,
prioritise lowercase for the more commonly used arguments such as mandatory
ones, and reserve uppercase for optional arguments.

Every subcommand must have a help section that describes how to use the CLI. Do
not assume that the user knows everything, so if an argument accepts input that
is only known at runtime, such as auto detected profiles, list these explicitly
and offer them in the tab auto-complete for their respective command line
argument. Arguments that depend on both runtime knowledge and a specific state,
such as a serial console emulator being plugged in, must offer a way to
dynamically enumerate these values via CLI and offer these via auto complete.

### CLI help formatting

CLI help tables must use the shared `format_help_table` helper from
`startup/cli_help.rs`. The standalone `zmux` and `zwt` CLIs each use their own
crate-local copy of it, in `crates/zmux/src/lib.rs` and `crates/zwt/src/lib.rs`:
the crates under `crates/` are separate Cargo workspaces, so sharing one
implementation would mean publishing a crate to hold twelve lines of string
padding. The three copies are deliberate and must stay identical in behaviour —
each carries the same test. Store option or command labels and descriptions
separately instead of embedding manual padding. The formatter computes the
longest label per table, uses a two-space separator, aligns multiline
continuation text, and emits no trailing whitespace.

This convention applies to `Commands`, `Operations`, `Options`, and equivalent
sections across maintained Rust CLI help. Do not manually count spaces or use
chained string replacements to insert help rows. Help-content tests should
verify semantic content and alignment without depending on fragile,
hand-counted padding. Shell-completion descriptions and upstream `zed/` and
`busy-v/` code are outside this convention.
