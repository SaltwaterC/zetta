# The zosh keep-alive extension

Zosh speaks stock Mosh protocol v2. This document specifies the one
addition it makes to that wire, what each half of zosh does with it, and
why every existing Mosh implementation keeps working either way.

It is implemented in three places: `crates/mosh_rs` (the client's
`UserStream` event and the timer that mints it), `crates/zosh/src`
(the `-k/--keep-alive` command line), and `crates/zosh/server/src`
(recognising one and answering it promptly).

## The problem

An idle Mosh session is very quiet. The client sends a bare
acknowledgement every three seconds (`ACK_INTERVAL_MS`) and the server
answers on its own three-second heartbeat (`HEARTBEAT_INTERVAL`). That is
long enough for a WiFi radio doing aggressive power management to park
between packets, and the cost lands on the next thing the user does: the
keystroke waits on the radio waking up rather than on the link.

The fix is to hold the session to a packet in each direction on a much
shorter interval — 500 ms by default. Nothing about the terminal changes;
this is purely about not letting the path go quiet.

## Wire format

One new field on the client → server `ClientBuffers.Instruction`, from
Mosh's `userinput.proto`:

```proto
// zosh extension
extend ClientBuffers.Instruction {
  optional uint32 zosh_keepalive_seq = 20;
}
```

**Field 20 is reserved for zosh.** It is unused by `userinput.proto`
(which uses 2 and 3 at the instruction level, and 4, 5 and 6 inside the
nested `Keystroke` and `ResizeMessage`) and by `hostinput.proto` (2, 3
and 7).

It is a bare varint rather than a nested message so the whole instruction
costs five bytes inside a `UserMessage`:

```
0x0A 0x03            UserMessage.instruction (field 1), length 3
  0xA0 0x01 <seq>    Instruction field 20, varint
```

A keep-alive is always its own instruction. It never shares one with a
keystroke or a resize, so a peer that ignores field 20 sees an
instruction with nothing in it and the keystroke runs either side of it
are unaffected.

`seq` counts keep-alives on this session, from 1, wrapping. **It is never
interpreted.** It exists so consecutive keep-alives differ on the wire
and so a packet capture can be read.

## Semantics

**A keep-alive carries no user input.** It contributes zero events to the
`UserStream`. This is not merely a convention: a Mosh server counts the
events it decodes in order to skip the prefix a cumulative diff has
already delivered, so a server that counted a keep-alive as an event
would drop a real keystroke later.

**The acknowledgement is `ack_num`.** The peer's next instruction names
the client state that carried the keep-alive, which is SSP's own
mechanism. There is no server → client keep-alive field, and none is
needed. The client already surfaces the round trip as
`LinkHealth::since_ack_ms`.

**A keep-alive is a floor on sending, not a timer on top of one.** The
client mints one only when it has sent nothing at all for the interval,
so a session carrying real traffic emits nothing extra. On an idle
session the sequence is: keep-alive out, answer back, interval restarts
from that send.

**A keep-alive is suppressed when it could not be acknowledged.** The
client stops minting them while shutting down, and once the peer has
gone quiet for `ACTIVE_RETRY_TIMEOUT_MS` (10 s). Mosh's own retry and
heartbeat logic is what keeps trying on a link that has stopped
answering, and an unacknowledged keep-alive would otherwise accumulate in
the live `UserStream`, since nothing ever subtracts an unacknowledged
prefix away.

## Why it works against a server that has never heard of it

Every Mosh server sets its delayed acknowledgement from the **emptiness
of the diff**, not from what the diff decodes to:

- the reference C++ `mosh-server` calls `set_data_ack()` when
  `!inst.diff().empty()`;
- MoshCatty, which backs zosh's own server, sets `pending_data_ack` on
  the same condition (`transport.rs`).

Either then sends within `ACK_DELAY` — 100 ms. A keep-alive rides in a
non-empty diff, so it draws an answer from any server, with no server
change at all. That is why `-k` is useful immediately rather than only
against a matching server, and why the extension needs no negotiation,
no probe and no capability exchange.

The instruction itself is inert everywhere it is not understood:

- `ClientBuffers.Instruction` is proto2 `extensions 2 to max`, so field
  20 parses into the unknown-field set rather than failing;
- the reference server's `UserStream::apply_string` is an
  `if keystroke / else if resize` chain with no `else`;
- MoshCatty's hand-rolled decoder calls `skip_field` on any tag it does
  not recognise, and `prost` (used by `mosh-rs`) skips unknown fields by
  default.

## What zosh's own server adds

Recognising the field buys one thing: the answer leaves on the same
event-loop pass instead of waiting out the 100 ms delayed-ack window,
which is the window's whole purpose (giving real data a chance to ride
along) and is worth nothing to a keep-alive on an idle session.

`UserStreamTracker::accept` reports `keep_alive` alongside the input
events, and `serve_session` calls `ServerTransport::force_next_send`. A
keep-alive queues no PTY write, does not mark the terminal dirty, and
does not advance the echo acknowledgement.

It is also recorded in the server's timing instrumentation as
`keepalive`, with the state number.

## Interoperability

| Client | Server | Behaviour |
| --- | --- | --- |
| `zosh -k` | zosh `mosh-server` | Recognised; answered on the same loop pass. |
| `zosh -k` | reference `mosh-server` | Field ignored; answered within 100 ms by SSP's delayed-ack path. |
| `zosh` without `-k`, or stock `mosh-client` | zosh `mosh-server` | No keep-alives; the server behaves exactly as before. |
| stock `mosh-client` | reference `mosh-server` | Untouched. |

## Cost

On an idle session with the default interval, roughly two datagrams per
second in each direction instead of one every three seconds. Each is a
Mosh datagram of a few tens of bytes — the keep-alive diff is five bytes
before Mosh's own random chaff, OCB tag and headers.

On a session that is doing anything at all, nothing: the interval is
measured from the last send, and ordinary traffic keeps resetting it.

## Command line

```
-k, --keep-alive        hold the session to a packet every 500 ms
    --keep-alive=MS     use MS milliseconds instead (20-3000)
```

Accepted by `zosh`, by `zetta mosh` (which forwards its arguments
verbatim), and by the bundled endpoint client (`zosh HOST PORT`). The
short form takes no separate value, so `-k host` still names a target.

The interval is bounded by Mosh's own minimum frame interval below and
its unassisted heartbeat above: outside that range there is nothing to
ask for.

There is **no server-side flag**, and there must not be one. The
reference `mosh-server` rejects an unknown option outright, so anything
the launcher added to the remote command line would break every stock
server. The extension is in-band or it is nothing.

`--client=PATH` runs an external endpoint client, which is not
necessarily zosh, so the setting travels to it as `MOSH_KEEPALIVE=<ms>`
in the environment alongside Mosh's own `MOSH_*` settings rather than as
an argument stock `mosh-client` would reject.
