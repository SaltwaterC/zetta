# Zetta GPUI platform routing fork

The source matches `zed/crates/gpui_platform` at Zed revision
`c9e8e611dbc279afa0914d28c4d37ad07f38c03b`. Its local manifest routes Linux
and FreeBSD builds to Zetta's `gpui_linux` fork while continuing to use the
upstream GPUI implementations on macOS, Windows, and the web.

Do not add platform behavior here. Synchronize the source from upstream and
retain only the standalone manifest paths required for that routing.
