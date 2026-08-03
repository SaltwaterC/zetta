# Forked dependency upstream audit

Reviewed 2026-08-03 after synchronizing the Zed submodule. It is now pinned to
`849ec5898a321eefbeb1d1beda130cc50ef43f10` (2026-08-03), which is the current
upstream `main` used for this sync.
The Alacritty base used by Zetta, `4c129667ce56611becdc82de6e28218c80e2e88f`,
is still upstream `master`.

## Fork inventory

| Local fork | Upstream/base | Current retained change |
| --- | --- | --- |
| `crates/alacritty_terminal` | `zed-industries/alacritty@4c129667` | Hybrid bounded-memory scrollback, allocator/performance fixes, Windows ConPTY read and hangup handling, shell integration, and resize behavior. |
| `crates/terminal` | `zed/crates/terminal@849ec589` | Standalone terminal engine with Zetta identity, PTY/process-group/CWD tracking, shell integration, unbounded scrollback coordinates, input mapping, export/serial support, diagnostics, and performance work. |
| `crates/terminal_view` | `zed/crates/terminal_view@849ec589` | Standalone renderer and interaction model, independent cursor/text layout, pixel-snapped subcell block/sextant painting, themes and font overrides, pane controls, path targets, literal/asynchronous search, inline sizing, alternate-screen anchoring, and scrollback editing. |
| `crates/gpui_platform` | `zed/crates/gpui_platform@849ec589` | Local routing manifest selecting Zetta's Linux, macOS, and Windows platform forks. |
| `crates/gpui_linux` | `zed/crates/gpui_linux@849ec589` | Executor cap, keyboard and serial fixes, Wayland diagnostics and resize safety, X11 exposure repainting, Zenity fallback, and Zetta-specific platform behavior. |
| `crates/gpui_macos` | `zed/crates/gpui_macos@849ec589` | Input-source lifetime/context gating, keyboard-layout recovery, pasteboard lifetime safety, native menu/profile shortcuts, and related tests. |
| `crates/gpui_windows` | `zed/crates/gpui_windows@849ec589` | Correct maximize/restore toggle, DirectX scene annotations, one-shot attention flashing, inactive popup behavior, and input activation fixes. |

Per-fork synchronization notes live in each fork directory's `UPSTREAM.md`.
`target/` directories and license-only differences are not fork patches.

## Historical changes before the previous Zed pin

These are the complete changes touching the forked paths between the earlier
baseline `90b3aa0b3bd3b453775b11a386907c7ac9acd997` and the previous pin
`c9e8e611dbc279afa0914d28c4d37ad07f38c03b`.

| Upstream change | Decision | Reason |
| --- | --- | --- |
| `afc13dc8e0` split xkbcommon Wayland/X11 features | Imported | Required for correct single-backend feature builds. |
| `72656afa6d` use the efficient Windows thread-pool API | Pending next Windows sync | The local Windows fork predates this platform change; merge it while retaining Zetta's zoom patch. |
| `fca4016aef` add workspace editor zoom | Not applicable | This is Zed workspace behavior; Zetta has independent terminal-pane maximize and restore actions. |
| `166f044fd0` add Wayland layer-shell exclusive zone/edge | Imported | Compatible GPUI Linux parity; dormant for Zetta's normal toplevel windows. |
| `3565c49dad` fix Unicode columns in path-like targets | Adapted independently | Zetta's standalone path-target flow is different, so the editor/workspace implementation is not imported as-is. |
| `5079b33d65` stop the KWin/Fcitx5 IME feedback loop | Imported | Prevents repeated unchanged cursor-rectangle commits and unbounded composition memory growth. |
| `f1280b64a4` unify `raw-window-handle` | Manifest-equivalent | Zetta pins the same `0.6` dependency directly because its platform forks are outside Zed's workspace. |
| `de827bce2f` add system notification platform APIs | Deferred | Zetta's notification feature is implemented at the application layer; importing unused platform APIs would add dependencies without changing terminal behavior. |
| `7eb8af27a6` remove the Windows `ExitProcess` workaround | Pending next Windows sync | Relevant platform cleanup, but it must be merged with the local Windows fork rather than copied blindly. |

## Changes merged from the synchronized upstream

These upstream changes were merged or adapted because they improve standalone
terminal correctness, rendering, lifecycle safety, or platform reliability:

| Upstream change | Result |
| --- | --- |
| `6297c88f42` close terminal process groups | Adapted into Zetta's PTY teardown and application-quit cleanup. |
| `79d238f6fd` select on Shift-drag during mouse tracking | Imported. |
| `37b9fbf22b` keep alternate screen bottom-aligned | Imported. |
| `0b3621db47` fix inline terminal first-line clipping | Imported. |
| `0c51c7fd24` reduce border-only quad overdraw | Imported for the Windows DirectX renderer. |
| `50e399332c` flash Windows attention once | Imported. |
| `826f28eb8f` align inactive Windows popup behavior | Imported. |
| `914e1c9873` retain macOS pasteboard objects | Imported. |
| `06b6160d46` remove the legacy macOS blur path | Imported. |
| `8e4e5a39ee` match macOS appearance to the selected theme | Imported. |
| `ae99a867d7` repaint X11 after exposure | Imported. |
| `c2a610f7eb` add sextant glyph support | Adapted for the standalone batched renderer: block, quadrant, shade, and sextant glyphs use pixel-snapped subcell quads, with an O(n log n) merge path for dense images. |
| `dc2a339d5d` fix Wayland serial token tracking | Adapted: typed eligible-input serials and press-only tracking preserve Zetta's observation-order rollover handling for selections and popup grabs. |
| `e99616cdd4` add resizable/minimizable window state | Imported through the synchronized GPUI API and integrated into Zetta's macOS titlebar implementation, custom frame, controls, titlebar click, and actions. |
| `2e2fb0a218` unify dependencies | Manifest-equivalent: standalone platform forks already declare the same direct constraints; they intentionally do not inherit Zed workspace dependencies. |

For `2e2fb0a218`, the manual standalone equivalents are
`pathfinder_geometry = "0.5"` and `swash = "0.2.6"` in `gpui_linux`; the
matching `async-task`, `block`, `cbindgen`, Core Graphics/Text, `etagere`,
`foreign-types`, and `pathfinder_geometry` constraints in `gpui_macos`; and
`etagere = "0.2"` in `gpui_windows`. The upstream `gpui`, `gpui_wgpu`, and
`media` manifests remain part of the synchronized Zed workspace, so they use
its new workspace entries directly. No local manifest should gain a
`workspace = true` dependency entry.

The remaining post-pin changes are retained below as the next review queue;
“merge candidate” means the behavior is relevant but requires a three-way
merge with Zetta's fork, not an automatic cherry-pick.

| Upstream change | Decision |
| --- | --- |
| `0c51c7fd24` reduce border-only quad overdraw | Imported for `gpui_windows`; shared GPUI changes remain deferred with the Zed pin. |
| `3f57e8d17d` avoid stealing focus from an open modal | Not applicable to Zetta's standalone terminal startup path. |
| `914e1c9873` retain macOS pasteboard objects | Imported for `gpui_macos`. |
| `6297c88f42` actually close terminal process groups | Adapted and imported into `terminal`; it captures both shell and foreground groups. |
| `50e399332c` flash Windows attention once | Imported for `gpui_windows`. |
| `826f28eb8f` align inactive Windows popup behavior | Imported for `gpui_windows`. |
| `94c6647995` keep zoomed panels open during internal focus moves | Not applicable to Zetta's standalone pane model. |
| `0b3621db47` fix inline terminal first-line clipping | Imported into the standalone renderer. |
| `37b9fbf22b` keep alternate screen bottom-aligned | Imported into `terminal_view`. |
| `79d238f6fd` start selection on Shift-drag during mouse tracking | Imported into `terminal`. |
| `a11083f9a7` defer GPUI appearance callbacks | Deferred with the Zed pin; shared GPUI is not forked here. |
| `06b6160d46` remove the legacy macOS blur path | Imported for `gpui_macos`. |
| `b2131e9df8` support Fetch requests from GPUI web workers | Not applicable; Zetta's desktop target does not use the web platform. |
| `4a1df1f7ca` open relative markdown links at a line | Not applicable to the standalone terminal. |
| `ec3d887507` defer GPUI element-arena clears during draws | Shared GPUI change; defer with the Zed pin. |
| `a8491e63b5` restore macOS file drags | Not applicable; Zetta has no Zed project-panel drag source. |
| `f52fd9ac44` add macOS project-panel file drag-out | Not applicable. |
| `8e4e5a39ee` match macOS appearance to the selected theme | Imported for `gpui_macos`. |
| `c97b7c0ea4` fix GPUI web bugs | Not applicable; the web platform remains upstream and is outside Zetta's desktop scope. |
| `c7aea6cbbd` add Wayland outbound drag support | Not applicable to the current terminal-only drag model. |
| `ae99a867d` repaint X11 after exposure | Imported for `gpui_linux`. |

This queue is intentionally recorded rather than silently applied: the forks
contain substantial standalone rewrites, so each candidate needs a source-level
merge and focused platform validation.
