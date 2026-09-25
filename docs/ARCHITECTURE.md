# Architecture

A routing table for `src/`: which module owns what, so a change lands in the
module that owns the behaviour rather than in `main.rs`.

This document is a convenience, not the source of truth. **Every module carries
its own `//!` documentation, and that is authoritative.** Where this file and a
module doc disagree, the module doc is right and this file has drifted — fix it
here. Read the module doc before assuming a file's role; several of them record
constraints that are not visible from the code (`session_state.rs`,
`remote_pane_transport/`, `text_edit.rs`).

Keep `src/main.rs` limited to crate wiring, actions, shared imports/constants,
and the process entry point. The groups below are a routing table, not a
layering rule: every module named here is a sibling under `src/`.

## Application state and lifecycle

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
  notifications; see "Render boundaries" in `AGENTS.md`

## Rendering

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

## Overlays, pickers, and prompts

- `text_edit.rs`: the single-line field every text field and picker query is
  built from — `TextField` itself, the char-boundary cursor arithmetic, the
  editing keys (`apply_text_field_key`) and the clipboard chords
  (`apply_clipboard_shortcut`). A surface holds a `TextField` and keeps only
  the keys that are its own; see the module docs for the two that deliberately
  do not
- `text_edit_ui.rs`: the rendering half of a field — the caret, the inline
  query run the overlays share, and the bordered frame the boxed fields share
- `searchable_dropdown.rs`: the shared state, keyboard handling, and rendering
  for searchable dropdowns. The settings editor and the remote-session picker
  own different option lists and commit different values, but their dropdown
  interaction is the same, and lives here so the two cannot drift
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

## Settings

- `settings_editor.rs`: typed configuration/keymap forms and persistence. A
  module directory — `settings_editor/configuration.rs` (the Configuration
  page's form, built over the file's parsed root so unknown keys survive a
  round trip), `settings_editor/keymap.rs` (the Keymap page's form, merged with
  the default template and stripped back to what was rebound), and
  `settings_editor/pane_templates.rs` (`PaneTemplatesForm`, which overlays
  either the built-in presets (the user configuration) or the resolved user
  configuration (a project)). The root re-exports all three
- `keymap_file.rs`: Zetta's keymap file model, replacing Zed's
  `settings::KeymapFile`. That type is 2,800 lines built around Zed's keymap
  *editor* and reached the `fs` crate, which dragged Zed's git and SSH-askpass
  layers into a terminal emulator. Zetta uses four entry points from it and
  owns them here instead; the file format is unchanged
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

## Configuration, profiles, and projects

- `config.rs`: the typed `Config`/`Profile` model, its parsing, and the
  overlay rules project configuration layers on top of it. The file's own shape
  is the `ConfigFile`/`SessionsFile`/`ProfileFile` deserialization mirror, so
  `#[serde(deny_unknown_fields)]` is what rejects a misspelled setting and a
  setting's name is not restated in an allow-list. Fields are `Setting<T>`
  rather than `Option<T>` because serde would read an explicit `null` as
  "absent", and this format reports it as a type error. A module directory —
  `config/discovery.rs` turns what `zetta_profiles` detected into what the
  application shows, attaching the `ProfileIcon` and the `Shell` the spawn path
  is written against, and converting back to the plain command that crosses a
  machine boundary. The detection itself — Homebrew prefixes, the MSYS2 and
  Cygwin installation roots, WSL distributions, `PATH` resolution — lives in
  `crates/zetta_profiles`, because the daemon resolves the same names
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

## Sessions, multiplexing, and process control

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
  attaching one from it),
  `background_session_ui/collaboration.rs` (the window-side model for a
  daemon-owned shared session: `zmux` addresses panes by stable ids while every
  window has its own pane-id namespace, and keeping that translation here makes
  an incoming snapshot safe to apply even when another window published the
  session), `background_session_ui/shared_panes.rs`,
  `background_session_ui/image_paste.rs` (the conversion from GPUI clipboard
  images to the PNG payload `zmux` understands, and the choice of which handler
  a pane's terminal is built with; the terminal crate owns ordering and the
  native local shortcut), and `background_session_ui/zosh_panes.rs` (a remote
  pane whose bytes arrive over Mosh instead: what it registers, and what it
  deliberately does not)
- `session_state.rs`: a tab as the multiplexer stores it — the opaque durable
  blob `zmux` round-trips without reading; see the module docs before adding a
  durable tab feature
- `session_auto_protect.rs`: the automatic-protection policy for stored
  sessions
- `remote_pane_transport.rs`: what carries a remote session's *panes*, as
  distinct from its control traffic, which is always the SSH forward. A module
  directory — the root owns `RemotePaneTransport` and the concurrent bootstrap;
  `remote_pane_transport/zosh_stream.rs` brings up one `zosh-server` per pane
  in front of `zmux relay-pane` and renders it with a headless
  `zosh::PaneSession`. Read its module docs before changing what a Mosh pane
  does and does not register
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

## Remote sessions and image paste

- `mosh.rs`: the `zetta mosh` compatibility proxy and the Mosh launcher's
  option table. The full Mosh command belongs to the bundled `zosh`
  executable, and this process is a transparent handoff so `zosh` sees the same
  option spelling, target and remote command the user supplied. `parse_mosh_args`
  lives here rather than beside the other subcommand parsers because two callers
  need it and only one of them is a command line
- `ssh_image_paste.rs`: image paste for a foreground OpenSSH or Mosh process.
  A remote application cannot read the desktop clipboard, so this sends a PNG
  through a second, batch-mode SSH connection and pastes the resulting remote
  path. A Mosh session reaches the same problem differently: `zosh` is launcher
  and client in one process, so its argument vector still names the SSH command
  and target it bootstrapped through
- `image_paste.rs`: clipboard image validation and PNG normalization shared by
  the image stores

## Disabled-build counterparts

Each of these is the same surface as a feature-gated module, for a build
without that feature, so no call site needs a feature predicate:

- `mux_stub.rs`: no-`zmux` application shims. The terminal-spawn path is shared
  by both builds; here these values deliberately do nothing
- `local_sessions.rs`: the part of the background-session protocol a no-`zmux`
  build needs. Such a build still owns detached sessions in the Zetta process,
  so these types mirror the application-facing pieces of the shared protocol
- `session_state_stub.rs`: restore-command validation shared by local sessions
- `remote_session_ui_stub.rs`: the remote-session picker's disabled-build surface
- `cli_service_stubs.rs`: disabled-build fallbacks for CLI-service actions
- `remote_pane_transport/zosh_stream_disabled.rs`: the Mosh pane surface for a
  build without the bundled Zosh client, where the type has no values at all

## CLI services and servers

- `cli_services.rs`: CLI service dispatch; a module directory —
  `cli_services/serial.rs`, `cli_services/servers.rs` (HTTP + TFTP server),
  `cli_services/clipboard.rs` (validated proxy to sibling `zcopy` and `zpaste`),
  and `cli_services/raw_terminal.rs`. `crates/zclip` shares the clipboard option
  parser with Zetta and keeps `arboard` behind a binary-only backend feature.
  When enabled, the `notify` arm proxies to sibling `zntfy`; its backend and
  built-in sounds live in `crates/zntfy`
- `byte_stream_pane.rs`: shared pane opener for byte-stream-backed panes
  (HTTP/TFTP server log panes, the serial console)
- `http_server.rs`: the embedded HTTP file server and the log stream its pane
  reads
- `tftp.rs`: shared TFTP packet/opcode types; a module directory —
  `tftp/server.rs` and `tftp/client.rs`
- `serial_console.rs`: serial device detection and the serial field model
- `output_benchmark.rs`: the `zetta benchmark output` workloads and results

## Platform and shell integration

- `windows_integration.rs`: the Windows Terminal handoff ABI and console
  handover; gated to Windows at its `mod` declaration
- `linux_desktop.rs`: the managed user desktop entry and its profile actions
- `shell_integration.rs`: the shell integration scripts, their placeholder
  substitution (including the completion trees), and shell detection
- `worktree_detection.rs`: linked-worktree detection for a pane's shell
  directory

## Viewers and assets

- `vi_syntax.rs`: grammar loading and syntax highlighting for `zetta vi`; see
  "Performance profiling" in `AGENTS.md` before changing when grammars are
  compiled
- `zetta_assets.rs`: embedded assets

## Binaries

`src/bin/` holds the executables built alongside the application: `zmux.rs`
(the multiplexer a client resolves beside its own executable — this is the one
that runs, not `crates/zmux`'s), `zmux_pty.rs`, `zosh.rs`, `zwt.rs`, and
`zetta_gui.rs`.
