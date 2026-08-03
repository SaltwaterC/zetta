# Zetta Alacritty terminal fork

Upstream base: `zed-industries/alacritty@4c129667ce56611becdc82de6e28218c80e2e88f`.
That revision remains the current upstream `master` as of 2026-08-03.

Retain these Zetta changes when synchronizing:

- hybrid scrollback storage using a small ring buffer and chunked archive;
- scrollback allocator, large-history, and benchmark fixes;
- Windows ConPTY fragmented-read coalescing and terminal-hangup handling;
- shell integration, resize, and sequence handling needed by Zetta's PTY
  lifecycle.

The eight Zetta commits carrying these changes are `d6aa84b`, `d7b896f`,
`57ecffe`, `d83beb7`, `1f6b1f7`, `9de38c6`, `31c3303`, and `7ba5a85`.
