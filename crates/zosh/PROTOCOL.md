# The zosh protocol extensions

Zosh speaks stock Mosh protocol v2. This document specifies the optional
additions it makes to that wire, what each half of zosh does with them, and
why every existing Mosh implementation keeps working either way.

It is implemented in four places: `crates/mosh_rs` (the client's `UserStream`
and host-event extensions), `crates/zosh/src` (the terminal frontend and the
command line), `crates/zosh/server/src` (recognising the extensions and
answering them promptly), and `crates/vt100` (the eviction queue the server's
emulator keeps so that the rows leaving the top of the screen can be carried at
all).

Three extensions are specified here: the keep-alive, terminal color-query
forwarding, and scrollback.

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
  optional uint32 zosh_keepalive_ms = 20;   // the interval being held to
}
```

**Field 20 is reserved for zosh.** It is unused by `userinput.proto`
(which uses 2 and 3 at the instruction level, and 4, 5 and 6 inside the
nested `Keystroke` and `ResizeMessage`) and by `hostinput.proto` (2, 3
and 7). The client and host directions have separate instruction messages,
so the host-direction field 20 used for terminal queries does not collide with
the client-direction keep-alive.

It is a bare varint rather than a nested message so the whole instruction
costs five or six bytes inside a `UserMessage`:

```
0x0A 0x04                 UserMessage.instruction (field 1), length 4
  0xA0 0x01 0xF4 0x03     Instruction field 20, varint 500
```

The value is the interval the client is holding the session to, in
milliseconds. It doubles as the marker, and it is repeated on **every**
keep-alive rather than sent once, so a server that missed the first — or
that started answering mid-session — can learn it from any later one.

A keep-alive is always its own instruction. It never shares one with a
keystroke or a resize, so a peer that ignores field 20 sees an
instruction with nothing in it and the keystroke runs either side of it
are unaffected.

### Terminal color queries

The bundled server's terminal emulator cannot know the palette of the
terminal outside the Mosh client. When a remote program asks for the
foreground or background color with OSC 10/11, zosh forwards that query to
the local terminal instead of consuming it.

The server-to-client extension is a message on field 20 of
`HostBuffers.Instruction`:

```proto
message ZoshTerminalQuery {
  optional uint64 id = 1;
  optional bytes bytes = 2;
}

extend HostBuffers.Instruction {
  optional ZoshTerminalQuery zosh_terminal_query = 20;
}
```

`bytes` is the complete query, including `ESC ] 10 ; ?` or `ESC ] 11 ; ?`
and either BEL or the string terminator `ESC \`. The server assigns IDs
monotonically and keeps each query in its cumulative terminal-state snapshots
until the client acknowledges the state. That makes retransmission safe even
when a query was the only thing that changed.

The client writes the query bytes to its stdout and waits for the matching
OSC 10/11 response from the local terminal. It removes that response from
stdin before keyboard handling and sends it back as field 21 of a standalone
`ClientBuffers.Instruction`:

```proto
extend ClientBuffers.Instruction {
  optional bytes zosh_terminal_response = 21;
}
```

Responses are cumulative UserStream events, so the server writes them to the
remote PTY in order and does not replay them when a state is retransmitted.
Only the zosh client/server pair implements this proxy. A stock Mosh client
skips host field 20, and a stock Mosh server skips client field 21; neither
peer gains color-query forwarding, but ordinary terminal output and keyboard
input remain compatible.

### Scrollback

Mosh synchronizes a screen. Everything that scrolls past the top of it between
two states is not in either of them, so it is gone — which is why `cat` of a
long file over Mosh leaves you with its last screenful and nothing above it,
and why the output of a command cannot be scrolled back to or copied out of.
Stock Mosh's client recovers a little of this by noticing that a screen has
shifted upward and scrolling the real terminal instead of repainting it, but
that only works while the screen moves by less than its own height between
frames. A burst moves it much further, matches nothing, and is repainted in
place.

zosh owns both ends, so its server keeps the rows that leave the top of the
screen and carries them to the client, which writes them into the real
terminal's history. There is no bound on how much survives a burst: what
bounds the extension is how much may be **in flight**, and a client that is
behind slows the program down rather than losing its output.

The rows travel in the display diff, as an OSC string:

```
ESC ] 777 ; zosh-scrollback ; FIRST ; BASE64 BEL
```

`FIRST` is the absolute index of the first row carried, counted from the start
of the session. Each row in the payload is framed as a flags byte
(bit 0: the logical line continued onto the row below), a big-endian `uint32`
length, and that many bytes of the row's contents with its formatting inline.
The contents assume only that the cursor is at the start of a blank row with
default attributes, and restore the attributes they found, so the client can
write one anywhere. Base64 is what keeps the payload inside an OSC string, so
that a Mosh implementation which has never heard of the extension discards it
as an unknown OSC rather than drawing it.

A marker is sent whenever the count has moved, **even with no rows attached**:
the number is how the client learns where the screen it is about to be shown
sits in the session's output. A count that moved further than the rows carried
is the server saying it had to drop some, and the client writes one line
saying how many rather than silently showing less.

The marker travels in the cumulative terminal-state diff for the same reason
the clear-scrollback generation does: retransmission, an out-of-order arrival
and a diff recomputed from an older acknowledged state are all then handled by
the machinery that already handles them for the screen. The rows are attached
to the protocol screen, and what reaches the terminal is the difference
between the screen it is showing and the one it is about to show.

#### Negotiation

One new field on the client → server `ClientBuffers.Instruction`:

```proto
// zosh extension
extend ClientBuffers.Instruction {
  optional uint32 zosh_scrollback_kib = 22;  // KiB the client will hold in flight
}
```

**Field 22 is reserved for zosh.** A client sends it once, in its first
instruction, before anything can have scrolled. It is an event in the
cumulative `UserStream`, so SSP delivers it exactly once without it having to
be repeated or confirmed — unlike the keep-alive, which is repeated because it
announces a setting that can change.

There is no server-side flag and there must not be one, for the reason given
under "Command line" below. The negotiation is in-band or it is nothing.

The server begins collecting rows **before** it has heard from anybody: the
program starts writing the moment the server does, which is before the first
datagram can arrive, so a server that only began collecting on request would
lose its opening screenfuls. What is collected on spec is bounded, is never
allowed to hold the program back, and is thrown away the moment the first
client state turns out not to carry field 22 — which is what every stock Mosh
client silently says by never sending it.

#### Flow control

The budget the client announces is how many bytes of **unacknowledged** rows
the server may hold. While it is reached the server stops reading the program:
its PTY queue fills, the reader blocks, and the program waits — the same
backpressure an SSH session has, and the reason nothing is lost rather than
the newest output winning.

Backpressure stops at `SCROLLBACK_STALL` (10 s, matching Mosh's own
`ACTIVE_RETRY_TIMEOUT`) since the peer was last heard from. Past that the
session is one whose client may never come back, and a Mosh session outliving
its client matters more than history accumulating for nobody, so the oldest
rows give way instead. Their indices are skipped rather than reused, which is
what lets a client that does return say how much it missed.

#### What is not carried

- **A scroll region.** A row pushed out of one is discarded rather than
  remembered, which is what keeps a full-screen program's redraw out of the
  history.
- **The alternate screen.** What leaves the top of it is a program redrawing
  itself, not history.
- **Rows a `CSI 3 J` asked to be forgotten**, including ones still in flight.
  A program that clears its history is asking for exactly that; the count
  still moves, and the clear travels in the same state, which is how the
  client knows the gap is deliberate rather than lost.
- **A wrapped line's identity across a resize.** A row that filled its last
  column is replayed so the terminal wraps it back together, but a row marked
  wrapped whose contents stop short is written as a line of its own rather
  than run onto the next.

#### Cost

The rows are Base64 inside the state that carries them, which is a third
larger than the bytes themselves before Mosh's zlib gets to them. That is the
price of riding in the display diff rather than in a field of its own, and it
buys the retransmission and rebase semantics above. The in-flight ceiling is
2048 KiB because one state has to stay inside Mosh's 4 MiB instruction limit
with the screen diff beside it.

A session that scrolls nothing pays nothing: no rows, no marker, not a byte.

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

**A keep-alive is a floor on sending, not a timer on top of one.** Each
side mints one only when it has sent nothing at all for the interval, so
a session carrying real traffic emits nothing extra. On an idle session
the sequence is: keep-alive out, answer back, interval restarts from that
send.

**Both sides hold up their own half.** This is the part that matters, and
the part a reply-only design gets wrong. A server that only ever answers
what arrives goes quiet exactly when the client's packets are the ones
being delayed — which is the condition a keep-alive exists to survive.
So a server that understands field 20 arms a timer of its own on the
announced interval and transmits on it **regardless of what it is
hearing**. Measured on loopback with the uplink blackholed, that is the
difference between a worst-case downlink gap of 3005 ms and one of
506 ms.

The server's timer lingers for 10 s past the last thing it heard
(`KEEP_ALIVE_LINGER`, matching Mosh's own `ACTIVE_RETRY_TIMEOUT`) and
then stops. That is an order of magnitude more than any power-management
stall observed, and it is what stops a detached session whose client has
genuinely gone from transmitting into the void forever.

The announced interval is clamped to 20-3000 ms before the server
believes it. It arrives over the network, so it is not trusted to be
sane even though it is authenticated.

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

Two things.

**A prompt answer.** It leaves on the same event-loop pass instead of
waiting out the 100 ms delayed-ack window. That window exists to give
real data a chance to ride along, and is worth nothing to a keep-alive on
an idle session.

**Its own half of the keep-alive**, described above: a timer armed by the
announced interval, independent of what the server is hearing. This is
the half a stock `mosh-server` cannot provide, and the reason `-k` alone
did not fix a link whose *server* is the one with broken power
management.

`UserStreamTracker::accept` reports `keep_alive_ms` alongside the input
events; `serve_session` clamps and latches it, calls
`ServerTransport::force_next_send`, and drives `keep_alive_due` from its
own last send. A keep-alive queues no PTY write, does not mark the
terminal dirty, and does not advance the echo acknowledgement.

Both halves are recorded in the server's timing instrumentation, as
`keepalive` (one arrived, with the state number and the announced
interval) and `keepalive_send` (this side's timer fired, with the gap
since its last send). See "Proving it" below.

## Interoperability

| Client | Server | Behaviour |
| --- | --- | --- |
| `zosh -k` | zosh `zosh-server` | Recognised; answered on the same loop pass, **and** the server holds up its own half on the same interval. |
| `zosh -k` | reference `mosh-server` | Field ignored; answered within 100 ms by SSP's delayed-ack path, but only ever in reply. |
| `zosh` without `-k`, or stock `mosh-client` | zosh `zosh-server` | No keep-alives; the server behaves exactly as before. |
| zosh client | zosh `zosh-server` | OSC 10/11 color queries are forwarded to the local terminal and their responses are written to the remote PTY. |
| zosh client | stock `mosh-server` | The host query extension is ignored; ordinary terminal traffic remains compatible. |
| `zosh` (default) | zosh `zosh-server` | Scrolled-off rows are carried and written into the local terminal's history. Measured on loopback, a 100 000-line burst arrives complete; without it, 23 lines of it do. |
| `zosh` (default) | stock `mosh-server` | Field 22 is skipped, no rows are ever carried, and the client falls back to inferring scrolls from the screen — exactly what stock Mosh does. |
| `zosh --no-scrollback`, or stock `mosh-client` | zosh `zosh-server` | The server settles the question on the first state it accepts, forgets what it had collected, and behaves exactly as a stock server for the rest of the session. |
| stock `mosh-client` | reference `mosh-server` | Untouched. |

## Cost

On an idle session with the default interval, roughly two datagrams per
second in each direction instead of one every three seconds. Each is a
Mosh datagram of a few tens of bytes — the keep-alive diff is five or six
bytes before Mosh's own random chaff, OCB tag and headers.

Measured on loopback over ten seconds:

| | client -> server | server -> client |
| --- | --- | --- |
| without `-k` | 0.44/s, 3000 ms gaps | 0.44/s, 3000 ms gaps |
| with `-k` | 2.10/s, median gap 500 ms | 2.13/s, median gap 499 ms |

The server's own timer adds nothing to that: it is a floor on sending, so
an ordinary reply resets it and it only ever fires when the reply did not
happen.

On a session that is doing anything at all, nothing: the interval is
measured from the last send, and ordinary traffic keeps resetting it.

## Proving it

The server's timing instrumentation is the way to confirm a real link,
and it needs no privileges. Start the session with the log enabled:

```sh
zosh -k --server="env MOSH_SERVER_TIMING_LOG=\$HOME/zosh-timing.log zosh-server" user@host
```

Then, on the server, the gaps between arriving keep-alives in
milliseconds, worst last:

```sh
awk '$2=="keepalive"{if(p)printf "%.0f\n",($1-p)/1000; p=$1}' ~/zosh-timing.log | sort -n | tail
```

A healthy link reads `496 500 502 ...`. No `keepalive` lines at all means
the client is not sending them — check that `zosh --help` lists
`--keep-alive`, and note that `--client=PATH` hands the setting to an
external client as `MOSH_KEEPALIVE`, which only zosh honours. Gaps far
above the interval mean the keep-alives are being delayed on the way in,
which is the stall itself; `keepalive_send` lines are this server's own
timer covering exactly that.

The log file must not already exist, and this needs the bundled Rust
server. With `sudo`, `tcpdump -ni any -ttt udp port PORT` shows the same
thing from either end, `-ttt` printing the gap between packets.

## Command line

```
-k, --keep-alive        hold the session to a packet every 500 ms
    --keep-alive=MS     use MS milliseconds instead (20-3000)
-s, --scrollback        keep the output that scrolls off the screen [default]
    --scrollback=KIB    hold KIB kibibytes of it in flight (16-2048)
    --no-scrollback     keep only what is on the screen, as stock Mosh does
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
necessarily zosh, so the settings travel to it as `MOSH_KEEPALIVE=<ms>` and
`MOSH_SCROLLBACK=<kib>` in the environment alongside Mosh's own `MOSH_*`
settings rather than as arguments stock `mosh-client` would reject.
