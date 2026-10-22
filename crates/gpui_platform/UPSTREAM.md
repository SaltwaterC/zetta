# Zetta GPUI platform routing fork

The source matches `zed/crates/gpui_platform` at Zed revision
`544780422affd11699fb531c49af5cc6e82a3fd7` (2026-10-21), except for the
routing differences below. Its local manifest routes Linux,
macOS, and Windows builds to Zetta's platform forks while continuing to use
the upstream web implementation.

Keep platform-specific behavior in the platform forks. This crate may expose
small application-facing routing helpers, such as the activation-token helper,
but it should not implement compositor behavior itself. Synchronize the source
from upstream and retain only the standalone manifest paths required for that
routing. The manifest remains intentionally outside Zed's workspace so the
platform forks can be compiled by the application. Keep direct dependencies in
those fork manifests aligned with the equivalent Zed workspace constraints; do
not replace them with `workspace = true` entries.

The source differs from upstream only where it routes:

- `prompt_for_paths_in`, `activate_window_with_token` and the `PATH_PROMPT`
  hook stay, because Zetta's platform forks expose them;
- the iOS backend (`gpui_ios`, `a47f721832`) is not a dependency, and
  `current_platform` has no iOS arm. `gpui_ios` depends on the submodule's
  `gpui`, so naming it would put a second `gpui` in the graph, and Zetta has no
  iOS target. Fork it as a routing crate if that ever changes;
- `futures` is a direct dependency, for the path-prompt routing.
