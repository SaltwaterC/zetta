# Zetta mosh-rs fork

The source baseline is [`wilsonglasser/mosh-rs`](https://github.com/wilsonglasser/mosh-rs)
at `90b37125f5e4a598be91dec37d23921b6865276e`, which is what
`crates/zosh` depended on as a git dependency before this fork.

`mosh-rs` is the client half of Mosh's State Synchronization Protocol:
the OCB3 datagram layer, fragmentation, the SSP sender/receiver timers,
the client's `UserStream`, the terminal, and predictive local echo. Only
`crates/zosh` depends on it.

## Why it is forked

Zosh's `-k/--keep-alive` needs the client to emit a keep-alive
instruction on its own schedule. `MoshSession` exposes only
`send_input` and `send_resize` and keeps its `TransportSender` private,
so neither the new `UserStream` event nor the timer that mints it can
live in `crates/zosh`. Per `AGENTS.md`, an upstream crate is changed by
forking it under `crates/`, never by editing a checkout in place.

## What was dropped from upstream

Neither carries a Zetta change; both are removed because nothing here
uses them.

- `src/bin/mosh-rs/` — upstream's standalone Unix/Windows front end.
  Zosh brings its own in `crates/zosh/src/{client,terminal,display}.rs`.
  Dropping it also drops the `rustix`, `signal-hook` and `windows-sys`
  dependencies, which the library itself never touches.
- `tests/` — upstream's integration suite, which needs a live
  `mosh-server` and a git `alacritty_terminal` dev-dependency. Zosh has
  its own live-server suite in `crates/zosh/src/tests/interop.rs`.

Every inline `mod tests` is kept, per `AGENTS.md`'s rule for a fork that
tracks an upstream counterpart. The manifest is otherwise upstream's,
with `publish = false`, `rust-version` raised to the repository's
toolchain, and `hex` restated as the one dev-dependency the inline tests
use.

## Mechanical adjustment: rustfmt

Upstream is hand-formatted and is not `rustfmt` clean under any style
edition — this was checked against 2015 through 2024 before deciding, so
there is no configuration that would avoid the churn. The fork is
formatted with the repository's `rustfmt` instead, because a crate that
permanently fails `cargo fmt --check` is worse than a one-time reformat.

That accounts for roughly 560 of the lines differing from upstream, and
it is why a synchronization has to run in this order: copy upstream's
`src/`, drop the two directories above, run `cargo fmt`, and only then
reapply the keep-alive patch below.

## Retained Zetta changes

**The keep-alive extension.** `crates/zosh/PROTOCOL.md` is the
specification; this is where the client half of it lives.

- `src/statesync.rs`: `UserEvent::KeepAlive(u32)`,
  `UserStream::push_keep_alive`, the `zosh_keepalive_ms` field at tag 20
  on `UserInstruction`, and its handling in `diff_from`/`apply_string`. A
  keep-alive is its own instruction, the way a resize is, so it never
  splices into a keystroke run, and it carries the interval so the peer
  can hold up its own half.
- `src/sender.rs`: `KEEP_ALIVE_DEFAULT_MS`,
  `TransportSender::set_keep_alive`, the `next_keep_alive` deadline
  computed by `calculate_timers` and folded into `next_due`, and the mint
  in `tick`. The deadline hangs off the last **send**, so a session
  carrying real traffic emits nothing extra, and it is suppressed while
  shutting down or once the peer has gone quiet past
  `ACTIVE_RETRY_TIMEOUT_MS`.
- `src/session.rs`: `MoshSession::set_keep_alive`, a pass-through.

Nothing else changed, and with the keep-alive off every upstream test
still passes unmodified.

See `../UPSTREAM_AUDIT.md` for the fork inventory.
