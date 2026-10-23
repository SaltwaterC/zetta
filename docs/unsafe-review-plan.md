# `unsafe` review plan

A review of every `unsafe` site in Zetta's own code and of the `unsafe` Zetta
added to its forks, done on 2026-10-21 against `787ba9f`. This plan orders the
fixes that came out of it. Line numbers refer to that commit.

## Scope

In scope:

- `src/` and the Zetta-authored crates: `zmux`, `zosh` (client and server),
  `console-server`, `zetta_profiles`, `zclip`, `zwt`, `wslx`, `zntfy`.
- The `unsafe` Zetta added to or changed in its patched forks:
  `alacritty_terminal`, `terminal`, `gpui_macos`, `gpui_linux`, `gpui_windows`,
  `vte`, `block`. Found by diffing each fork against `zed/crates/*`, the
  crates.io release it came from, or the `fb68c5b` import of
  `alacritty_terminal`.

Out of scope:

- `zed/`, `busy-v/` and `shitty/`: these are submodules.
- Upstream `unsafe` the forks carry unchanged: roughly 800 lines across the gpui
  platform crates. Rewriting it would make every upstream sync harder.
- The routing-only forks (`ui`, `gpui_wgpu`, `gpui_web`, `gpui_apple`, …). They
  add no `unsafe`, and AGENTS.md forbids adding behaviour to them.

The network-facing parsers contain no `unsafe` at all: `moshcatty`, `mosh_rs`
and `vt100`. `console-server` also parses client messages with safe code
(`u32_at`, `as_chunks`); its remaining `unsafe` is Win32 plumbing. So most of
what follows is about handle, descriptor and process ownership, not parsing.

## How to read this plan

**Status** is `confirmed` if the finding was checked by reading the code after
the review agent reported it, `reported` if it was not, and `fixed` once the
fix has landed; a fixed item's section says what was done. Nothing below has
been reproduced at runtime. Windows and macOS code was reviewed statically from
Linux.

**Effort:**

| Size | Meaning |
| --- | --- |
| S | Under a day, one module |
| M | A few days, or a design decision first |
| L | Spans crates, or carries upstream merge cost |

**Validation:** every item lists the suites it touches, using the commands
AGENTS.md gives for that area. A crate under `crates/` is its own workspace and
needs its own `cargo test`. For `zmux`, build the binary first:
`(cd crates/zmux && cargo build --bin zmux && cargo test)`. Windows code needs
`make check-windows` and `make clippy-windows`, plus a run on a Windows host.
macOS cross-checks need the Apple SDK, so macOS code needs a run on a macOS host.

## Overview

| ID | Severity | Status | Area | Summary | Effort |
| --- | --- | --- | --- | --- | --- |
| P1.1 | High | fixed | zmux | Upgrade leaves every pane master and the listener inheritable | M |
| P1.2 | Medium, UB | fixed | zetta_profiles | 1-byte heap overflow in the MSYS2 registry reader | S |
| P1.3 | Medium | confirmed | alacritty_terminal | Reclaimed pane signals a pid it already reaped | S |
| P1.4 | Medium | confirmed | alacritty_terminal | ConPTY `pause()` spins on a closed handle after a reader panic | S |
| P2.1 | Medium | reported | zmux | Session owner identified by pid alone | M |
| P2.2 | Medium | reported | console-server | Client-sized allocations can abort the host process | S–M |
| P2.3 | Medium–Low | reported | zmux (Windows) | `PeerChallenge` attestation can be answered by an impostor | M |
| P3.1 | Low–Medium | reported | zosh | Wake pipes not close-on-exec atomically | S |
| P3.2 | Low | confirmed | zntfy | `/tmp` fallback directory used without validation | S |
| P3.3 | Low | reported | zwt | macOS `clonefile` error path deletes a file zwt did not create | S |
| P3.4 | Low | reported | zmux | `BorrowedFd<'static>` used after the lock that keeps it alive | S |
| P3.5 | Low | reported | alacritty_terminal | `Row::new` relies on a `debug_assert` for columns ≥ 1 | S |
| P3.6 | Low | reported | terminal | `tcgetpgrp` on a possibly reused fd number | S |
| P3.7 | Low | confirmed | src | `tcsetpgrp` without blocking SIGTTOU | S |
| P3.8 | Low, UB | reported | several | Technical UB and safe functions that take raw pointers | S each |
| P3.9 | Low | reported | src, gpui_macos | Leaks and lifetime edges in platform integration | S each |
| P4.1 | – | – | five crates | Make `rustix` a direct dependency | M |
| P4.2 | – | – | Windows code | Std handle types for manual `CloseHandle`/`DuplicateHandle` | M |
| P4.3 | – | – | several | Shared helpers for duplicated `unsafe` | S each |
| P4.4 | – | – | forks | Harden Zetta-rewritten fork files | S each |
| P4.5 | – | – | all | SAFETY comments, then deny undocumented `unsafe` | M |

Phase 1 is four small fixes that close real holes. Do it first and in any order.
Phase 2 needs a design decision per item. Phase 3 is independent clean-up. Phase
4 is the replacement work. It is mechanical, but do it after phases 1–3,
because several of its helpers are the vehicles for those fixes.

## Phase 1: fix first

### P1.1 zmux upgrade leaves descriptors inheritable

`prepare_upgrade` clears `FD_CLOEXEC` on every pane master and on the listener,
so they survive `execv` (`crates/zmux/src/server/upgrade.rs:171-185`). The new
image claims them and never sets it back:

- `server/upgrade.rs:452` claims pane masters with `OwnedFd::from_raw_fd`, then
  `tty::reclaim`, which only sets non-blocking.
- `server.rs:1306` claims the listener with `Listener::from_raw_fd`.

Nothing on the spawn path closes stray descriptors: alacritty's `pre_exec`
closes only the new pane's own pair. So after an upgrade, every shell the daemon
starts holds the master of every pane and the listening socket. A process in an
unprotected session can read from and inject into a secret-protected session,
or `accept()` connections meant for Zetta.

Two windows make this worse:

- If `execv` fails (`UpgradeRefused::AfterAnswering`), the old image keeps
  running with the descriptors inheritable.
- Between `prepare_upgrade` dropping the sessions lock and `execv`, a pane
  spawned outside the lock also inherits them. It also inherits the handover
  memfd (`upgrade.rs:282`, created without `MFD_CLOEXEC`), which holds every
  session's Argon2 verifier and key envelope.

`LifetimeLock::adopt` (`server/lifetime_lock.rs:192-230`) already does it
right: check the descriptor is open, match it, set `FD_CLOEXEC`.

**Fix:**

1. Add `claim_inherited_fd(raw) -> Result<OwnedFd>`: check the descriptor is
   open, set `FD_CLOEXEC`, then claim it. Use it for pane masters, the listener,
   the handover file (`upgrade.rs:265`) and the lifetime lock.
2. Restore `FD_CLOEXEC` on every carried descriptor when `execv` returns.
3. Close the prepare-to-exec window. Create the memfd with `MFD_CLOEXEC`. Hold a
   spawn gate from `prepare_upgrade` until `execv`, so no pane starts while
   carried descriptors are inheritable. Alternatively, clear `FD_CLOEXEC` in one
   step immediately before `execv`.
4. Backstop: at daemon start, mark every descriptor ≥ 3 close-on-exec, using
   `close_range(3, ~0, CLOSE_RANGE_CLOEXEC)` on Linux and a `/dev/fd` sweep
   elsewhere. Do this before claiming anything. Marking does not close, so
   claims still work. This also stops descriptors the daemon inherited from its
   own spawner from reaching shells: the running daemon currently passes a
   `memfd:pulseaudio` to all of its shells.

**Tests** (in `crates/zmux/tests/`):

- After a scripted upgrade, every adopted descriptor has `FD_CLOEXEC`.
- A pane spawned after the upgrade has, under `/proc/<pid>/fd`, none of the
  other panes' masters and not the listener.

Check that both tests fail with the fix removed.

**Validate:** the zmux suite; `make check-features`; `make check-macos` if the
SDK is available, otherwise a macOS host, because the listener is rebound there.

**Done.** All four steps, with both options for step 3:

- `crate::upgrade::claim_inherited_fd` claims the pane masters, the listener,
  the handover and the lifetime lock.
- `InheritedAcrossExec` clears close-on-exec on everything carried in
  `exec_carrying`, immediately before `execv`, and sets it again when dropped,
  which only a failed `execv` reaches. The handover memfd is `MFD_CLOEXEC`.
- `Daemon::spawn_gate` is held for writing across that step and for reading
  around every pane start (`start_terminal`). Other subprocesses the daemon
  runs are not gated, but now only the clear-to-`execv` step is exposed.
- `mark_inherited_close_on_exec` runs first in `server::run`. On FreeBSD
  without fdescfs, `/dev/fd` lists only 0–2, so the sweep misses there.

`an_upgrade_leaves_no_descriptor_inheritable` and
`a_shell_started_after_an_upgrade_inherits_nothing_from_the_daemon` (Linux)
both fail with the claim and the backstop removed: the new shell held the lock,
the listener and the other pane's `/dev/ptmx`. Either half alone passes them.
Not yet checked on macOS.

### P1.2 Heap overflow in the MSYS2 registry reader

In `crates/zetta_profiles/src/discovery.rs:385-416`, the first
`RegQueryValueExW` returns `byte_len`. The buffer is `vec![0u16; byte_len / 2]`,
which is one byte short when `byte_len` is odd. The second call still passes the
original `byte_len`, so the API may write one byte past the allocation.

`REG_SZ` data can have an odd length. The value read is `DisplayName` of every
subkey under HKCU `…\Uninstall`, which any process running as the user can
create. Profile discovery runs in Zetta and in the zmux daemon.

**Fix:** route it through `registry_string` (around line 599). That function
uses an exact-size `u8` buffer and slices by the length returned, and already
does this correctly for Cygwin. Alternatively, use `RegGetValueW` with
`RRF_RT_REG_SZ`. Either way, only one registry-string reader remains.

**Tests:** a Windows-only test in `crates/zetta_profiles/src/tests/` that writes
an odd-length `REG_SZ` under a scratch HKCU key, reads it through the helper,
and deletes the key.

**Validate:** the `zetta_profiles` suite on a Windows host; `make
check-windows`; `make clippy-windows`.

**Done.** `registry_string` moved up from inside
`cygwin_registered_installation_roots` to the top level of `discovery.rs`, and
the MSYS2 reader now uses it in place of its own `read_string`, so one
registry-string reader remains. The MSYS2 lookup now accepts what the Cygwin one
already did: `REG_EXPAND_SZ` as well as `REG_SZ` (left unexpanded), and a value
with no terminating NUL. It decodes UTF-16 strictly, so an unpaired surrogate
rejects the value instead of being replaced with U+FFFD. The helper also checks
the type again after the second query, and slices with `get` rather than
indexing.

`registry_strings_of_odd_length_are_read_within_their_size` writes 13- and
11-byte `REG_SZ` values, a 1-byte value and a `REG_DWORD` under a volatile
scratch HKCU key, and deletes the key on drop. It passes on a Windows host. It
pins how the helper reads an odd length, but cannot detect the one-byte
overflow itself, which needs page heap to fault. The RAII key guard from P4.3 is
not part of this fix.

### P1.3 Reclaimed pane signals a pid it already reaped

In `crates/alacritty_terminal/src/tty/unix.rs`, `PtyChild::Reclaimed { pid }` is
reaped by `try_wait` (line 232) and by `next_child_event` (line 705). Nothing
records that it was. zmux keeps the pane, and its `Pty`, after the child exits.

When the pane is dropped, `Drop` calls `terminate_child` (line 516). That calls
`waitpid(pid, WNOHANG)`, which returns 0 if the pid now belongs to another child
of the daemon, such as another pane's shell. It then sends SIGHUP, waits
250 ms, sends SIGKILL, and reaps that shell. `Owned` children avoid this
because `std::process::Child` caches its status. This only affects panes carried
across `zmux upgrade`.

`reclaim()` (line 251) also accepts any `u32`. A value of 0 becomes `kill(0, …)`,
which signals the daemon's own process group, and `u32::MAX` becomes
`kill(-1, …)`, which signals every process the user can. The value comes from
the previous image, so this is hardening.

**Fix:**

- Change the variant to `Reclaimed { pid, status: Option<ExitStatus> }`.
- Set `status` wherever the child is reaped, and return the cached status from
  `try_wait`.
- Skip `terminate_child` in `Drop` once `status` is set.
- Make `reclaim` reject 0 and anything above `i32::MAX`.

**Tests** (alacritty_terminal):

- `reclaim(0, ..)` and `reclaim(u32::MAX, ..)` are errors.
- After the child exits, `try_wait` returns the same status on repeated calls.
  Today the second call fails with ECHILD.
- Dropping a reaped reclaimed pane sends no signal: spawn a sentinel child and
  check it is still alive.

**Validate:** the `alacritty_terminal` and `terminal` suites; the zmux suite.

### P1.4 ConPTY `pause()` spins after a reader panic

In `crates/alacritty_terminal/src/tty/windows/blocking.rs`, `finished` is stored
only on the reader thread's normal exit (line 204). If the thread panics,
unwinding drops the source, which closes its handle, and `finished` is never
set. `pause()` (line 247) then loops forever, calling `CancelIoEx` every 1 ms on
a closed handle value. Once Windows reuses that value, the loop cancels
unrelated I/O in the process, for example another pane's `ReadFile`.

**Fix:**

- Set `ended` and `finished`, and notify, from a drop guard created at the top
  of the reader closure, so unwinding takes the same path as a normal exit.
- Also leave the loop when the reader's `JoinHandle::is_finished()` is true.

**Tests:** a Windows test that makes the reader closure panic and asserts that
`pause()` returns.

**Validate:** the `alacritty_terminal` suite on a Windows host; `make
check-windows`; `make clippy-windows`.

## Phase 2: needs a design decision

### P2.1 Session owner identified by pid alone

`session.owner` is a pid. It stays set after the owner exits; that is
deliberate, see `is_in_scope_for` at `crates/zmux/src/server.rs:265-277`.
`session_identity_authorized` (`lifecycle.rs:2521`) accepts `peer_pid == owner`
without the secret. `share` (`lifecycle.rs:2066-2122`) then lets the request
replace `session.authentication`.

A same-user process that cycles through pids until it gets the old owner's can
therefore set its own secret on a protected session and attach to it. Yama
already blocks ptrace of the daemon, so this is the route that remains.

**Decision needed:** what counts as the owner's identity. Options:

- (pid, start time): `/proc/<pid>/stat` field 22 on Linux, `proc_pidinfo` on
  macOS.
- A pidfd from `SO_PEERPIDFD` (Linux 6.5+), falling back to (pid, start time).
- On macOS, `LOCAL_PEERTOKEN`, whose audit token carries a pid version.

Whichever is chosen, it is compared on every authorization, not only at attach.

**Tests:** a peer whose pid matches the owner's but whose start time differs is
refused, both for attach and for `Share`.

**Validate:** the zmux suite; `make check-features`; a macOS host.

### P2.2 console-server allocations a client can size

`console-server` runs inside Zetta and inside the zmux daemon. A program in a
pane can call `CreateConsoleScreenBuffer` and then
`SetConsoleScreenBufferSize(h, {32766, 32766})`. That passes
`set_off_screen_size`'s checks (`buffers.rs:1165-1171`), and
`ScreenBuffer::resize` (`buffers.rs:61`) then allocates about 4.3 GB. Windows
does not overcommit, so the allocation fails and `handle_alloc_error` aborts the
process, with every pane in it. The number of screen buffers is not capped.

`api_input` (`doskey.rs:27`) and `read_console` (`api.rs:231`) allocate a size
the client declares, up to about 4 GiB. `Input::inject` (`input.rs:215-232`)
has no record cap, unlike the write and resize paths.

**Decision needed:** what conhost does at these limits. Match it rather than
choosing numbers.

**Fix:**

- Cap cells per buffer and the number of buffers per client.
- Use `try_reserve`, and answer `STATUS_NO_MEMORY` instead of aborting.
- Chunk or cap the input paths.
- Cap `inject` at 65536 records, as `write` is.

**Tests:** unit tests in which oversized requests return an error status;
`make conformance-windows`, to show conhost's expectations still hold.

**Validate:** the `console-server` suite and `make conformance-windows` on a
Windows host.

### P2.3 Windows `PeerChallenge` can be answered by an impostor

An attacker names the victim's pid. The daemon duplicates the read end of a
pipe into that process and sends the handle value back to the attacker
(`lifecycle.rs:2638-2641`). A same-user process can then open the victim with
`PROCESS_DUP_HANDLE`, duplicate the handle out, read the nonce and answer. It
is then attested as the victim, which leads to the same control path as P2.1.
The comment at `transport.rs:935-948` says this is impossible.

On Windows, a same-user process of the same integrity can already open the
daemon itself, so the boundary is largely nominal.

**Decision needed:** whether to harden it, or to document that it is nominal on
Windows. If hardening, use `WSAIoctl(SIO_AF_UNIX_GETPEERPID)` on the accepted
socket, plus a process-creation-time check. This also gives `peer_is_this_user`
a real check, where today it returns `Ok(true)`. Either way, correct the comment.

**Validate:** the zmux suite on a Windows host; `make check-windows`.

## Phase 3: low-severity fixes

Each of these is independent and can be its own commit.

| ID | Where | Fix | Validate |
| --- | --- | --- | --- |
| P3.1 | `crates/zosh/src/wait.rs:145-161`; zosh-server `wake.rs`, `session_io.rs:461` | Replace `pipe()` + `fcntl` with `UnixStream::pair()` + `set_nonblocking`; `client.rs:1185` already does this. It runs in the multi-threaded GUI, so a pane spawned between the two calls inherits the pipe. Removes 10 `unsafe` sites. | zosh suite; `cargo test --test zosh_pane -- --ignored` |
| P3.2 | `crates/zntfy/src/lib.rs:343-348` | Validate the `/tmp/zetta-<uid>` fallback before writing: owned by us, mode 0700, not a symlink, and open with `O_NOFOLLOW`. Port zmux's `private_fs` checks; zntfy cannot depend on zmux. Today another local user can pre-create the directory and plant a symlink at `notification-icon.png`. | zntfy suite |
| P3.3 | `crates/zwt/src/worktree_copy.rs:566-572` | Drop the `remove_file` after a `clonefile` failure. `clonefile` never leaves a partial destination, so on `EEXIST` the removal deletes a file zwt did not create. | zwt suite; a macOS host |
| P3.4 | `crates/zmux/src/server/upgrade.rs:761` | `handover_handles` borrows the master as `'static`, and callers use it after dropping the sessions lock. Instead, `try_clone()` it into an `OwnedFd` while the lock is held. | zmux suite |
| P3.5 | `crates/alacritty_terminal/src/grid/row.rs:77` | Make `debug_assert!(columns >= 1)` an `assert!`, or clamp where `cell_width` can be NaN, which casts to 0 columns. This is upstream code, but a one-line change. | alacritty_terminal suite |
| P3.6 | `crates/terminal/src/pty_info.rs:62` | Hold a duplicated `OwnedFd` (`Mutex<Option<Arc<OwnedFd>>>`) dropped in `close()`, instead of a raw fd number that another pane's `openpty` can reuse. | terminal suite |
| P3.7 | `src/startup/cli_modes.rs:180` | Block SIGTTOU around `tcsetpgrp` with `pthread_sigmask`, the standard job-control idiom. This only matters when the wrapped shell dies without giving the terminal back. | `cargo test cli_modes` |
| P3.8 | see below | Technical UB and safe functions that take raw pointers | per crate |
| P3.9 | see below | Leaks and lifetime edges | per crate |

**P3.8, technical UB and safe signatures that take raw pointers:**

- `zmux/src/transport.rs:417,477`: the control-message buffer has alignment 1,
  but is read as `cmsghdr`. Use a `#[repr(C, align(8))]` buffer; P4.1 makes it
  moot.
- `zmux/src/transport.rs:920`: `TOKEN_USER` read through a `Vec<u8>`. Use the
  aligned helper (P4.3).
- `zetta_profiles/src/installation_trust.rs:243-295`: check `AceSize` before
  reading `Mask` or the SID, and use `read_unaligned`.
- `src/windows_integration.rs:618-630`: declare the startup-info strings as
  `ManuallyDrop<windows_core::BSTR>` and convert them safely, rather than
  scanning for a NUL.
- `console-server/src/launch_control.rs:99`: `assert!(length <= N)` in
  `read::<N>`.
- `console-server/src/native.rs:146`: make `ioctl` an `unsafe fn` or give it
  typed wrappers, and give `Completion` a lifetime so a stale reply buffer
  cannot reach the driver.
- `console-server/src/native.rs:55`: `pub Handle::new` can wrap any raw handle
  and closes it on drop. Replace it with `OwnedHandle`/`HandleOrNull`.
- `zosh` `agent/windows_pipe.rs` (both copies): take `&[u16]` and return
  `OwnedHandle`, rather than a safe fn taking a raw `PCWSTR`.
- `zmux` `upgrade_windows.rs:335,362`: validate the handle with `GetFileType`
  before taking ownership, not after.
- `zmux` `relay/terminal_windows.rs:68`: use `try_clone_to_owned()` on stdout,
  rather than building an `OwnedHandle` that can be NULL.

**P3.9, leaks and lifetime edges:**

- `src/windows_integration.rs:219`: `WriteFile` is passed NULL for both the
  bytes-written count and the overlapped structure, which the docs forbid. Use
  `File::write_all`.
- `src/profile_icon.rs:295`: the `?` leaks the icon handle, and the bits are
  read without `GdiFlush()`.
- `src/default_terminal.rs:657,663`: `CFString`s leak on error paths, and the
  SAFETY comment there is wrong. Use `CFRetained` from `objc2-core-foundation`.
- `src/windows_integration.rs:676`: make `ComApartment` `!Send` with
  `PhantomData<*const ()>`.
- `gpui_macos` `window.rs`: clear the input context's gate before `release` in
  `drop_text_input_context`, so a context AppKit keeps alive cannot forward to a
  freed client. This is Zetta-added code, so there is no merge cost.
- `alacritty_terminal` `tty/windows/conpty.rs`:
  - Load `conpty.dll` with `LoadLibraryExW(LOAD_LIBRARY_SEARCH_APPLICATION_DIR |
    LOAD_LIBRARY_SEARCH_SYSTEM32)` rather than by bare name.
  - Check `ERROR_ALREADY_EXISTS` on the `Local\ZettaPalette-<pid>-<n>` event.

## Phase 4: replace `unsafe` with safe code

### P4.1 `rustix` as a direct dependency

`rustix` 1.1.4 is already in every relevant lockfile as a transitive dependency.
Adding it as a direct dependency at that version adds an edge without moving
any version. Check that `git diff Cargo.lock` shows only the new dependency
line. Adding features may pull in nothing new; confirm that as well.

| Crate | Features | Replaces | Approx. sites |
| --- | --- | --- | ---: |
| zmux | `net termios process fs event` | peer credentials, `sendmsg`/`recvmsg`, `fcntl`, `memfd_create`, termios, `TIOCGWINSZ`, `poll`, `kill`, `geteuid` | 41 |
| zosh, zosh-server | `termios event fs process net` | termios, `poll`, `fcntl`, `read`/`write` on the pty, `waitid`, `geteuid`, `SO_PEERCRED` | 18 |
| root (`src/`) | `termios process` | `raw_terminal.rs`, `serial_console.rs:140`, `output_benchmark.rs`, `cli_modes.rs`, `default_terminal.rs:220` | 12 |
| zclip | `termios event` (+ `pty` in tests) | `remote.rs:267-311` | 6 |
| zwt | `fs` | `statfs`, `FICLONE` | 4 |
| alacritty_terminal, terminal | `termios process` | `set_nonblocking`, `tcgetpgrp`, `killpg` | 5 |

There are four copies of the raw-mode termios code: root `raw_terminal.rs`,
zmux `relay/terminal_unix.rs`, zmux `secret_prompt.rs` and zosh `terminal.rs`.
Converting them together is a natural first step.

Two calls stay on `libc` because rustix does not wrap them: `getpeereid` and
`LOCAL_PEERPID` on macOS, and FreeBSD `xucred`.

**Validate:** each crate's suite; `make check-features`; `make clippy-linux`;
`make check-macos` or a macOS host.

### P4.2 Std handle types on Windows

Replace manual `CloseHandle` and `DuplicateHandle` with `OwnedHandle`,
`BorrowedHandle::try_clone_to_owned`, `std::io::pipe`, and
`PipeReader`/`PipeWriter`. This covers about 40 sites:

| Crate | Sites |
| --- | --- |
| zosh | about 20: agent accept loops, `DetachedChild`, `child_exit` including its `unsafe impl Send`, `sleep_guard`, the bootstrap pipe |
| zmux | about 10 |
| console-server | 4 |
| `src/windows_integration.rs` | `duplicate_handle` |
| alacritty_terminal | `tty/windows/mod.rs`, `conpty.rs` |

Calls that stay: `ConnectNamedPipe`, `TerminateProcess`, the
`CreateProcessW` attribute lists, and the console-mode calls.

**Validate:** `make check-windows`; `make clippy-windows`; each crate's suite on
a Windows host.

### P4.3 Shared helpers

| Helper | Where | Replaces |
| --- | --- | --- |
| `claim_inherited_fd` | zmux | 5 claim sites (delivered with P1.1) |
| One registry-string reader, with an RAII key guard or `windows-registry` | zetta_profiles | the P1.2 site, and about 15 registry calls with `windows-registry`. Check that it supports `KEY_WOW64_*` views first. |
| Aligned `token_user_sid()` | zmux | 3 copies (`transport.rs`, `private_fs.rs`, `listener.rs`) |
| One copy of `agent/windows_pipe.rs` and `agent/unix_socket.rs` | zosh client and server | 8 duplicated sites |
| `Event` (create, set, reset) | console-server | 8 sites down to 3 |

### P4.4 Harden Zetta-rewritten fork files

These files are Zetta rewrites, so the changes carry no upstream merge cost.
Each currently depends on an invariant that only a `debug_assert` or a comment
protects, on paths terminal output drives.

- `alacritty_terminal/src/grid/archive.rs:602`: store `*const [Row<T>]` and
  index it with a bounds check. Keep `unsafe` only for the lifetime extension.
- `alacritty_terminal/src/grid/storage.rs:679`: give `swap` a `live.swap(a, b)`
  fast path and a three-step `mem::replace` otherwise, rather than two raw
  pointers.
- `alacritty_terminal/src/term/cell.rs:283`: add
  `assert!(blank.extra.is_none())` once per row before the bitwise copy. If
  `erased()` ever kept `extra`, that copy would double-free an `Arc`.
- `vte/src/lib.rs:753,969,1018`: use `utf8_chunks`, a checked
  `chars().next()`, and `char::from_u32`. `ground_dispatch` is the parser's hot
  path, so first compare interleaved release binaries on `average_draw_ms` (see
  `docs/performance.md`).
- `gpui_linux` `wayland/clipboard.rs:153,160,393`: use `as_fd()`,
  `std::io::pipe`, and `Deref` instead of `get_mut`.

### P4.5 Document and lock in

1. Add SAFETY comments where they are missing. In `src/`, they are missing from:
   - all of `output_benchmark.rs`
   - `default_terminal.rs:220`
   - the macOS half of `silent_mode.rs`
   - `keybindings.rs:1034`
   - most of `windows_integration.rs`
2. Narrow the `unsafe` blocks that span whole functions:
   `windows_integration.rs:1120,1161,1194` and `keybindings.rs:1034`.
3. Deny `clippy::undocumented_unsafe_blocks` in the root `[lints.clippy]`, and
   record the reason there, as AGENTS.md asks. The crates under `crates/` are
   separate workspaces, so each needs its own `[lints]` entry. Run
   `make clippy-platforms`, because most of these sites sit in `cfg` arms the
   host does not compile.

## Left as is

These stay `unsafe`. Nothing safe replaces them, or replacing them costs more
than it buys.

- Fork and daemonise in zosh-server and the zosh proxy; `pre_exec(setsid)`,
  because `CommandExt::setsid` is unstable; `posix_spawn` with `SETEXEC` and the
  responsibility SPI on macOS.
- Private or undocumented APIs: `NtQueryWnfStateData`, `ConsoleControl`,
  `NtOpenFile`, PEB writes to console-server's own suspended child.
- `TIOCGSERIAL`, `clonefile`, `GetVolumeInformationW`,
  `FSCTL_DUPLICATE_EXTENTS_TO_FILE`, `malloc_trim`.
- COM interfaces (handoff, shell links, jump lists), DirectWrite, GDI, code
  pages, keyboard layout, the CoreFoundation, IOKit and TIS calls, and
  objc2-intents. The bindings Zetta uses mark all of these `unsafe`.
- The 8-byte store in `cell.rs:382` (`write_drawn_ascii`): a deliberate
  performance choice, guarded by static asserts.
- `objc` 0.2 `ClassDecl` ivars in `gpui_macos`. Moving to `define_class!` is L
  and conflicts with every upstream sync.
- The `block` fork. It is already the minimum change Rust 1.99 needs.
- All upstream-identical `unsafe` in the gpui forks.

## Related

`server::tests::an_idle_session_sleeps_instead_of_polling_and_still_answers_at_once`
(`crates/zosh/server/src/tests/server.rs:555`) failed once under load during this
review and passed three runs in isolation. Its 1500 ms window includes starting
`cat` on a pty. Widen the window, or start timing once the pty is up.
