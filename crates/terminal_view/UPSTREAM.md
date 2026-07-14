# Zetta terminal view fork

The renderer started from `zed/crates/terminal_view` at the revision recorded by
the `zed` submodule. This fork intentionally exposes only a standalone terminal
view. Zed workspace, project, editor, database, language, panel, and persistence
integration must not be added as dependencies here.
