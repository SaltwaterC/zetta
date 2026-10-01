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
is therefore left alone, and `keep_alive_interval` is a separate,
allocation-free walk of the same bytes.

SSP already answers any non-empty diff within its 100 ms delayed-ack window, so
the server needs nothing to be *correct* here.  Recognising the field buys two
things.  `force_next_send` puts the answer on the same loop pass, because the
delayed ack exists to let real data ride along and an idle keep-alive has none
to wait for.

More importantly, the announced interval arms a timer of the server's own
(`keep_alive_due`), driven from `last_send` and deliberately independent of
anything arriving.  A server that only ever replies goes quiet exactly when the
client's packets are the ones being delayed, which is the condition the
keep-alive exists to survive; that is why the timer does not consult
`last_recv` except to stop lingering after ten seconds.  The interval is
clamped before it is believed, because it arrives over the network.

## Session loop

The loop waits on nothing itself. Each input has a thread that blocks on it and
wakes the loop through `wake::WakingSender`, which unparks after publishing so
an event sent while the loop is still draining leaves a park token behind:

| Input | Thread |
| --- | --- |
| UDP datagrams | `zosh-udp-reader`, blocking on a clone of the socket |
| PTY output, input-write completions | the PTY reader and writer |
| agent connections and frames | the agent listener and one thread per connection |
| the child's exit | `zosh-child-exit` (`child_exit.rs`) |

Everything else is a clock, and `next_wake` parks the loop until the earliest
one: the transport's own timers (`Transport::next_deadline`, the reason
`crates/moshcatty` is forked), the echo-acknowledgement grace period, the
keep-alive, the association and network timeouts, the scrollback stall and the
sleep-guard linger. An idle session therefore wakes for a heartbeat or a
keep-alive rather than every few milliseconds; the loop it replaced polled every
5 ms, about 200 wakeups a second, which was most of its CPU on a small host.

Two rules keep that safe, and both are pinned by tests in `tests/server.rs`:

- **Never park with work owed.** A pass that stopped a drain early, freed a
  scrollback budget that was holding PTY output back, or became owed a host
  update after building one goes round again instead.
- **Never leave a passed deadline in the set.** A deadline in the past means
  "wake at once", so each one is either consumed by the pass it wakes or left
  out once it has fired. A keep-alive that cannot be sent stays due, so its
  retry is floored at `KEEP_ALIVE_MIN` rather than allowed to spin.

The child is watched without being reaped (`waitid(WNOWAIT)` on Unix, a
duplicated process handle on Windows), so the loop still owns it and kills it
the way `portable_pty` does. PTY end of file cannot stand in for this: a
background job can hold the terminal open after the shell exits, and a ConPTY's
output stays open until the pseudoconsole is closed.

UDP sends share the reader's blocking socket, so they are bounded by a 2 ms send
timeout instead; one that cannot go out fails the way a nonblocking send did,
and SSP retransmits it.

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
