# Zetta MoshCatty fork

The source baseline is [`binaricat/MoshCatty`](https://github.com/binaricat/MoshCatty)
at `554b9d305e7ac4b11de740d764bbc3e05f816d7b`, which is what
`crates/zosh/server` and the root package's `zosh-server` feature depended
on as a git dependency before this fork.

MoshCatty is a Rust implementation of Mosh's wire protocol: the OCB3
datagram layer, fragmentation, the protobuf instruction codecs and the
State Synchronization Protocol transport. `zosh-server` uses those four
layers for the server half of the protocol; nothing here uses its client,
framebuffer or prediction modules, which are carried only because removing
them would be a change of its own.

## Why it is forked

`zosh-server` has to know when the transport next needs attention so it can
sleep until then. Upstream's `Transport` offers only `tick()`, which compares
private state — the pending-acknowledgement flag, each state's send time, the
retransmission timeout — against the clock, so the only way to honour its
timers from outside was to call it every few milliseconds. On an idle
session that was about 200 wakeups a second, which was most of the server's
CPU on a small host. Per `AGENTS.md`, an upstream crate is changed by forking
it under `crates/`, never by editing a checkout in place.

## What was dropped from upstream

None of these carries a Zetta change; each is removed because nothing here
uses it.

- `src/bin/mosh_client.rs` — upstream's standalone client, with the
  `conpty-test-probe` feature that exists only for it and the release
  profile and `.cargo/config.toml` that exist only to package it.
- `tests/live_mosh_prediction.rs` and `tests/windows-conpty-input.cjs` —
  they need a remote host over SSH and the standalone client respectively.
  `tests/protocol_suite.rs`, which runs over loopback, is kept.
- `assets/`, `docs/`, `scripts/`, `package.json`, `package-lock.json` and
  `README.md` — Netcatty's packaging and documentation.

Every inline `mod tests` is kept, per `AGENTS.md`'s rule for a fork that
tracks an upstream counterpart. Upstream is already `rustfmt` clean. The
manifest is upstream's with the dropped items removed and `rust-version`
raised to the repository's toolchain.

## Mechanical adjustment: clippy

Upstream raises four default clippy lints under the repository's toolchain,
each fixed in place without a change of behaviour so the fork lints clean with
`-D warnings`:

- `src/ansi_apply.rs`: the non-private `h`/`l` arms of the CSI mode match
  take `nums.contains(&4)` as a guard (`collapsible_match`). The arm after
  them is `_ => {}`, so a guard that fails does what the nested `if` did.
- `src/prediction.rs`: `sort_by_key(|a| (a.y, a.x))` for the equivalent
  `sort_by` (`unnecessary_sort_by`); both sorts are stable.
- `src/transport.rs`: `Duration::abs_diff` in `update_rtt` for the
  hand-written absolute difference (`manual_abs_diff`).

A synchronization copies upstream's `src/`, drops the items above, reapplies
these, and then reapplies the patch below.

## Retained Zetta changes

**`Transport::next_deadline`** in `src/transport.rs`: the earliest instant at
which `tick` may have something to send, or at which `shutdown_timed_out` may
change, with `None` once nothing is scheduled. It mirrors the conditions
`tick` tests — a forced send, an unsent state and its pacing, the active and
quiet retransmission rules, the heartbeat and periodic acknowledgement, the
delayed acknowledgement, and the shutdown retries and timeout — and is never
later than the moment `tick` would act. **A change to `tick`'s conditions has
to be made to `next_deadline` as well**; the tests in
`src/tests/transport_deadline.rs` (declared at the foot of `transport.rs`,
outside upstream's test module) pin each one.

**`KeptCompressor`** in `src/transport.rs`: one zlib stream per `Transport`,
reset for each instruction instead of an encoder built for every datagram
`tick` sends. Building one costs more than compressing an idle session's
acknowledgements, and the reset stream's output is byte-identical to a fresh
encoder's (`a_kept_compressor_writes_what_a_fresh_encoder_would`, in the same
test file). `zlib_compress` is kept, `#[cfg(test)]`, because upstream's tests
build fixtures with it.

**`Transport::recv_state` and `ReceivedStateDiff` are public**, and the struct
carries the instruction's `ack_num`. A server needs a received state's numbering
as well as its diff, and before this `zosh-server` decrypted, reassembled and
decompressed every datagram a second time to recover it.

**Assumed receiver state.** `Transport::prospective_base_num` and
`prospective_chain_expired` are public, and `Transport::set_pending_on` queues
a diff on the base it names or refuses it (`None`) — unlike
`set_pending_from`, which quietly substitutes the acknowledged state, right for
a caller that resends everything and wrong for one whose diff only makes sense
from the base it named. `zosh-server` diffs each frame from the newest state
the client has probably received, as stock Mosh does, rather than from the
acknowledged one; carrying scrolled-off history over a 100 ms link, that cut
what each frame resent from 42 KiB/s to 16 KiB/s.

**`Transport::send_interval` is public**, unchanged otherwise. The server
transport sends a new state the moment it has one, and `zosh-server` builds
frames at stock Mosh's pace (`FramePacer` in `server.rs`) using this interval
rather than building every frame and letting the transport drop all but the
newest.

Nothing else changed, and every upstream test still passes unmodified.

See `../UPSTREAM_AUDIT.md` for the fork inventory.
