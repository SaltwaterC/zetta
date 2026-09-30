# portable-pty

Forked from the crates.io portable-pty 0.9.0 source (MIT), upstream
https://github.com/wezterm/wezterm (`pty/`). Source and license are preserved;
upstream's examples and their development dependencies are not carried, and
the manifest is the normalized one crates.io publishes, with `publish = false`.

Zetta changes one thing, in `src/win/psuedocon.rs`: a child attached to the
pseudoconsole is started with NULL standard handles instead of
`INVALID_HANDLE_VALUE`. The console replaces NULL handles with the
pseudoconsole's own, which is what Windows Terminal relies on; it keeps
`INVALID_HANDLE_VALUE`, which is also the current-process pseudo-handle, as the
child's handle. Under a detached `zosh-server.exe`, whose own standard handles
are `NUL`, that left `cmd.exe` without a prompt and Windows OpenSSH stalling as
soon as its session opened. Upstream's reason for setting the handles at all —
not inheriting a daemon's redirected handles — still holds with NULL.

Root and the standalone zosh server patch crates.io to this fork.
