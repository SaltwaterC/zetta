# Forked dependency upstream audit

Reviewed 2026-10-22 after synchronizing the Zed submodule. It is now pinned to
`544780422affd11699fb531c49af5cc6e82a3fd7` (2026-10-21), which is the current
upstream `main` used for this sync. The previous review was against
`2890c340e07a4c4c7e6778e99a49f5414115b250` (2026-08-28).
The Alacritty base used by Zetta, `4c129667ce56611becdc82de6e28218c80e2e88f`,
is still upstream `master`, so that fork has no upstream to catch up with.

## Fork inventory

All local forks, including routing-only forks, declare `rust-version = "1.99"`
to match the repository's pinned toolchain. Preserve this manifest adjustment
when synchronizing from upstream.

The local `gpui_macos` and `gpui_apple` packages use version `0.1.0-zetta`
to distinguish them from upstream during Cargo's path-lock reconstruction.
Transitive upstream dev-dependencies otherwise introduce the same-name,
same-version packages and leave the local patched `block` edge unlocked,
causing a registry index refresh on every run. Preserve that distinction.

Rust 1.99 also renames atomic `fetch_update` to `try_update`; the GPUI,
terminal and MoshCatty forks use the new spelling without changing their
ordering or update callbacks.

| Local fork | Upstream/base | Current retained change |
| --- | --- | --- |
| `crates/block` | crates.io `block@0.1.6` | Inhabited opaque native class symbol and raw address construction for Rust 1.99; see `block/UPSTREAM.md`. |
| `crates/proc-macro-error2` | crates.io `proc-macro-error2@2.0.1` | Public `extern crate proc_macro` declaration so its existing macro re-export remains valid under Rust 1.99; see `proc-macro-error2/UPSTREAM.md`. |
| `crates/vt100` | crates.io `vt100@0.16.2` | CSI b (REP) support, with preceding-character state retained across Mosh screen clones; rows shared between screens behind `Arc` with copy-on-write and a diff that skips shared rows; and `Parser::from_screen`/`into_screen`; see `vt100/UPSTREAM.md`. |
| `crates/vte` | crates.io `vte@0.15.0` | `std` OSC accumulation bounded at `MAX_OSC_RAW_STD` (8 MiB), with a longer OSC discarded through its terminator and the buffer's retained capacity shrunk after a large one; OSC 133 prompt and command marks; see `vte/UPSTREAM.md`. |
| `crates/alacritty_terminal` | `zed-industries/alacritty@4c129667` | Hybrid bounded-memory scrollback, allocator/performance fixes, Windows ConPTY read and hangup handling, shell integration, resize behavior (including prompt rows kept out of reflow), and attached PTYs whose child belongs to the multiplexer. |
| `crates/terminal` | `zed/crates/terminal@54478042` | Standalone terminal engine with Zetta identity, PTY/process-group/CWD tracking, shell integration, unbounded scrollback coordinates, input mapping, export/serial support, diagnostics, and performance work. |
| `crates/terminal_view` | `zed/crates/terminal_view@54478042` | Standalone renderer and interaction model, independent cursor/text layout, pixel-snapped subcell block/sextant painting, themes and font overrides, pane controls, path targets, literal/asynchronous search, inline sizing, alternate-screen anchoring, and scrollback editing. |
| `crates/gpui_platform` | `zed/crates/gpui_platform@54478042` | Local routing manifest selecting Zetta's Linux, macOS, and Windows platform forks. |
| `crates/gpui_linux` | `zed/crates/gpui_linux@54478042` | Executor cap, keyboard and serial fixes, Wayland diagnostics and resize safety, Wayland renderer created at first paint from an instance created in the background, X11 exposure repainting, Zenity fallback, and Zetta-specific platform behavior. |
| `crates/gpui_macos` | `zed/crates/gpui_macos@54478042` | Input-source lifetime/context gating, keyboard-layout recovery, pasteboard lifetime safety, native menu/profile shortcuts, and related tests. The Metal renderer is no longer forked: it now comes from the submodule's `gpui_apple`. |
| `crates/gpui_windows` | `zed/crates/gpui_windows@54478042` | Correct maximize/restore toggle, DirectX scene annotations, one-shot attention flashing, inactive popup behavior, and input activation fixes. |
| `crates/gpui` | `zed/crates/gpui@54478042` | Unstable sort for the three sprite vectors in `Scene::finish`, `ShapedLine::paint_in_layer`, the per-frame glyph-sprite memo, `Window::with_text_scale`, the primary-selection async read, and deferred test-platform clipboard reads; see `gpui/UPSTREAM.md`. |
| `crates/mosh_rs` | `wilsonglasser/mosh-rs@90b37125` | Zosh's `-k/--keep-alive` client half: a keep-alive `UserStream` event at protobuf tag 20 and the send-floor timer that mints it; and the idle-cost work: screens shared behind `Arc` with unchanged renders skipped, one kept zlib compressor, no per-pass socket-mode syscall, `next_wake_ms`, and a `Vt100Screen::feed` that resumes from its screen; and a 4 MiB cap on the compressed bytes `FragmentAssembly` holds for one instruction. Upstream's standalone front end (`src/bin/`) and live-server integration suite (`tests/`) are not carried. |
| `crates/moshcatty` | `binaricat/MoshCatty@554b9d30` | `Transport::next_deadline`, so `zosh-server` can sleep until the transport's next timer instead of polling `tick`, a kept zlib compressor, a public `recv_state` and `send_interval`, assumed-receiver-state queuing (`set_pending_on`), four behaviour-preserving clippy fixes, and a high-water-mark replay window (`replay.rs`) whose `Transport::receive` reports whether a datagram was in order, with `last_recv` refreshed only for in-order datagrams, as stock mosh does; and stock mosh's 2048-byte `RECEIVE_MTU`, checked before a datagram is decrypted. Upstream's standalone client (`src/bin/`), its live-host tests and Netcatty's packaging are not carried. |
| `crates/theme_settings` | `zed/crates/theme_settings@54478042` | `ThemeSettings` as a plain gpui global rather than a `SettingsStore` entry, with `IntoGpui` vendored, the unused settings-file writers removed, and bundled themes parseable off the GUI thread. |

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

## Changes merged from the 2026-10-21 pin

Of the 779 upstream commits in `2890c340..54478042`, 209 touch forked paths.
Every fork was three-way merged (base: the previous pin, ours: the fork,
theirs: the new pin) rather than copied, and the routing-only forks came out
identical to upstream apart from their recorded mechanical adjustments.
Everything upstream changed was taken unless a row below says otherwise.

Some "Zetta" sides of a conflict turned out not to be Zetta patches at all:
`gpui_linux` and `gpui_windows` had never taken a few changes the previous
pin already carried, so the merge read them as Zetta deletions. Those were
resolved in upstream's favour: `gpui_windows`' cross-window `DrawCoordinator`
and `force_render_pending` (`ec3d887507`), its `WM_ENDSESSION` shutdown
(`7f2a2c3c3e`) and `render_to_image`; `gpui_linux`'s
`set_background_appearance` early return and `PresentationState` tests
(`eb354c8d50`).

### Adapted

| Upstream change | Result |
| --- | --- |
| `f37989fbdf` cross-platform headless windowing; one `LinuxPlatform` over a `DisplayConnection` | Taken whole. Zetta's Linux patches were moved onto it: the async clipboard reads dispatch through `DisplayConnection` (headless answers empty); the main-task watchdog moved to `linux/main_task_watchdog.rs` and wraps `LinuxCommon::register_sources`, which now runs every backend's runnables, so it covers X11 too; the deferred Wayland renderer gained `release`, which the window's `Drop` calls where upstream takes its `Option<WgpuRenderer>`; the path prompt reads the connection's window identifier. |
| `b4ecc45422` group sprites by atlas texture | Zetta keeps its unstable sprite sort and takes the new `(order, texture_id.index)` key. |
| `0ef92145ff` `PlatformAtlas::get_or_insert_with` takes the key by value | Applied to `DeferredAtlas` and the glyph-sprite memo. |
| `e39fb7c2e0` `ScrollWheelEvent` → `ScrollEvent`, `on_scroll_wheel` → `on_scroll` | Applied across `src/`, `terminal` and `terminal_view`. |
| underline recording (`Window::underline`, `painted_underlines`) | Zetta's text scale moved into `Window::underline`, and `paint_line_glyphs` takes upstream's `paint_underline` callback. |
| `ee6badf4b0`, `util::fs_embed!` for Zed's `assets` | Folder relocated to `../../zed/assets`. A debug build of `fs_embed!` reads assets from the checkout at runtime, so the root enables `util`'s `debug-embed`. `rust-embed`, `rust-embed-impl` and `rust-embed-utils` are pinned at 8.11.0, matching Zed's lock: 8.12 no longer matches what `util` re-exports (`EmbeddedCompressedFile`). |
| `a0f8fa7bdb` Windows dialogs off the foreground thread (`dialog::show_dialog`) | `gpui_windows`' `prompt_for_paths_in` runs through it, off the foreground thread. |
| macOS objc2 migration | `gpui_macos`' keyboard, input-context gate, mouse-down Shift normalisation and `prompt_for_paths_in` were rebuilt on the objc2 code; see `gpui_macos/UPSTREAM.md`. |
| `650a8d1bed` release the terminal parse buffer | Both of `terminal`'s `vte` parsers are created on first use, dropped when the pty is released, and the echo parser on reset; a replay keeps the output parser's state. |
| `a51d23c93d`, `51a3a0e971` `PathWithPosition::parse_str` rebuilds the path with `with_file_name` | On a Windows host that joins a POSIX path's file name with `\`, which broke Cygwin path translation and WSL path links that carry a position. `terminal::posix_path_position` parses such targets keeping the written path, and both callers use it. Found by running `terminal`'s Windows suite on Windows. |
| Zed's workspace `rust-embed` now has `compression` | Cargo unifies the feature into every `RustEmbed` derive in the build, Zetta's own included, so an embedded file is inflated on first load. Inflating the fonts on every launch (~11 MiB, ~57 ms) delayed the first terminal's child from ~39 to ~92 ms after launch and cost ~65-90 ms of `compare.py` wall time, in 8 of 8 interleaved rounds. Zetta's fonts are now `include_bytes!` (`UNCOMPRESSED_FONTS` in `src/zetta_assets.rs`, excluded from the derive), and Zed's ~1.5 MiB are read on the startup prefetch thread. `util` turns the feature on, so it cannot be turned off from here; check startup after any sync that changes Zed's `rust-embed` features. |
| `f37989fbdf` connects in `run` rather than at platform creation | `gpui_linux` connects at creation again (`LinuxPlatform::connect_early`), which starts the wgpu instance prewarm ~9 ms sooner. |
| Zed's `khronos-egl` patch (`19c69782eb`) | Carried in the root `[patch.crates-io]` at the same fork commit: wgpu's GL backend makes EGL calls on Linux even when Vulkan renders, so the unwrap it removes is reachable at Zetta's startup. |
| `620efaa5c7` AltGr input is not a meta sequence | Placed in `standalone.rs` `key_down` after scrollback search and vi `/` handling. |
| `63b29c2edd` `"..."` entries in path-regex settings | Zetta has no `SettingsStore` layering; its default-regex test is adapted. |
| upstream `theme_settings` tests | Carried, with `::settings::` spelled `::settings_content::`. `WindowBackgroundContent`'s new Mica variants are mapped in the vendored `content_into_gpui.rs`. |

### Not imported

| Upstream change | Decision |
| --- | --- |
| `a47f721832` iOS backend (`gpui_ios`) and its `gpui_platform` arm | Not routed: `gpui_ios` depends on the submodule's `gpui`, so naming it would put a second `gpui` in the graph, and Zetta has no iOS target. |
| `c7aea6cbbd` Wayland outbound drag | Still declined (as at the previous pin): Zetta has no drag source. Inbound drops, including `87a1ea30e8`, are upstream's. |
| `de827bce2f` platform notifications on Linux (`system_notifications.rs`) | Still omitted; Zetta owns notifications at the application layer. |
| `f2cdf3d5bd`, `05c2c9aba0`, `d3267a39ff`, `f8adffe9d1` agent display output and styling in `terminal`/`terminal_view` | Declined: nothing in Zetta uses them, and the output generation adds a count to every terminal event. |
| `ca49b5dac8` read-only provider terminals (`MouseInputMode`) | Declined: Zetta has no read-only views, and it changes every mouse API signature. |
| Zed's other new `[patch.crates-io]` entries (`cpal`, `async-task` → Zed's fork, a pinned `calloop`, `scratch`) | Not carried: Zetta's build compiles and passes without them, and it never carried Zed's `calloop` or `async-task` patches. |
| `screen-capture` in `gpui_windows` | Does not build without Zed's `windows-capture` patch; Zetta does not enable the feature. |

The terminal forks' own commit-by-commit decisions are in their
`UPSTREAM.md`s. `gpui_windows` moved to `windows` 0.62 with upstream; the root,
`terminal` and `console-server` stay on 0.61/`windows-sys`, which is a second
copy of the crate in a Windows build but no type crosses between them.

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
