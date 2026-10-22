# Zetta theme_settings fork

This crate is synchronized with `zed/crates/theme_settings` at Zed revision
`544780422affd11699fb531c49af5cc6e82a3fd7` (2026-10-21).

Retain these Zetta patches when synchronizing:

- `ThemeSettings` is a plain gpui global with its own `Default`, not an entry in
  Zed's `SettingsStore`. Zetta resolves configuration itself in `Config`, and the
  store's only remaining job was to hold this struct and `TerminalSettings`;
  carrying it meant carrying `settings`, and behind it `fs`, `git`, `askpass`,
  `rope`, `text` and `migrator`. The defaults were read out of the running store
  before the move, so the rendered result is unchanged.
- `IntoGpui` is vendored into `content_into_gpui.rs`; it was the only thing this
  crate needed from `settings` itself, and it only touches `settings_content` and
  gpui types.
- The settings-file writers (`set_theme`, `set_icon_theme`, `set_mode`) are
  removed. They wrote back into Zed's settings file, which Zetta does not have.
- Value types come from `settings_content` rather than `settings`'s re-exports.
  Upstream's tests are carried, with `::settings::` spelled
  `::settings_content::` for the same reason.
- `parse_bundled_themes` parses and refines every bundled theme without touching
  GPUI, and `init_with_bundled_themes` takes them already parsed;
  `load_bundled_themes` and `init` are unchanged in effect. Parsing them is about
  5 ms, and Zetta's start-up does it on another thread while the platform starts
  (`src/startup/prefetch.rs`).

Beyond those, synchronizing is a copy from upstream plus the mechanical
adjustments the move out of Zed's workspace requires:

- `Cargo.toml` spells out every dependency that was `workspace = true`, and the
  `[lints]` table, because neither can be inherited from outside
  `zed/Cargo.toml`. Dependencies that are themselves forked point at `../<name>`;
  the rest point into `../../zed/crates/<name>`.
- `publish` is `false`.

If this crate ever stops sitting between Zetta and `gpui` — because Zetta drops
the dependency, or because upstream drops its own `gpui` dependency — delete the
fork rather than keeping it in step.
