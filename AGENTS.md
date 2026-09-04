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
and the process entry point. Put behavior in the module that owns it. The
groups below are a routing table, not a layering rule: every module named here
is a sibling under `src/`.

### Application state and lifecycle

- `app.rs`: `Zetta` struct, tab/pane lifecycle, and state that doesn't belong
  to a narrower module below. A module directory — the root owns the struct,
  the window's construction/resume/close, and the free predicates its actions
  decide from; the actions are grouped by what they act on: `app/tabs.rs`
  (open, close, pin, order, tab move mode), `app/panes.rs` (split, close,
  focus, broadcast, and what a terminal's exit does to its pane),
  `app/pane_templates.rs` (applying a pane template, and `--replace-pane`),
  `app/window_actions.rs` (window actions and application-menu navigation),
  and `app/attention.rs` (routing an attention ID to the tab that owns it)
- `startup.rs`: `run()`'s startup-mode dispatch, the handoff-to-a-running-process
  sequence, the GUI launch (`ApplicationLaunch`), window/process lifecycle, and
  theme resolution (`resolve_profile_theme`); a module directory —
  `startup/cli_help.rs` (usage/help text), `startup/arg_parsing.rs`
  (`StartupMode`/`StartupArgs` parsing — itself a module directory: the root
  holds the types, `StartupArgs::for_mode`, the plain launch's option loop and
  `parse_subcommand`'s dispatch, with one parser per subcommand in
  `startup/arg_parsing/subcommands.rs` and
  `startup/arg_parsing/benchmark.rs`), `startup/cli_modes.rs` (the
  subcommands that never open a window — one function per `StartupMode`
  variant), `startup/process_control_loop.rs` (the event loop that applies
  `ProcessControlCommand`s, one handler per command),
  `startup/keybindings.rs` (keybinding constants/constructors and macOS
  native menu construction), `startup/window.rs` (opening, tracking and
  closing this process's windows, and the quit policy that follows the last
  close), `startup/watchers.rs` (the configuration/keymap and session-catalog
  pollers, which read only when a file's stamp changes),
  `startup/theming.rs` (theme loading, the baked Zetta overrides, and
  keymap normalization/validation), `startup/workload.rs` (the deterministic
  producer workloads `zetta benchmark` drives the renderer with), and
  `startup/wsl.rs` (WSL/MSYS2 profile and working-directory integration)

  `run()` dispatches with one exhaustive `match` over `StartupMode`, so a new
  variant has to name the function that handles it rather than silently
  falling through to a GUI launch. Add the arm there, not another sequential
  test.
- `pane.rs`: pane layout, tab models, terminal creation, and pane focus. A
  module directory — the root owns `TerminalPane` and the settings a spawn is
  made with; `pane/layout.rs` (the `PaneLayout` split tree and every operation
  that reshapes it), `pane/tab.rs` (`Tab`, and the maximize/minimize/focus
  state that changes what it shows without changing the tree),
  `pane/stack.rs` (`PaneStack`), and `pane/overlay.rs` (the overlay text,
  font-size steps, and the colour model the style picker edits). The root
  re-exports all four, so the rest of the crate still names them
  `crate::pane::…`
- `pane_resize.rs`: pane resize/move mode, keyboard and drag-based resizing
- `pane_view_state.rs`: pane maximize/minimize/restore and font size
- `pane_controls.rs`: per-pane control visibility and its idle timer
- `stacked_panes.rs`: command terminals that share a pane's layout region
  (`PaneStack` entry lifecycle)
- `rename.rs`: tab and pane rename state
- `terminal_spawn.rs`: terminal process spawning and its event wiring
- `default_terminal.rs`: registering and detecting Zetta as the system's
  default terminal, and the desktop-environment detection that needs
- `configuration_reload.rs`: settings/keymap file editing and configuration
  reload
- `view_boundary.rs`: `ZettaSubview`, the entity wrapper that lets part of the
  render tree be cached and be the target of its own scroll/hover
  notifications; see "Render boundaries" below

### Rendering

- `app_render.rs`: top-level `Render for Zetta` composition (action
  registration, overlay collection, and the tab-icon-picker/overlay-style-picker
  overlays); delegates to `title_bar_render.rs` and `tab_body_render.rs`
- `title_bar_render.rs`: title bar composition, its menus (application,
  profile, reconnect), the layout predicates the bar shares with the tab bar,
  and `title_bar_chrome_height`, which sizes the cached chrome boundary
- `tab_bar_render.rs`: the measured tab row, individual tabs, and the bar
  that hosts them
- `tab_body_render.rs`: tab body composition (maximized-pane bar, minimized
  pane shelf, pane content wiring)
- `pane_render.rs`: pane layout and resize-gutter rendering
- `window_frame.rs`: window decorations (`WindowFrameGeometry`), window
  controls, and resize edges
- `performance.rs`: frame collection, performance metrics, and the
  performance overlay

### Overlays, pickers, and prompts

- `text_edit.rs`: the single-line field every text field and picker query is
  built from — `TextField` itself, the char-boundary cursor arithmetic, the
  editing keys (`apply_text_field_key`) and the clipboard chords
  (`apply_clipboard_shortcut`). A surface holds a `TextField` and keeps only
  the keys that are its own; see the module docs for the two that deliberately
  do not
- `text_edit_ui.rs`: the rendering half of a field — the caret, the inline
  query run the overlays share, and the bordered frame the boxed fields share
- `tab_search.rs`: cross-pane scrollback search and its overlay
- `tab_icon_picker.rs`: tab icon picker model, rendering, and the
  `Zetta` methods that drive it
- `pane_theme_picker.rs`: per-pane theme picker model and overlay
- `pane_overlay.rs`: per-pane overlay text and style picker
- `command_palette.rs`: palette model and matching, including
  `CommandPalette::apply_key` — the list half of a picker's key handling
  (arrows, `enter`, and re-filtering as the query changes), shared by the
  command palette and the theme picker so the two do not drift. `escape` and
  what `enter` runs stay with each surface, because those are what differ
- `command_palette_ui.rs`: palette interaction, rendering, and its overlay
- `multi_command.rs`: the multi-command prompt's model, its completion
  catalog, and completion-context parsing
- `multi_command_ui.rs`: multi-command prompt interaction, rendering, and its
  overlay
- `close_confirmation_ui.rs`: the pinned-tab close confirmation
- `session_auth_ui.rs`: the session passphrase/secret prompt, its field model,
  and the protect/reconnect flows that submit it
- `remote_session_ui.rs`: the remote-session picker; SSH discovery is kept off
  the render path deliberately, see the module docs
- `serial_console_ui.rs`, `http_server_ui.rs`, `tftp_server_ui.rs`: the
  per-service prompts that open a byte-stream pane
- `server_ui.rs`: `ServerRoot` resolution shared by the HTTP and TFTP prompts,
  including the WSL path translation

### Settings

- `settings_editor.rs`: typed configuration/keymap forms and persistence. A
  module directory — `settings_editor/configuration.rs` (the Configuration
  page's form, built over the file's parsed root so unknown keys survive a
  round trip), `settings_editor/keymap.rs` (the Keymap page's form, merged with
  the default template and stripped back to what was rebound), and
  `settings_editor/pane_templates.rs` (`PaneTemplatesForm`, which overlays
  either the built-in presets (the user configuration) or the resolved user
  configuration (a project)). The root re-exports all three
- `project_form.rs`: the typed form for a project's `.zetta/config.json` and its
  serialization; every field is optional because the file is an overlay
- `settings_ui.rs`: settings state and event handling; a module directory —
  `settings_ui/keymap.rs` (capture, search cache), `settings_ui/controls.rs`
  (the control list and focus/scroll navigation),
  `settings_ui/dropdowns.rs` (what a dropdown offers and what choosing an
  option does), `settings_ui/editing.rs` (activating a control, text input,
  toggles, sliders, and the numeric fields that repeat while held),
  `settings_ui/pane_templates.rs` (pane-template state, and the `templates`
  accessors that decide which form the editor edits),
  `settings_ui/projects.rs` (project registry actions and the project
  configuration builder's state), and `settings_ui/theme_extensions_ui.rs`
  (fetch/download/remove)
- `settings_view.rs`: settings rendering; a module directory —
  `settings_view/pages.rs` (per-`SettingsPage` content),
  `settings_view/modals.rs` (font/profile/keymap-capture modals),
  `settings_view/pane_templates.rs` (template list, layout preview, and node
  details), `settings_view/projects.rs` (project list and configuration
  builder),
  `settings_view/form_widgets.rs` (the form's shared controls, held in a struct
  so the page and the modals can be built in separate passes), and
  `settings_view/widgets.rs` (shared widget building blocks, including the
  `action_button`/`control_row`/`text_field`/`dropdown_field` helpers the denser
  forms share)

### Configuration, profiles, and projects

- `config.rs`: the typed `Config`/`Profile` model, its parsing, and the
  overlay rules project configuration layers on top of it. The file's own shape
  is the `ConfigFile`/`SessionsFile`/`ProfileFile` deserialization mirror, so
  `#[serde(deny_unknown_fields)]` is what rejects a misspelled setting and a
  setting's name is not restated in an allow-list. Fields are `Setting<T>`
  rather than `Option<T>` because serde would read an explicit `null` as
  "absent", and this format reports it as a type error. A module directory —
  `config/discovery.rs` holds the per-platform shell detection (Homebrew
  prefixes, the MSYS2 and Cygwin installation roots, WSL distributions, `PATH`
  resolution) that produces the profile set `Config::defaults` starts from
- `project.rs`: `ProjectConfig`, `ProjectRegistry`, and project field
  validation
- `project_context.rs`: the active project for a window, project detection for
  a directory, and the theme/profile resolution that follows from it
- `project_cli.rs`: `zetta project` argument parsing and its non-open commands
- `project_commands.rs`: registered project commands and their name/command/
  environment validation
- `profile_cli.rs`: `zetta profile` argument parsing and command results
- `profile_icon.rs`: `ProfileIcon`, automatic icon selection for a program,
  and executable icon extraction
- `theme_extensions.rs`: theme-extension discovery and installation

### Sessions, multiplexing, and process control

- `background_sessions.rs`: the application's half of background sessions —
  the runner, the catalog directory, and the parts that need GPUI; the schema,
  verifier and publisher live in the `zmux` crate
- `background_session_ui.rs`: background-session detach/store/reconnect and
  the reconnect picker; shared-mode panes (the `SharedPaneEntry` registry,
  arbitrated-size application, shared exit routing, and the revoke handover
  that converts an exclusive pane to shared). A module directory — the root
  holds the state and predicates the transitions share;
  `background_session_ui/detach.rs` (detach, protect, share, store),
  `background_session_ui/reconnect.rs` (taking a session back, and its
  authentication), `background_session_ui/restore.rs` (rebuilding the tab a
  returned session becomes, and the picker entries),
  `background_session_ui/observers.rs` (what a window watches on a background
  pane, and the catalog it publishes),
  `background_session_ui/multiplexer.rs` (handing a session to `zmux` and
  attaching one from it), and `background_session_ui/shared_panes.rs`
- `session_state.rs`: a tab as the multiplexer stores it — the opaque durable
  blob `zmux` round-trips without reading; see the module docs before adding a
  durable tab feature
- `session_auto_protect.rs`: the automatic-protection policy for stored
  sessions
- `mux.rs`: `MuxRuntime`, the `zmux` client connection shared by every pane in
  the process, and its retention/recovery state
- `mux_identity.rs`: identity-file resolution for multiplexer commands
- `process_control.rs`: the per-process control socket. Every
  `zetta <subcommand>` that has to reach a running window goes through here;
  the decoded request is applied by `startup/process_control_loop.rs`. A module
  directory — the root owns the wire format (`ControlRequest`,
  `ControlResponse`, their payload types, the decoded `ControlRequestCommand`,
  and the `CONTROL_VERSION` history), which stays there rather than in a
  submodule so the four halves can read its private fields:
  `process_control/server.rs` (`ProcessControlServer`, the listener thread and
  the completion waits), `process_control/decode.rs` (`decode_control_request`,
  which is what enforces the fields each command may carry),
  `process_control/client.rs` (one function per subcommand that reaches a
  window, all sent through `send_control_request`), and
  `process_control/endpoint.rs` (endpoint discovery, publication, and the
  dead-process reaping)
- `run_command.rs`: the `zetta pane wait` registry shared by wrapper clients
  and terminal lifecycle events; deliberately GPUI-free
- `command_panes.rs`: `PaneCommand`/`ShellCommandRequest` and the pane-opening
  side of `zetta pane` and registered project commands
- `silent_mode.rs`: silent-mode state, the system do-not-disturb query, and
  `FocusStatusAccess`

### CLI services and servers

- `cli_services.rs`: CLI service dispatch; a module directory —
  `cli_services/serial.rs`, `cli_services/servers.rs` (HTTP + TFTP server),
  `cli_services/notify.rs`, `cli_services/clipboard.rs`, and
  `cli_services/raw_terminal.rs`
- `cli_service_stubs.rs`: disabled-build fallbacks for CLI-service actions
- `byte_stream_pane.rs`: shared pane opener for byte-stream-backed panes
  (HTTP/TFTP server log panes, the serial console)
- `http_server.rs`: the embedded HTTP file server and the log stream its pane
  reads
- `tftp.rs`: shared TFTP packet/opcode types; a module directory —
  `tftp/server.rs` and `tftp/client.rs`
- `serial_console.rs`: serial device detection and the serial field model
- `notification_sounds.rs`: built-in notification sounds and their synthesis
- `output_benchmark.rs`: the `zetta benchmark output` workloads and results

### Platform and shell integration

- `windows_integration.rs`: the Windows Terminal handoff ABI and console
  handover; gated to Windows at its `mod` declaration
- `linux_desktop.rs`: the managed user desktop entry and its profile actions
- `shell_integration.rs`: the shell integration scripts, their placeholder
  substitution (including the completion trees), and shell detection
- `worktree_detection.rs`: linked-worktree detection for a pane's shell
  directory

### Viewers and assets

- `vi_syntax.rs`: grammar loading and syntax highlighting for `zetta vi`; see
  "Performance profiling" before changing when grammars are compiled
- `zetta_assets.rs`: embedded assets

Prefer extending these modules over growing `main.rs`. If a module becomes
difficult to navigate, split it by responsibility rather than creating a
generic helpers module. Keep rendering code separate from state transitions
where practical.

Every keyboard-reachable settings control has to *show* focus and has to be
scrolled into view, or the page is unusable from the keyboard even though its
tab order is complete. Rows carry that: `control_row` (and `setting_row` on the
Configuration page) highlight while any control they host is focused, because a
text field's or dropdown's own ring is a one-pixel border and a switch has none.
Anything focusable that is not in a row — a list row, a preview node, a bare
button — needs its own focus treatment. `scroll_settings_control_into_view` only
estimates the offset from a control's position in the tab order;
`widgets::track_focus_scroll` finishes it from the element's laid-out bounds, so
new focusable elements should be wrapped in it (a plain `overflow_y_scroll` div
honours neither `Window::request_autoscroll` nor `ScrollHandle::scroll_to_item`).

Prefer splitting a module by responsibility once it approaches roughly 1500
lines rather than letting it keep growing. Never let a single `render`
function or method own an entire screen or top-level function own an entire
CLI/state surface — extract per-section methods or functions (passing in
already-computed values instead of recomputing them) once that function is
hard to scan in one pass. Splitting a long function is only done when the
pieces get smaller: moving nine tenths of a body into one new function leaves
the same function under a new name.

**No function in `src/` is over 200 lines**, and that is the ceiling to keep,
not a target to aim at — most are far below it. The shapes that get a function
there, and what each wants instead:

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
bundle rather than a longer parameter list or an
`#[allow(clippy::too_many_arguments)]` — `PaneLayoutContext`, `PageWidgets`,
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
borrowed `Copy` bundle described under "Application architecture" first, and
check whether the bundle already exists — `PaneLayoutContext`, `PageWidgets`,
`MuxPaneIds`, `TabBodyCorners` and `TabCorners` all had call sites still passing
their contents one at a time. Suppress only once the remaining arguments are
genuinely independent, and note in the reason why they cannot be bundled. The
threshold counts `self`, `&mut Window` and `&mut Context<Self>`, so a GPUI
method with five real parameters trips it; that is a legitimate reason, and
naming it is the point.

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
stopped firing on that platform. Run the one covering what you changed;
`make clippy-platforms` covers every platform this machine has a toolchain for.

Take a `Mutex` with `unwrap_or_else(|poisoned| poisoned.into_inner())` rather
than `unwrap`. Every mutex in the crate guards a slot that is replaced whole on
each write, so a panic elsewhere leaves the value usable and poisoning would
only spread one window's failure to another. A mutex that does guard an
invariant a panic can break should use `unwrap` and say so at the lock.

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

The crates under `crates/` are their own Cargo workspaces, so a root
`cargo test` does **not** run their tests and `cargo test -p <crate>` refuses.
Anything touching them has to be validated from the crate's own directory, and
`crates/zmux`'s integration tests drive a separately built binary:

```sh
(cd crates/alacritty_terminal && cargo test)
(cd crates/terminal && cargo test)
(cd crates/zmux && cargo build --bin zmux && cargo test)
```

`cargo build --bin zmux` first is not optional there: those tests start the
binary rather than linking it, so a stale one silently tests the previous
implementation — which is how an assertion becomes vacuous without anyone
noticing.

Note also that `crates/zmux` builds a `zmux` binary of its own, while the root
package builds one from `src/bin/zmux.rs`. The one that actually runs is the
root's, because a client resolves the multiplexer beside its own executable —
so `crates/zmux`'s tests passing says nothing about the binary a user runs.

Cargo serialises on the target-directory lock, so `make build` run alongside
`make test` blocks until the tests finish rather than building immediately.
Before trusting a manual run of `target/debug/zetta` or `target/debug/zmux`,
check the binary is newer than the sources — `cargo check` and `cargo test` do
not refresh either one.

Run Clippy for broader Rust changes when practical:

```sh
cargo clippy --all-targets
```

For changes touching Linux platform selection, or a CLI service
(`cli_services.rs`/`tftp.rs` and their gating), check the feature combinations
too — the second exercises every `cli_services`/`servers_enabled`/
`tftp_enabled` gate, since it has no CLI service enabled:

```sh
make check-features
```

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

Each checks natively when it *is* the host and cross-checks otherwise, so which
of them is the cheap one depends on where you are sitting. All pass
`--all-targets`, so the tests behind those `cfg`s are compiled as well — a
plain `cargo check` skips them, which is how test code that does not build
under a feature combination goes unnoticed.

They check, they do not link or run. Running a platform's tests needs a machine
of that platform, so a green `make check-windows` is not a green Windows test
suite.

A cross check needs more than the Rust target: `aws-lc-sys`, `ring`,
`tree-sitter` and `wasmtime` all compile C or assembly against the target's own
headers, so there has to be a C toolchain that can produce them. Each target
probes for one and says what to install rather than failing several screens
into a build script. `make check-platforms` skips a platform it has no
toolchain for, but still fails one it can check.

- **Windows** needs MinGW-w64 (`x86_64-w64-mingw32-gcc`): a distribution
  `mingw-w64` package, or `brew install mingw-w64`.
- **Linux** from macOS or Windows needs a `x86_64-linux-gnu-gcc`; on macOS,
  `brew install messense/macos-cross-toolchains/x86_64-unknown-linux-gnu`. No
  Wayland or X11 development package is required, because `wayland-backend`
  is used with `dlopen` and `cargo check` does not link.
- **macOS** needs osxcross with an Apple SDK. The Rust target alone is not
  enough and cannot be made to work: a Linux `cc` rejects
  `-arch`/`-mmacosx-version-min` outright, and a bare clang gets past that only
  to fall back to `/usr/include` and fail on glibc headers.

Set `CC_<target with underscores>` (for example `CC_x86_64_apple_darwin`) to
point at a toolchain that is not on `PATH` under its usual name.

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

### CLI help formatting

CLI help tables must use the shared `format_help_table` helper from
`startup/cli_help.rs`. The standalone `zmux` and `zwt` CLIs each use their own
crate-local copy of it, in `crates/zmux/src/lib.rs` and `crates/zwt/src/lib.rs`:
the crates under `crates/` are separate Cargo workspaces, so sharing one
implementation would mean publishing a crate to hold twelve lines of string
padding. The three copies are deliberate and must stay identical in behaviour —
each carries the same test. Store option or command labels and descriptions separately
instead of embedding manual padding. The formatter computes the longest label
per table, uses a two-space separator, aligns multiline continuation text, and
emits no trailing whitespace.

This convention applies to `Commands`, `Operations`, `Options`, and equivalent
sections across maintained Rust CLI help. Do not manually count spaces or use
chained string replacements to insert help rows. Help-content tests should
verify semantic content and alignment without depending on fragile,
hand-counted padding. Shell-completion descriptions and upstream `zed/` and
`busy-v/` code are outside this convention.
