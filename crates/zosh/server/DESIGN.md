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

The one addition to that wire is zosh's keep-alive, specified in
`../PROTOCOL.md`.  Nothing else in this server departs from stock Mosh
protocol v2.

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

## Keep-alive

`../PROTOCOL.md` is the specification.  Two things about it matter to the
design above.

A keep-alive decodes to **zero** UserStream events, so it does not disturb the
event counting `UserStreamTracker` depends on.  A server that counted one as an
event would skip a real keystroke in the next cumulative diff.  `decode_events`
is therefore left alone, and `carries_keep_alive` is a separate,
allocation-free walk of the same bytes.

SSP already answers any non-empty diff within its 100 ms delayed-ack window, so
the server needs nothing to be *correct* here.  What recognising the field buys
is `force_next_send`: the answer leaves on the same loop pass, because the
delayed ack exists to let real data ride along and an idle keep-alive has none
to wait for.

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
