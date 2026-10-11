# Zetta gpui_apple fork

This crate is synchronized with `zed/crates/gpui_apple` at Zed revision
`2890c340e07a4c4c7e6778e99a49f5414115b250`.

**No Zetta patches. Do not add any.** This fork exists only so that the `gpui`
fork can be wired in: this crate sits between Zetta and `gpui`, and Cargo
resolves a dependency edge from the manifest that declares it, so leaving this
crate in the submodule would pull a second copy of `gpui` into the build. See
`crates/gpui/UPSTREAM.md` for why that is not survivable, and
`crates/UPSTREAM_AUDIT.md` for the full routing set.

Synchronizing is therefore a straight copy from upstream, followed by the
mechanical adjustments the move requires:

- `Cargo.toml` spells out every dependency that was `workspace = true`, and the
  `[lints]` table, because neither can be inherited from outside
  `zed/Cargo.toml`. Dependencies that are themselves forked point at `../<name>`;
  the rest point into `../../zed/crates/<name>`.
- `publish` is `false`.

If this crate ever stops sitting between Zetta and `gpui` — because Zetta drops
the dependency, or because upstream drops its own `gpui` dependency — delete the
fork rather than keeping it in step.

## Cargo package identity

Keep the local package version distinct from upstream (`0.1.0-zetta` rather
than `0.1.0`). Cargo reads upstream GPUI manifests through transitive
dev-dependencies while reconstructing path packages from the lockfile. With
identical names and versions, it can attach the lock entry to the upstream
copy, leaving the local fork's patched `block` dependency unlocked and
refreshing the crates.io index on every invocation. Preserve this version
distinction when synchronizing, and regenerate the affected lockfiles.
