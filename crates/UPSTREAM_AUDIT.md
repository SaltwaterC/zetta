# Forked dependency upstream audit

Reviewed 2026-08-29 after synchronizing the Zed submodule. It is now pinned to
`2890c340e07a4c4c7e6778e99a49f5414115b250` (2026-08-28), which is the current
upstream `main` used for this sync. The previous review was against
`849ec5898a321eefbeb1d1beda130cc50ef43f10` (2026-08-03).
The Alacritty base used by Zetta, `4c129667ce56611becdc82de6e28218c80e2e88f`,
is still upstream `master`, so that fork has no upstream to catch up with.

## Fork inventory

| Local fork | Upstream/base | Current retained change |
| --- | --- | --- |
| `crates/alacritty_terminal` | `zed-industries/alacritty@4c129667` | Hybrid bounded-memory scrollback, allocator/performance fixes, Windows ConPTY read and hangup handling, shell integration, resize behavior, and attached PTYs whose child belongs to the multiplexer. |
| `crates/terminal` | `zed/crates/terminal@2890c340` | Standalone terminal engine with Zetta identity, PTY/process-group/CWD tracking, shell integration, unbounded scrollback coordinates, input mapping, export/serial support, diagnostics, and performance work. |
| `crates/terminal_view` | `zed/crates/terminal_view@2890c340` | Standalone renderer and interaction model, independent cursor/text layout, pixel-snapped subcell block/sextant painting, themes and font overrides, pane controls, path targets, literal/asynchronous search, inline sizing, alternate-screen anchoring, and scrollback editing. |
| `crates/gpui_platform` | `zed/crates/gpui_platform@2890c340` | Local routing manifest selecting Zetta's Linux, macOS, and Windows platform forks. |
| `crates/gpui_linux` | `zed/crates/gpui_linux@2890c340` | Executor cap, keyboard and serial fixes, Wayland diagnostics and resize safety, X11 exposure repainting, Zenity fallback, and Zetta-specific platform behavior. |
| `crates/gpui_macos` | `zed/crates/gpui_macos@2890c340` | Input-source lifetime/context gating, keyboard-layout recovery, pasteboard lifetime safety, native menu/profile shortcuts, and related tests. The Metal renderer is no longer forked: it now comes from the submodule's `gpui_apple`. |
| `crates/gpui_windows` | `zed/crates/gpui_windows@2890c340` | Correct maximize/restore toggle, DirectX scene annotations, one-shot attention flashing, inactive popup behavior, and input activation fixes. |
| `crates/gpui` | `zed/crates/gpui@2890c340` | Unstable sort for the three sprite vectors in `Scene::finish`. |
| `crates/theme_settings` | `zed/crates/theme_settings@2890c340` | `ThemeSettings` as a plain gpui global rather than a `SettingsStore` entry, with `IntoGpui` vendored and the unused settings-file writers removed. |

### Routing-only forks

These carry **no** Zetta patches. They exist because `gpui` is not a leaf: each
of them sits between Zetta and `gpui`, and Cargo resolves a dependency edge from
the manifest that declares it, so leaving any of them in the submodule pulls a
second, incompatible copy of `gpui` into the build. A path override in
`.cargo/config.toml` was tried first and rejected — it rearranged the crate
graph and Cargo documents it as slated to become a hard error.

Synchronize them by straight copy from upstream, then reapply the mechanical
adjustments recorded in each one's `UPSTREAM.md` (resolved `workspace = true`
dependencies and `[lints]`, `publish = false`, and relocated `include_*!` and
`rust_embed` paths). If Zetta ever stops depending on one, or upstream drops its
own `gpui` dependency, delete the fork rather than keeping it in step.

| Routing-only fork | Lines | Reaches `gpui` via |
| --- | ---: | --- |
| `crates/ui` | 28,586 | direct |
| `crates/settings_content` | 11,803 | direct |
| `crates/theme` | 5,872 | direct |
| `crates/gpui_wgpu` | 4,840 | direct |
| `crates/gpui_web` | 4,474 | direct |
| `crates/gpui_macros` | 3,317 | direct |
| `crates/task` | 3,164 | direct |
| `crates/gpui_apple` | 2,013 | direct |
| `crates/zed_actions` | 998 | direct |
| `crates/component` | 530 | direct |
| `crates/syntax_theme` | 345 | direct |
| `crates/release_channel` | 308 | direct |
| `crates/assets` | 65 | direct |
| `crates/menu` | 37 | direct |

Seven more routing forks were deleted when Zetta stopped depending on Zed's
settings layer: `settings`, `migrator`, `fs`, `git`, `askpass`, `rope` and
`text` — 45,476 lines, including Zed's git layer and its SSH askpass helper.
`settings_content` stays: it is a schema crate of plain value types with no
filesystem or git dependency, and Zetta's `TerminalSettings` still reads its
enums. `theme_settings` is no longer routing-only — it owns `ThemeSettings` as a
plain gpui global and has its own `UPSTREAM.md` entry above.

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

## Changes merged from the 2026-08-28 pin

Of the 361 upstream commits in `849ec589..2890c340`, 40 touch forked paths.
These were merged or adapted:

| Upstream change | Result |
| --- | --- |
| `52b2418110` extract the shared Apple renderer | Adopted by deleting `gpui_macos`'s `metal_renderer.rs`, `metal_atlas.rs`, `shaders.metal` and `build.rs` — all byte-identical to the previous pin — and depending on `zed/crates/gpui_apple`. This also brings in `be8c6f9fb3` (renderer resource management) for macOS. |
| `be8c6f9fb3` tweak renderer resource management | Imported verbatim into `gpui_windows` (`directx_renderer.rs`, `shaders.hlsl`), which had no local divergence. |
| `7040aa5669` clear the render target before compositing COLR emoji | Imported verbatim into `gpui_windows/direct_write.rs`. |
| `1d7e5f1d01` fix window placement on a secondary monitor with different DPI | Imported; `display.rs` verbatim, `window.rs` merged. |
| `5dd0666dfb` fix the missing NUL terminator in X11 `WM_CLASS` | Imported. |
| `d9ad6aff67` release X11 client state before the close callback | Imported; the previous form called `should_close()` while holding `borrow_mut()`. |
| `c43e2d9734` handle XKB context initialization failure | Imported, with the upstream regression test. |
| `655ed1385b` stop inactive Wayland windows updating the IME position | Imported. |
| `4d1935b8d0` clear the X11 urgency hint when the window becomes active | Imported. |
| `f4178619ac` drain buffered X11 events after foreground work | Imported, except the removal of `set_input_focus` from `activate`, which is a separate focus-behaviour change Zetta has not adopted. |
| `2bf9e26473` fix alt-f5 and add ctrl-alt-key | Imported; the modified-function-key table spelled f5 as `F5`, and the alt-prefix path also encoded named keys as their own names. |
| `f25b256f2c` respect word boundaries after tree branches | Imported into both term configs. |
| `c8dfe26a7e` add visual-line select to terminal vi mode | Imported. |
| `184e124bba` resolve path hyperlinks using per-line cwd | Adapted: `cwd_history`/`cwd_at_line` feed both `process_hyperlink` and Zetta's own `path_like_target_at_event_position`. WSL and Cygwin keep resolving against the current directory, because their translation applies to the currently reported path. The commit's `emit_title_changed_if_changed` bugfix does not apply — Zetta already captured the previous value before `load()`. |
| `b41505358f` make hyperlinks display correctly with changing content | Adapted into the standalone renderer: `make_content` carries a hovered word only across an unchanged grid and shifts its match when the viewport follows new output, and `terminal_element` matches on the hovered word's id rather than the whole value. |
| `3b90a96830` release completed terminal PTY resources | Adapted. Upstream's typed `TerminalMode`/`PtyResources` refactor is agent-driven and was not imported; the underlying leak was. See below. |
| `7f2a2c3c3e`, `7150765979` | Already absorbed by the submodule bump: these are `Platform` signature changes. |
| `eb354c8d50` make the Wayland render loop demand-driven | Already present — Zetta's `FrameLoop` state machine and upstream's are the same design. Nothing to import. |

### Not imported

| Upstream change | Decision |
| --- | --- |
| `492acd6c81` revert "actually close process groups" | **Not imported.** Upstream reverted its own fix because it terminated newly started tasks; Zetta keeps the behaviour, because not orphaning a job that ignores SIGHUP/SIGTERM is the point. The reuse hazard behind that regression is addressed directly instead — see below. |
| `279da63882`, `93f07f6d17`, `107ee1a60a`, `08827f9208`, `e3056061d4`, and the view half of `1c9cbd3b24` | Not applicable: all in `terminal_view.rs`, `terminal_panel.rs` or `persistence.rs`, which this fork does not compile (`[lib] path = "src/standalone.rs"`). |
| `a7d74150ac` fix `git_gutter_width`; `1c9cbd3b24`'s `used_lines` | Not applicable: no Zetta counterpart. |
| `5b70f793d3` use `Duration`; `1271f8b0e8` bump rustc to 1.97 | Cosmetic. `hyperlinks.rs` matches the previous pin, so both remain trivially importable when wanted. |
| `2893b86b04` + `242fe31a39` macOS simple fullscreen over the notch | New upstream feature; Zetta has no `simple_fullscreen_state`. Optional. |
| `4c6c4750d3` improve Windows shell discovery | Not applicable: Zetta never calls `Platform::restart`. |
| `74490daced`, `fecc3273ed`, `4601ead416` | Headless/bench/web plumbing. Only `gpui_platform`'s `test-support = [.., "gpui_windows/test-support"]` was taken. |

### PTY lifetime, corrected while merging the above

The comparison surfaced two Zetta-side problems that the upstream commits above
only partly describe:

- An exited-but-open pane retained its entire event loop. `PtyIo` holds the
  loop thread's `JoinHandle`, the thread *returns* its `EventLoop` rather than
  dropping it, and an un-joined handle keeps the returned value alive — so the
  pty master descriptor, the poller's descriptor and the loop's buffers were all
  held until the pane was closed. `Terminal::release_pty_resources` now drops
  them when the child ends.
- Releasing them makes the borrowed pty master descriptor stale, which is the
  hazard behind upstream's revert: `tcgetpgrp` on a recycled descriptor answers
  with another pane's foreground process group, and that answer was about to be
  sent SIGTERM and SIGKILL. `ProcessIdGetter` now marks the descriptor closed
  when the loop is released, teardown skips signalling entirely once the child
  is known to have ended, and `Terminal::drop` captures the process groups
  before shutting the pty down rather than after, which is what its own comment
  always said it did.

## Deliberate behavioural divergences from upstream

Changes where Zetta intentionally differs from the upstream source, so a future
synchronization does not quietly revert them.

| Divergence | Upstream behaviour | Why Zetta differs |
| --- | --- | --- |
| OSC 8 hyperlinks require the hyperlink modifier (`crates/terminal/src/terminal.rs`, `mouse_up`) | A plain left click on a cell carrying an OSC 8 hyperlink calls `open_url` immediately; only heuristically detected URLs require the modifier. | The URI in an OSC 8 sequence is chosen by whatever writes to the terminal and need not match the visible text, so ordinary output could turn a single click into a system-handled open of a `file://`, `smb://`, or registered custom-scheme target. Requiring the same modifier as a detected URL makes both link kinds behave alike and keeps opening an explicit act. Recorded in the 2026-08-12 security review. |
