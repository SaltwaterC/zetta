//! The daemon's background threads: the reaper, the drain loop, and the
//! liveness sweep that reclaims panes from clients that are gone.
//!
//! The drain loop is the only thing that reads a held pty, so its pacing is
//! what a shared pane's output latency and backpressure come from.

use super::*;

/// Reaps children and tells whoever is holding their terminals.
///
/// Every `Pty` registers its own `SIGCHLD` pipe, so on any child's exit each
/// pane is asked in turn and only the one that actually exited answers.
#[cfg(unix)]
pub(super) fn start_reaper(daemon: Arc<Daemon>) -> Result<()> {
    spawn_worker("zmux reaper", move || {
        let (signals, reader) = Stream::pair().expect("creating the SIGCHLD pipe");
        signal_hook::low_level::pipe::register(libc::SIGCHLD, signals)
            .expect("watching for terminal process exits");
        let daemon = daemon.clone();
        Box::new(move || reaper_loop(daemon, reader))
    });
    Ok(())
}

#[cfg(unix)]
pub(super) fn reaper_loop(daemon: Arc<Daemon>, mut reader: Stream) {
    let mut byte = [0; 1];
    while reader.read(&mut byte).is_ok() {
        let mut exits = Vec::new();
        {
            let mut sessions = daemon.sessions.lock().unwrap();
            for session in sessions.iter_mut() {
                for pane in session.panes.iter_mut() {
                    if pane.exited {
                        continue;
                    }
                    if let Some(exit) = observe_pane_exit(session.id, pane) {
                        exits.push(exit);
                    }
                }
            }
        }
        for (session_id, pane_id, raw_status, input_sent) in exits {
            broadcast(
                &daemon,
                &Event::PaneExited {
                    session_id,
                    pane_id,
                    raw_status,
                    input_sent,
                },
            );
        }
        // A pane that exited while detached has nothing left to attach to:
        // end the session rather than keep offering a dead terminal. A
        // pane a client is still reading is kept until it lets go, which
        // the reclaim and detach paths then prune.
        if prune_exited_panes(&daemon) {
            publish(&daemon);
        }
    }
}

/// Notices that a pane's process ended, and records what was observed.
///
/// Returns the exit to broadcast, or `None` if the pane is still running. The
/// status is stored on the pane as well as broadcast, because a broadcast
/// reaches whoever is listening at that instant and only the parent can ever
/// observe a status — so a client that was not listening has no other way back
/// to it.
///
/// Shared by both platforms' reapers and by the drain thread's recovery sweep,
/// so "marked exited" and "status recorded" cannot drift apart.
pub(super) fn observe_pane_exit(
    session_id: u64,
    pane: &mut Pane,
) -> Option<(u64, u64, Option<i32>, bool)> {
    if pane.exited {
        return None;
    }
    let raw_status = match pane.pty.next_child_event() {
        Some(ChildEvent::Exited(status)) => exit_status_raw(status),
        // The child ended but its status could not be obtained. Still an exit:
        // whoever is showing the pane has to be told, or it waits forever.
        Some(_) => None,
        None => return None,
    };
    pane.exited = true;
    pane.exit_status = raw_status;
    Some((
        session_id,
        pane.id,
        raw_status,
        shared_input_sent(&pane.attachment),
    ))
}

/// Whether any shared client typed into a pane. Only the shared data plane
/// can know: input from shared clients travels through the daemon, whereas an
/// exclusive client types through its own descriptor and its own keystrokes
/// are the truth for it.
pub(super) fn shared_input_sent(attachment: &Attachment) -> bool {
    match attachment {
        Attachment::Shared(clients) => clients.iter().any(|client| client.input_sent),
        _ => false,
    }
}

/// The exit as the client will report it.
///
/// Unix carries a wait status and Windows an exit code; each platform's
/// terminal reads it back as its own, so no translation happens in between.
#[cfg(unix)]
pub(super) fn exit_status_raw(status: std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt as _;
    Some(status.into_raw())
}

#[cfg(windows)]
pub(super) fn exit_status_raw(status: std::process::ExitStatus) -> Option<i32> {
    status.code()
}

/// Drains panes no client is reading directly.
///
/// A detached pane still has to be read or its child blocks on a full buffer,
/// so this runs whatever the retention setting is; only what is *kept* differs.
/// A shared pane is drained here too — it is the daemon, not any client, that
/// reads it — and what is read is relayed to every shared client, which is
/// the shared mode's data plane.
pub(super) fn start_drain(daemon: Arc<Daemon>) -> Result<()> {
    spawn_worker("zmux drain", move || {
        let (wake, waker) = Stream::pair().expect("creating the drain wake channel");
        waker
            .set_nonblocking(true)
            .expect("making the drain wake channel non-blocking");
        *daemon.drain_wake.lock().unwrap() = Some(wake);
        let daemon = daemon.clone();
        Box::new(move || drain_loop(daemon, waker))
    });
    Ok(())
}

pub(super) fn drain_loop(daemon: Arc<Daemon>, mut waker: Stream) {
    let mut buffer = vec![0; 16 * 1024];
    let mut evicted = false;
    let mut last_liveness_check = std::time::Instant::now();
    let mut instant_idle_waits = 0u32;
    loop {
        // A client that exits without detaching leaves its panes marked
        // as taken, and a pane nobody reads blocks its program as soon as
        // the terminal's buffer fills. Checking periodically rather than
        // per iteration keeps this off the hot path.
        // The same cadence covers a second recovery. The reaper is woken by
        // `SIGCHLD` and then asks each pane whether it was the one that ended,
        // which needs that pane's own signal byte to have been written already —
        // and that write is not ordered against the reaper's wake. So an exit
        // can be missed, and with a single pane there is no later signal to
        // notice it on, leaving a terminal waiting forever for an exit that had
        // already happened. Sweeping on a timer is what stops the exit path
        // depending on winning that race.
        //
        // Deliberately not every drain tick: this costs a syscall per pane and
        // the drain runs at fifty hertz, whereas recovering a rare missed signal
        // a second or two late is indistinguishable from recovering it at once.
        let mut missed_exits = Vec::new();
        if last_liveness_check.elapsed() >= CLIENT_LIVENESS_INTERVAL {
            last_liveness_check = std::time::Instant::now();
            reclaim_panes_from_departed_clients(&daemon);
            let mut sessions = daemon.sessions.lock().unwrap();
            for session in sessions.iter_mut() {
                for pane in session.panes.iter_mut() {
                    if let Some(exit) = observe_pane_exit(session.id, pane) {
                        missed_exits.push(exit);
                    }
                }
            }
        }
        let mut idle = true;
        {
            let mut sessions = daemon.sessions.lock().unwrap();
            for session in sessions.iter_mut() {
                let session_id = session.id;
                for pane in session.panes.iter_mut() {
                    if !drain_reads(&pane.attachment) {
                        continue;
                    }
                    // Whatever the terminal could not take when it arrived.
                    if !pane.pending_input.is_empty() {
                        flush_pending_input(pane);
                        idle = false;
                    }
                    if pane.exited {
                        // The child is gone, but the master still holds
                        // what it wrote while dying — the pane's last
                        // lines. Read those out now: once a pane is marked
                        // exited the reaper has already broadcast its
                        // PaneExited, and nobody else will ever drain it.
                        loop {
                            match read_pane(&mut pane.pty, &mut buffer) {
                                Ok(0) | Err(_) => break,
                                Ok(read) => {
                                    pane.retained.push(&buffer[..read]);
                                    #[cfg(feature = "session-persistence")]
                                    record_persistence_output(
                                        &daemon,
                                        session_id,
                                        pane.id,
                                        &buffer[..read],
                                    );
                                    record_handover_output(pane, &buffer[..read]);
                                    relay_output(pane, &buffer[..read]);
                                }
                            }
                        }
                        continue;
                    }
                    // Held off while a viewer catches up. The wait below returns
                    // at once for a pane whose terminal is readable, and the
                    // instant-wait backoff turns that into a re-check about every
                    // millisecond — fine grained next to the threshold.
                    let held_off = relay_backpressure(pane, &mut evicted);
                    if evicted {
                        // A viewer dropped for not reading can leave one behind,
                        // and that is a departure like any other.
                        offer_exclusive_if_alone(&daemon, session_id, pane);
                        evicted = false;
                    }
                    if held_off {
                        continue;
                    }
                    // Read the pane out as far as the buffer goes before
                    // relaying any of it. One relayed frame per *read* made the
                    // frame count follow how eagerly the drain wakes rather than
                    // how much output there is — and since the wait became
                    // event-driven, that is once every few bytes. A burst then
                    // arrived as thousands of tiny frames, each with its own
                    // header, socket write and parse at the far end, which cost
                    // the relay far more than it cost to produce and made every
                    // viewer look like it could not keep up.
                    //
                    // Non-blocking on both platforms, so this returns promptly
                    // whether or not the pane has more to give.
                    let mut filled = 0;
                    while filled < buffer.len() {
                        match read_pane(&mut pane.pty, &mut buffer[filled..]) {
                            Ok(0) | Err(_) => break,
                            Ok(read) => filled += read,
                        }
                    }
                    if filled > 0 {
                        pane.retained.push(&buffer[..filled]);
                        #[cfg(feature = "session-persistence")]
                        record_persistence_output(&daemon, session_id, pane.id, &buffer[..filled]);
                        record_handover_output(pane, &buffer[..filled]);
                        idle = false;
                        relay_output(pane, &buffer[..filled]);
                    }
                }
            }
        }
        if !missed_exits.is_empty() {
            for (session_id, pane_id, raw_status, input_sent) in missed_exits {
                log::debug!(
                    "pane {pane_id}'s exit was noticed by the drain rather than the reaper"
                );
                broadcast(
                    &daemon,
                    &Event::PaneExited {
                        session_id,
                        pane_id,
                        raw_status,
                        input_sent,
                    },
                );
            }
            if prune_exited_panes(&daemon) {
                publish(&daemon);
            }
            continue;
        }
        if idle {
            // A pty whose child has gone reports hangup continuously until the
            // reaper marks the pane exited, so a wait over it returns at once,
            // every time, with nothing to read. Only a *run* of such waits is
            // that case: a wait cut short by real output is followed by a pass
            // that reads something, which clears this.
            if instant_idle_waits >= INSTANT_IDLE_WAITS_BEFORE_BACKING_OFF {
                thread::sleep(HANGUP_BACKOFF);
            }
            let started = Instant::now();
            wait_for_drainable(&daemon, &mut waker, idle_wait(last_liveness_check));
            if started.elapsed() < HANGUP_BACKOFF {
                instant_idle_waits = instant_idle_waits.saturating_add(1);
            } else {
                instant_idle_waits = 0;
            }
        } else {
            instant_idle_waits = 0;
        }
    }
}

/// Whether this pane must not be read yet, because a viewer has fallen behind.
///
/// Backpressure, rather than buffering without limit or dropping the viewer.
/// Leaving the bytes in the terminal is what makes the *program* wait — exactly
/// what happens when a client reads its own pty — so a shared pane costs a program
/// what an exclusive one costs instead of letting it outrun every viewer.
///
/// A viewer that has stopped reading altogether is the other case, and is dropped
/// here rather than waited for: its backlog will never shrink, and holding the pane
/// for it would freeze the pane for every other viewer.
pub(super) fn relay_backpressure(pane: &mut Pane, evicted: &mut bool) -> bool {
    let now = Instant::now();
    let mut hold = false;
    let mut stalled = Vec::new();
    if let Attachment::Shared(clients) = &mut pane.attachment {
        for client in clients.iter_mut() {
            let written = client.relay.written.load(Ordering::Relaxed);
            if written != client.written_seen {
                client.written_seen = written;
                client.wrote_at = now;
            }
            let backlog = client.relay.queued.load(Ordering::Relaxed);
            // Checked at *any* backlog, not only past the threshold. A blocked
            // relay drains what is queued into a socket that has room, so the
            // backlog dips below the threshold between passes — and gating the
            // check on the threshold meant a viewer that had stopped reading
            // entirely was never noticed here at all, leaving the pane to stutter
            // until the write timeout eventually killed the relay seconds later.
            if backlog > 0 && now.duration_since(client.wrote_at) >= RELAY_STALL_TIMEOUT {
                stalled.push(client.client_id.clone());
                continue;
            }
            if backlog >= RELAY_BACKPRESSURE_BYTES {
                hold = true;
            }
        }
        if !stalled.is_empty() {
            log::debug!(
                "dropping {} shared viewer(s) whose backlog stopped shrinking",
                stalled.len()
            );
            clients.retain(|client| !stalled.contains(&client.client_id));
        }
    }
    if !stalled.is_empty() {
        collapse_empty_shared(&mut pane.attachment, pane.handover_waiters);
        *evicted = true;
    }
    hold
}

/// Whether the drain thread is the one reading this pane.
///
/// An exclusive client reads its own pane, and a pane under revoke is still
/// being read by its holder until the snapshot arrives. Shared by the drain's
/// read pass and by the set of terminals it waits on, so the two cannot drift
/// apart — waiting on a pane nobody then reads would stall the drain, and
/// reading one it never waits on would put that pane back on a timer.
pub(super) fn drain_reads(attachment: &Attachment) -> bool {
    match attachment {
        // Mid-handover in either direction, nothing reads the pane: the holder
        // still has the descriptor on the way in, and the frames already queued
        // have to land before the descriptor does on the way out.
        Attachment::Exclusive(_) | Attachment::Revoking { .. } | Attachment::Granting { .. } => {
            false
        }
        Attachment::None | Attachment::Shared(_) => true,
    }
}

/// How long a wait may last with nothing happening.
///
/// Bounded by the liveness sweep and nothing else: the wait ends by itself when
/// there is something to do, so the only reason to return empty-handed is to run
/// that sweep on schedule.
pub(super) fn idle_wait(last_liveness_check: Instant) -> Duration {
    CLIENT_LIVENESS_INTERVAL
        .saturating_sub(last_liveness_check.elapsed())
        .max(HANGUP_BACKOFF)
}

/// How long to pause after a run of waits that returned instantly with nothing
/// to read, which is what a hung-up terminal looks like before it is reaped.
pub(super) const HANGUP_BACKOFF: Duration = Duration::from_millis(1);

/// How many such waits to allow before pausing. More than one, because a single
/// instant return is the ordinary case of output arriving.
pub(super) const INSTANT_IDLE_WAITS_BEFORE_BACKING_OFF: u32 = 2;

/// Blocks until a pane the drain is responsible for has output, a client
/// attaches or detaches, or the wait runs out.
///
/// This is what makes shared mode responsive. Sleeping a fixed twenty
/// milliseconds instead put half of that on every shared keystroke's round
/// trip — measured at a ten millisecond median, against ten *microseconds* for
/// a client reading the pty itself — and the waker could not shorten it,
/// because it was drained *before* the sleep rather than waited on, so
/// `wake_drain` had no effect at all.
#[cfg(unix)]
pub(super) fn wait_for_drainable(daemon: &Arc<Daemon>, waker: &mut Stream, timeout: Duration) {
    use std::os::fd::AsRawFd as _;
    let mut fds = vec![libc::pollfd {
        fd: waker.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    }];
    {
        let sessions = daemon.sessions.lock().unwrap();
        for session in sessions.iter() {
            for pane in session.panes.iter() {
                // An exited pane is drained to the end by the pass above and
                // then reports hangup for ever; waiting on it would never block.
                if pane.exited || !drain_reads(&pane.attachment) {
                    continue;
                }
                fds.push(libc::pollfd {
                    fd: pane.pty.file().as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                });
            }
        }
    }
    let millis = timeout.as_millis().min(i32::MAX as u128) as libc::c_int;
    // SAFETY: `fds` is a valid, initialised slice of exactly the length given,
    // and the descriptors outlive the call — the sessions lock is released only
    // after they are collected, and a pane closing while this waits shows up as
    // `POLLNVAL`, which ends the wait rather than corrupting it.
    unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, millis) };
    drain_waker(waker);
}

#[cfg(windows)]
pub(super) fn wait_for_drainable(_daemon: &Arc<Daemon>, waker: &mut Stream, timeout: Duration) {
    // A pseudoconsole's pipes cannot be waited on alongside the wake channel in
    // one call the way a pty's descriptors can, so this keeps the fixed tick and
    // the latency that comes with it.
    drain_waker(waker);
    thread::sleep(timeout.min(WINDOWS_DRAIN_TICK));
}

#[cfg(windows)]
pub(super) const WINDOWS_DRAIN_TICK: Duration = Duration::from_millis(20);

/// Empties the wake channel.
///
/// Every byte has to go: a wake channel left readable makes the next wait return
/// immediately, and the one after that, which is a busy loop rather than a wait.
pub(super) fn drain_waker(waker: &mut Stream) {
    let mut buffer = [0; 64];
    while waker
        .read(&mut buffer)
        .is_ok_and(|read| read == buffer.len())
    {}
}

/// Writes as much queued shared input as the terminal will take right now.
///
/// A partial write is normal on a non-blocking master and is not an error: what
/// is left stays queued and the drain thread tries again, so a paste larger than
/// the terminal's free buffer arrives in full rather than in part.
pub(super) fn flush_pending_input(pane: &mut Pane) {
    use alacritty_terminal::tty::EventedReadWrite as _;
    if pane.pending_input.is_empty() {
        return;
    }
    let mut written = 0;
    while written < pane.pending_input.len() {
        match pane.pty.writer().write(&pane.pending_input[written..]) {
            Ok(0) => break,
            Ok(count) => written += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => {
                // The terminal is gone, so nothing queued for it can ever be
                // delivered. Dropping it is right; keeping it would make the
                // drain thread retry a write that cannot succeed.
                log::debug!("writing shared input to pane {} failed: {error:#}", pane.id);
                pane.pending_input.clear();
                return;
            }
        }
    }
    pane.pending_input.drain(..written);
}

/// Sends a pane's output to every shared client, dropping clients whose
/// socket can no longer be written. A pane whose shared set empties stops
/// being shared.
pub(super) fn relay_output(pane: &mut Pane, bytes: &[u8]) {
    if !matches!(pane.attachment, Attachment::Shared(_)) {
        return;
    }
    // One frame, shared between the viewers by reference: the fan-out costs a
    // refcount each rather than a copy of the pane's output per viewer.
    let frame = match crate::transport::encode_message(&Event::Output {
        pane_id: pane.id,
        length: bytes.len(),
    }) {
        Ok(mut frame) => {
            frame.extend_from_slice(bytes);
            Arc::<[u8]>::from(frame)
        }
        Err(error) => {
            log::warn!("could not frame a pane's output: {error:#}");
            return;
        }
    };
    queue_for_shared_clients(&mut pane.attachment, pane.handover_waiters, &frame);
}

/// Returns a pane whose last shared client has gone to being unheld.
///
/// Only [`remove_shared_client`] used to do this, so a client dropped for being
/// unwritable — a wedged viewer past the relay's write timeout — left the pane
/// "shared with nobody": still drained, but never exclusively attachable again
/// and never pruned, because both require [`Attachment::None`].
pub(super) fn collapse_empty_shared(attachment: &mut Attachment, handover_waiters: usize) {
    if handover_waiters == 0
        && matches!(attachment, Attachment::Shared(clients) if clients.is_empty())
    {
        *attachment = Attachment::None;
    }
}

/// How often the daemon checks that the clients holding panes still exist.
pub(super) const CLIENT_LIVENESS_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// Takes back panes whose client is gone.
///
/// Detaching is normally explicit, but a client that crashes — or is killed —
/// never sends it. Without this the pane stays marked as held: the daemon
/// never resumes reading it, so the session appears alive while its program
/// blocks on a terminal nobody is draining, and it can never be handed to
/// anyone else in a usable state.
pub(super) fn reclaim_panes_from_departed_clients(daemon: &Arc<Daemon>) {
    let holders = {
        let sessions = daemon.sessions.lock().unwrap();
        sessions
            .iter()
            .flat_map(|session| session.panes.iter())
            .filter_map(|pane| match pane.attachment {
                Attachment::Exclusive(holder) => Some(holder),
                // A revoke whose holder died can never be answered; reclaim lets
                // the waiting attach take the pane.
                Attachment::Revoking { holder } => Some(holder),
                _ => None,
            })
            .collect::<std::collections::BTreeSet<_>>()
    };
    if holders.is_empty() {
        return;
    }

    let departed = holders
        .into_iter()
        .filter(|pid| !process_is_running(*pid))
        .collect::<std::collections::BTreeSet<_>>();
    if departed.is_empty() {
        return;
    }

    let mut reclaimed = false;
    let mut sessions = daemon.sessions.lock().unwrap();
    // Only the panes are reclaimed. A session's *scope* is deliberately left
    // alone: a private backgrounding is not a slow way of sharing it, so a
    // window exiting must not turn a session another Zetta could not see into
    // one it can attach. `Request::SetSessionScope` is how that happens, and it
    // is a request somebody makes. A session already offered by its window is
    // already shared, so reclaiming its pane does not need to widen anything.
    for session in sessions.iter_mut() {
        for pane in session.panes.iter_mut() {
            let departed_holder = match pane.attachment {
                Attachment::Exclusive(holder) => departed.contains(&holder),
                Attachment::Revoking { holder } => departed.contains(&holder),
                _ => false,
            };
            if departed_holder {
                log::info!(
                    "reclaiming pane {} from client {}, which exited without detaching",
                    pane.id,
                    match pane.attachment {
                        Attachment::Exclusive(holder) | Attachment::Revoking { holder } => holder,
                        _ => unreachable!(),
                    }
                );
                pane.attachment = Attachment::None;
                #[cfg(windows)]
                pane.pty.resume_reader();
                reclaimed = true;
            }
        }
    }

    let (_, _consoles) = end_abandoned_sessions(&mut sessions);
    drop(sessions);
    #[cfg(windows)]
    for console_id in _consoles {
        close_host_console(daemon, console_id);
    }
    if reclaimed {
        // An attach may be waiting on this pane's revoke handover.
        daemon.sessions_condvar.notify_all();
    }
    // A pane a departed client was showing may have ended while attached: it
    // was kept until the client let go, and now that it has, it has nothing
    // left to attach to.
    prune_exited_panes(daemon);
    publish(daemon);
}

/// Ends sessions that no client holds and that nobody asked to keep.
///
/// Being held is what the user asked for by detaching; it is not something a
/// client's death or a closed pane confers. A session that was never detached
/// belonged to a window that is now gone, so it ends with that window — which is
/// what "visible tabs do not become background sessions implicitly" means.
/// Promoting them instead turned every Zetta that crashed or was killed into a
/// pile of sessions holding stray shells the user never asked for and could not
/// meaningfully attach.
///
/// Dropping a `Session` drops its panes' PTYs, which hangs up their children, so
/// this is also what stops an orphaned shell from lingering. Returns whether
/// anything ended.
pub(super) fn end_abandoned_sessions(sessions: &mut Vec<Session>) -> (bool, Vec<u64>) {
    let before = sessions.len();
    #[cfg(windows)]
    let mut consoles = Vec::new();
    #[cfg(not(windows))]
    let consoles = Vec::new();
    sessions.retain(|session| {
        let abandoned = !session.keep && session.panes.iter().all(|pane| pane.attachment.is_none());
        if abandoned {
            log::info!(
                "ending session {}, which no window holds and nobody asked to keep",
                session.id
            );
            #[cfg(windows)]
            consoles.extend(session.panes.iter().map(|pane| pane.console_id));
        }
        !abandoned
    });
    (sessions.len() != before, consoles)
}

pub(super) fn process_is_running(process_id: u32) -> bool {
    let process_id = sysinfo::Pid::from_u32(process_id);
    let mut system = sysinfo::System::new();
    system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[process_id]), true);
    system.process(process_id).is_some()
}

pub(super) fn wake_drain(daemon: &Arc<Daemon>) {
    if let Some(wake) = daemon.drain_wake.lock().unwrap().as_mut() {
        use std::io::Write as _;
        let _ = wake.write_all(b".");
    }
}

#[cfg(windows)]
pub(super) fn close_host_console(daemon: &Daemon, console_id: u64) {
    if let Err(error) = daemon.pty_host.close(console_id) {
        log::debug!("could not close pseudoconsole {console_id}: {error:#}");
    }
}

#[cfg(all(test, unix))]
#[path = "../tests/server/workers.rs"]
mod tests;
