# Zetta Alacritty terminal fork

This crate is Zetta's local fork of
[`alacritty_terminal`](https://github.com/zed-industries/alacritty/tree/4c129667ce56611becdc82de6e28218c80e2e88f/alacritty_terminal)
at revision `4c129667ce56611becdc82de6e28218c80e2e88f`.

The fork retains Alacritty's Apache-2.0 license. Zetta carries it locally so
terminal-engine changes required for effectively unbounded scrollback,
efficient history archival, and Windows shell behavior can be reviewed and
tested with the application. See `UPSTREAM.md` for the retained patch list.
