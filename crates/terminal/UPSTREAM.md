# Zetta terminal fork

This crate is copied from `zed/crates/terminal` at the revision recorded by the
`zed` submodule. Zetta owns this fork so its scrollback ceiling can follow the
Alacritty engine's signed line-coordinate range instead of Zed's 100,000-line
product limit.

The functional patch is confined to `DEFAULT_SCROLL_HISTORY_LINES` and
`MAX_SCROLL_HISTORY_LINES` in `src/terminal.rs`. When updating the Zed
submodule, copy upstream terminal changes here and retain that override.
