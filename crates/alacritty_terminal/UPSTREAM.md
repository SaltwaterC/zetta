# Zetta Alacritty terminal fork

Upstream base: `zed-industries/alacritty@4c129667ce56611becdc82de6e28218c80e2e88f`.
That revision remains the current upstream `master` as of 2026-08-29.

Retain these Zetta changes when synchronizing:

- `src/snapshot.rs`, a Zetta-authored module with no upstream counterpart:
  serializing a `Term`'s grid back into the escape sequences that would
  reproduce it. It lives here rather than in `crates/terminal` because both
  sides of a session need it — the window snapshots a screen when it hands a
  session over, and the multiplexer, which keeps a grid per pane it reads,
  snapshots one when it hands a session back. The only upstream file it touches
  is `lib.rs`, which declares the module; its tests live in
  `crates/terminal/src/tests/snapshot.rs`, where the harness for building a
  terminal from bytes already is.
- hybrid scrollback storage using a small ring buffer and chunked archive;
- scrollback allocator, large-history, and benchmark fixes;
- Windows ConPTY fragmented-read coalescing and terminal-hangup handling;
- shell integration, resize, and sequence handling needed by Zetta's PTY
  lifecycle;
- attached PTYs (`tty::unix::attach`), where the master file descriptor is
  passed in by the `zmux` multiplexer and the child belongs to that process.
  Upstream's `Pty` assumes it spawned the child, so four things diverge and
  must survive a synchronization: `PtyChild` distinguishes an owned child from
  an attached one and from one *reclaimed* across the multiplexer's own
  `execv` (still this process's child, so still reaped here); `Drop` must not
  hang up or reap an attached child (detaching a session is exactly that drop);
  `next_child_event` reads the exit status from the multiplexer's socket because
  `waitpid` is only available to the real parent; and `EventedPty` gains
  `child_is_foreign`, which is how the event loop tells the two apart.
- the event loop's handling of a hung-up master, which follows from the above.
  Upstream loops back round for "the inevitable `Exited` event", and for a child
  it spawned that event really is inevitable. For a foreign child it is not: the
  only route is a report over the multiplexer's control channel, so a broken
  channel used to mean a pane that accepted no input, showed no exit and could
  not be closed — while spinning a core, because the poller is level-triggered.
  `hungup_too_long` bounds that wait for a foreign child only, and paces the
  poll; an owned child's path is byte-for-byte upstream's.
- `ChildEvent::WatcherDisconnected` must not call `Term::exit`. The two genuine
  exit events may, because the child really has ended; a lost watcher says
  nothing about the child, and `Term::exit` sends `Event::Exit`, which the
  consumer reads as "ended with no usable status". Sending it anyway overruled
  whatever the consumer had decided a disconnect meant.
- Windows: `tty::windows::attach` unblocks the duplicated console pipes through
  `conpty::PIPE_CAPACITY`, which is why that constant is `pub(super)`. It must
  not be re-declared beside `attach`; `piper::pipe` asserts a positive capacity,
  and a second constant that drifted to zero panicked every attached pane.

The eight Zetta commits carrying these changes are `d6aa84b`, `d7b896f`,
`57ecffe`, `d83beb7`, `1f6b1f7`, `9de38c6`, `31c3303`, and `7ba5a85`.
