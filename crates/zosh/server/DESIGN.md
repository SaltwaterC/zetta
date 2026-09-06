# Server design

## Compatibility boundary

The wire boundary is stock Mosh protocol v2:

```text
stock mosh-client
       |
       | UDP + AES-128-OCB3 + SSP
       v
ServerTransport
       |
       +--> ReceivedState { old_num, new_num, throwaway_num, diff }
       |
       v
UserStreamTracker
       |
       +--> newly committed input events only
       v
PTY backend
  +----+----+
  |         |
POSIX     ConPTY
```

No client-side changes are part of the design.

## Why `ReceivedState` is required

A `UserMessage` diff is relative to the SSP state identified by `old_num`.
Treating the diff as fresh input is incorrect because a newer state can be sent
from an older acknowledged base and therefore contain bytes already executed by
the server.

For example:

```text
state 1 = "l"
state 2 = "ls"   (also based on state 0)
```

If state 1 has already committed `l`, state 2's diff from state 0 still contains
`ls`.  The server must commit only `s`.

`UserStreamTracker` keeps state metadata and the uncommitted suffix needed to
reconstruct such branches without retaining the entire lifetime input history.

## Terminal state

PTY output is parsed into an authoritative VT state.  Outbound Mosh SSP states
are generated as terminal transforms from the latest screen state acknowledged
by the client.  Each outbound state number is associated with a screen snapshot
so a later acknowledgement can advance that base safely.

## Platform split

Everything above the PTY abstraction is common Rust code.

- Linux/macOS: `portable-pty` -> POSIX PTY
- Windows: `portable-pty` -> ConPTY

The Windows bootstrap process is separate from the long-lived session process
because SSH expects the remote bootstrap command to terminate after the
`MOSH CONNECT` line is read.
