# Forked crate upstream audit

Reviewed against pinned Zed revision
`c9e8e611dbc279afa0914d28c4d37ad07f38c03b` on 2026-07-18. The previous fork
baseline was `90b3aa0b3bd3b453775b11a386907c7ac9acd997`.

| Upstream change | Decision | Reason |
| --- | --- | --- |
| `afc13dc8e0` split xkbcommon Wayland/X11 features | Already imported | Required for correct single-backend feature builds. |
| `fca4016aef` add workspace editor zoom | Not applicable | Changes Zed workspace panes and its uncompiled terminal panel. Zetta now has independent terminal-pane maximize and restore actions. |
| `166f044fd0` add runtime layer-shell exclusive zone/edge | Already imported | Compatible GPUI Linux parity; dormant for Zetta's normal toplevel windows. |
| `3565c49dad` fix Unicode columns in path-like targets | Not applicable | The change moved navigation into Zed's editor/workspace integration, which the standalone terminal view deliberately does not compile. Zetta currently does not open editor path targets. |
| `5079b33d65` stop the KWin/Fcitx5 IME feedback loop | Imported | Prevents repeated unchanged cursor-rectangle commits and unbounded memory growth during Wayland composition. Includes upstream regression tests. |
| `f1280b64a4` unify `raw-window-handle` dependency | Manifest-equivalent | Zetta pins the same `0.6` version directly because its fork is outside Zed's Cargo workspace. |
| `de827bce2f` add system notification platform APIs | Deferred | Zetta has no notification caller. Importing it now would add `notify-rust` and platform transitive dependencies without changing terminal behavior. Revisit with a designed terminal-bell notification feature. |

No upstream `terminal` source changes occurred in this revision range. No
upstream changes affected the compiled standalone `terminal_view` renderer.
