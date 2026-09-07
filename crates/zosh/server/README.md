# Mosh server implementations

Zosh can honor a remote shell's standard `CSI 3 J` (erase saved lines) request
when the remote Mosh server preserves it. This directory contains the Rust
`mosh-server-rs` implementation used by the bundled `mosh-server` binary and
the patch for building the upstream C++ server. The shell widget and its
bindings do not need to change. An ordinary screen redraw does not erase saved
lines.

Unmodified Mosh 1.4.0 discards `CSI 3 J`. SSH works because it transports the
terminal byte stream; Mosh sends reconstructed screen state. Rebuilding only
the local Zosh binary does **not** enable this feature on an unmodified server.

## Build the bundled Rust server

The Rust server is a standalone Cargo package as well as a root Zetta binary
target. Build it from this directory when preparing a server for a remote host:

```sh
cargo build --release --locked
```

The resulting `target/release/mosh-server` can be copied to the remote host
and selected from the local machine:

```sh
zosh --server=/absolute/path/to/mosh-server pi
```

It implements Mosh protocol v2 and detects `CSI 3 J` in PTY output. The server
adds the same absolute `OSC 777;zosh-clear-scrollback;N` generation used by the
Zosh display code, including for a clear that does not change screen cells.
Snapshots retain the generation, so retransmitted and out-of-order states do
not clear the local terminal more than once.

Prediction acknowledgements wait until input has been written to the PTY,
then allow a 50 ms grace period for shell output. Unrelated output does not
confirm newer input. Each cumulative screen update carries the current echo
acknowledgement, including when an earlier update was lost or combined away.

`mosh-1.4.0-scrollback.patch` adds the missing server state. It applies to the
official [Mosh 1.4.0 release](https://github.com/mobile-shell/mosh/releases/tag/mosh-1.4.0).
The source archive's SHA-256 is
`872e4b134e5df29c8933dff12350785054d2fd2839b5ae6b5587b14db1465ddd`.
The patch modifies Mosh's GPL-licensed terminal implementation; the upstream
license and copyright notices remain in the source distribution.

## Build the patched upstream server

Copy this patch to the remote host. Install Mosh's build prerequisites first;
on Debian these include `build-essential`, `pkg-config`, `libprotobuf-dev`,
`protobuf-compiler`, `libssl-dev`, `libncurses-dev`, and `zlib1g-dev`.
Use a fresh source directory:

```sh
curl -fLO https://github.com/mobile-shell/mosh/releases/download/mosh-1.4.0/mosh-1.4.0.tar.gz
printf '%s\n' '872e4b134e5df29c8933dff12350785054d2fd2839b5ae6b5587b14db1465ddd  mosh-1.4.0.tar.gz' | sha256sum -c -
tar -xzf mosh-1.4.0.tar.gz
patch -d mosh-1.4.0 -p1 < /absolute/path/to/mosh-1.4.0-scrollback.patch
cd mosh-1.4.0
./configure --disable-client --without-utempter CXXFLAGS='-O2 -std=c++17'
make -j2
```

The resulting `src/frontend/mosh-server` can run directly from the build
directory. Select that absolute **remote** path from the local machine:

```sh
zosh --server=/absolute/path/to/mosh-1.4.0/src/frontend/mosh-server pi
```

This leaves the system Mosh installation available. The existing `--server`
option also works through `zetta mosh`. Deployment is separate from the normal
local Zetta build; Zosh does not automatically install software on the host.

## State and rendering

The server increments a 64-bit generation on each `CSI 3 J`. Copies, resizes,
and terminal resets preserve it; framebuffer equality includes it, so a clear
with no changed cells still produces an update. When the generation differs
from the acknowledged base frame, the display diff includes:

```text
ESC ] 777 ; zosh-clear-scrollback ; DECIMAL_GENERATION BEL
```

This is a Zosh extension inside Mosh's existing authenticated host display
message, not a change to the Mosh transport version. Zosh stores the generation
in each reconstructed screen. Only displaying a changed generation emits a
local `CSI 3 J`, together with a full redraw of the current screen. Repeated
diffs against an older base and late frames therefore do not repeat the clear.
The internal marker is consumed locally and is not printed or used as a title.
Stock clients ignore the marker and retain their existing behavior.

The operation clears the local emulator's saved lines, including any history
that predates a `--no-init` session, just as the shell's request does over SSH.

## Verification

### Opt-in timing diagnostics

Set `MOSH_SERVER_TIMING_LOG` **on the server** to a new file in an existing
directory. The file must not already exist; on Unix it is created with mode
0600. For example, from the client:

```sh
zosh --server='env MOSH_SERVER_TIMING_LOG=/tmp/zosh-timing-1.log /absolute/path/to/mosh-server' mac-host
```

The reference `mosh` wrapper accepts the same `--server` value. Rebuild the
remote Rust server first and open a new session. Use a different filename for
each session. Logging survives server daemonization and does not use stdout,
which remains reserved for the bootstrap protocol.

Reproduce the pause during normal use, note the approximate time and action,
then retrieve the log from the Mac. Records contain elapsed microseconds,
an event name, two numeric fields (`a`, `b`), and a cumulative dropped-record
count. The header gives the process, platform, and approximate Unix start time.
No typed text, terminal output, session keys, or peer addresses are logged.

| Event | a | b |
| --- | --- | --- |
| `input_state` | accepted client state | acknowledged server state |
| `input_apply` | client frame | new user-event count |
| `input_queued`, `input_written`, `input_written_observed` | client frame | 0 |
| `pty_write_begin`, `pty_write_end`, `pty_read`, `pty_output_apply` | byte count | 0 |
| `udp_authenticated`, `udp_sent` | datagram byte count | 0 |
| `host_update` | queued server state | echo acknowledgement |
| `udp_send_error` | OS error code, or 0 | 0 |
| `loop_gap`, `*_slow` | elapsed microseconds (at least 100 ms) | 0 |
| `heartbeat`, `session_start`, `child_exited`, `pty_eof`, `pty_write_error` | 0 | 0 |
| `session_end` | 1 on error, otherwise 0 | 0 |

Compare write begin/end to locate a blocked PTY write, read/apply to locate
reader-queue delays, and host-update/send to locate transport pacing. Frame
records connect accepted input to writer completion. A heartbeat is emitted
each second that the main loop runs; `loop_gap` includes work and sleeping.
Slow-phase records identify PTY draining, input processing, screen diffing,
transport processing/sending, or child polling that takes at least 100 ms.
A successful UDP send only means the local OS accepted it, not that the client
received it.

Logging uses a bounded queue and a separate disk writer; full queues drop
records rather than blocking input. Timestamps are captured on producer
threads, so adjacent lines can arrive slightly out of timestamp order.
Logging stops at approximately 64 MiB and marks the limit in the file.
Disk errors stop logging without terminating the session. Unset the variable
to disable diagnostics. Abrupt termination can lose queued tail records;
normal shutdown allows up to 250 ms to drain them.

### Scrollback verification

With a patched server built on the test machine, run from `crates/zosh`:

```sh
ZOSH_TEST_SERVER=/absolute/path/to/mosh-server cargo test --locked a_remote_shell_clear -- --ignored
```

This starts a temporary loopback session and runs a shell fixture. It checks
the shell's clear-and-redraw output, an ordinary redraw, and a clear with no
changed cells. The same test must fail with `remote CSI 3 J was lost` against
unmodified Mosh. The server and client unit tests additionally cover repeated
and out-of-order states, fragmented markers, malformed markers, and unrelated
terminal output.
