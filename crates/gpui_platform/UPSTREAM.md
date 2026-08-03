# Zetta GPUI platform routing fork

The source matches `zed/crates/gpui_platform` at Zed revision
`849ec5898a321eefbeb1d1beda130cc50ef43f10`. Its local manifest routes Linux,
macOS, and Windows builds to Zetta's platform forks while continuing to use
the upstream web implementation.

Do not add platform behavior here. Synchronize the source from upstream and
retain only the standalone manifest paths required for that routing. The
manifest remains intentionally outside Zed's workspace so the platform forks
can be compiled by the application. Keep direct dependencies in those fork
manifests aligned with the equivalent Zed workspace constraints; do not replace
them with `workspace = true` entries.
