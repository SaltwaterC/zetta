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

On Unix the loop reads and writes its UDP socket and its PTY master itself,
nonblocking, and waits for either in one `poll`, the way stock mosh-server
waits in one `select` (`session_io.rs`). What still has a thread of its own
wakes the loop through a pipe in that same `poll`:

| Input | Unix | Windows |
| --- | --- | --- |
| UDP datagrams | the loop | `zosh-udp-reader`, blocking on a clone of the socket |
| PTY output, input-write completions | the loop | the PTY reader and writer |
| agent connections and frames | the agent listener and one thread per connection | the same |
| the child's exit | `zosh-child-exit` (`child_exit.rs`) | the same |

The threads publish through `wake::WakingSender`, which wakes the loop after
publishing, so an event sent while the loop is still draining leaves a pending
wake-up behind. On Windows a ConPTY's pipes cannot be waited on together with a
socket, so there every input has a thread and the loop parks. Unix ran that way
too until a profile of a scrolling htop found half the server's time in the
kernel, a third of it futex wake-ups and the context switches around them; it
now matches stock mosh-server to within about a fifth while scrolling, and
undercuts it idle and while resizing.

A run of resizes in one client state reaches the program as the last of them
(`apply_user_events`): a dragged pane edge sends many, and each one applied is a
SIGWINCH and a whole-screen redraw for a size that is already gone. Each is
still checked.

Everything else is a clock, and `next_wake` puts the loop to sleep until the earliest
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

Frames are built at stock Mosh's pace, not whenever the screen changes
(`FramePacer`): no sooner than 8 ms after the screen first changed, so a
program's redraw arriving in several reads is one frame, and no sooner than
the transport's frame interval after the last one. Building a frame is the
expensive part — a diff of the whole screen and a snapshot — and the server
transport would send every one it was given. Snapshots share their screen
behind `Arc`, so acknowledging a state does not copy it again and a frame
that changed nothing on screen holds the previous one; within a screen the
vt100 fork shares rows between copies, so a snapshot copies row pointers and
the diff against the acknowledged screen skips the rows it still shares.

Each frame is a diff from the newest state the client has probably received
(`ServerTransport::frame_base`, Mosh's assumed receiver state), not from the
one it has acknowledged: a state sent within the retransmission timeout is
taken as held, and a frame diffed from it carries only what changed since.
Diffing from the acknowledged state instead resent everything since the last
acknowledgement in every frame, which with history being carried over a slow
link was every unacknowledged scrolled-off row each time. `queue_frame` falls
back to the acknowledged state when there is no snapshot of the assumed one or
the transport will not take it as a base, and once the newest frame's base has
aged past assuming (`frame_base_expired`) the next frame is rebuilt from the
acknowledged state, so a lost base costs one rebuild rather than a stuck screen.

An echo acknowledgement with no screen change to go with it waits up to 30 ms
for one (`ECHO_PIGGYBACK`). Each key's acknowledgement falls due between two
screen updates, and sent on its own it was a third of all frames while typing;
stock mosh-server, measured the same way, sends one frame per key.

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
