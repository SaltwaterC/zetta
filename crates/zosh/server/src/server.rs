use crate::agent::AgentServer;
use crate::args::Config;
use crate::child_exit::{self, ChildExitWatch};
#[cfg(any(unix, windows))]
use crate::lifecycle;
use crate::protocol::{
    AgentHostRecord, ReceiveOutcome, ServerTransport, encode_host_message_with_agent,
};
use crate::session_io::{LoopWait, PTY_CHUNK, PtyEvent, PtyIo, PtyWrite, UdpIo};
use crate::sleep_guard::{self, IdleSleepGuard};
use crate::terminal_queries::QueryResponder;
use crate::terminal_state::TerminalState;
use crate::timing;
use crate::user_stream::{UserEvent, UserStreamTracker};
use crate::wake::{WakeDeadline, Waker};
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use moshcatty::Ocb;
use moshcatty::pb::HostInstruction;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::collections::VecDeque;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::Path;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const INITIAL_ROWS: u16 = 24;
const INITIAL_COLS: u16 = 80;
const ASSOCIATION_TIMEOUT: Duration = Duration::from_secs(60);
const IO_BUDGET: Duration = Duration::from_millis(2);
// Match stock Mosh's late acknowledgement grace period. PTY output is not
// proof that the shell has processed a particular input frame.
const ECHO_DELAY: Duration = Duration::from_millis(50);
// Stock Mosh's SEND_MINDELAY: how long a frame waits after the screen first
// changes, so the rest of a burst of output lands in the same frame. A
// program redrawing a screen writes it in several reads' worth of bytes, and
// without this each one became a frame of its own.
const FRAME_MINDELAY: Duration = Duration::from_millis(8);
// How long an echo acknowledgement with no screen change to go with it waits
// for one. While someone types, each key's acknowledgement falls due between
// two screen updates, and sent on its own it was a third of all frames. Long
// enough to reach the next update at key-repeat speed; short enough not to
// push a slow link's prediction confirmations towards Mosh's 250 ms glitch
// threshold.
const ECHO_PIGGYBACK: Duration = Duration::from_millis(30);
const MAX_ROWS: u16 = 1024;
const MAX_COLS: u16 = 1024;
const MAX_CELLS: u32 = 262_144;
const UDP_BUFFER: usize = 65_535;
// Bounds on a client-announced keep-alive interval. The client validates its
// own, but this one arrives over the network, so it is clamped rather than
// trusted: the floor is Mosh's minimum frame interval and the ceiling its
// unassisted heartbeat, past which a keep-alive asks for nothing.
const KEEP_ALIVE_MIN: Duration = Duration::from_millis(20);
const KEEP_ALIVE_MAX: Duration = Duration::from_millis(3000);
// How long the server goes on keeping its half alive after the last thing it
// heard. Matching Mosh's own ACTIVE_RETRY_TIMEOUT is what stops a session
// whose client has vanished from transmitting into the void forever, while
// still covering a power-management stall an order of magnitude longer than
// any that has been observed.
pub(crate) const KEEP_ALIVE_LINGER: Duration = Duration::from_secs(10);
// How long the peer may go unheard before its scrollback budget stops
// holding the program back. Up to here the program is slowed to what the
// client can take, which is what makes the history complete; past here the
// session is one whose client may never return, and a Mosh session outliving
// its client matters more than the history it is accumulating for nobody.
// Matching KEEP_ALIVE_LINGER is deliberate: it is the same judgement about
// when a peer has stopped being a peer.
const SCROLLBACK_STALL: Duration = KEEP_ALIVE_LINGER;

struct PtySession {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    master: Box<dyn portable_pty::MasterPty>,
    /// The loop's end of `master`.
    io: PtyIo,
    exited: bool,
    /// Wakes the loop when the child exits. Without one the child is polled
    /// every `child_exit::FALLBACK_POLL`, from `next_child_poll`.
    exit_watch: Option<ChildExitWatch>,
    next_child_poll: Instant,
}

impl PtySession {
    /// Whether this pass should ask the child whether it has exited.
    fn child_may_have_exited(&mut self) -> bool {
        if let Some(watch) = &self.exit_watch {
            return watch.exited();
        }
        let now = Instant::now();
        if now < self.next_child_poll {
            return false;
        }
        self.next_child_poll = now + child_exit::FALLBACK_POLL;
        true
    }

    /// When the loop has to wake to poll the child, if it has to at all.
    fn child_poll_deadline(&self) -> Option<Instant> {
        (!self.exited && self.exit_watch.is_none()).then_some(self.next_child_poll)
    }
}

#[cfg_attr(
    windows,
    allow(
        unused_mut,
        reason = "only the Unix server setup mutates this configuration"
    )
)]
pub fn run(mut cfg: Config) -> Result<()> {
    let timing_file = timing::open()?;
    let (socket, port) = bind_udp(cfg.bind_ip, cfg.port_low, cfg.port_high)?;

    let mut key = [0u8; 16];
    getrandom::fill(&mut key).map_err(|error| anyhow!("generating Mosh session key: {error}"))?;
    let key_text = STANDARD_NO_PAD.encode(key);
    let ocb = Ocb::new(&key).context("initializing AES-128-OCB3")?;
    key.fill(0);

    if cfg.forward_agent {
        crate::agent::prime_forwarded_agent(cfg.verbose > 1);
    }

    // This is the bootstrap contract parsed by the stock mosh wrapper.
    println!("MOSH CONNECT {port} {key_text}");
    std::io::stdout().flush().context("flushing MOSH CONNECT")?;

    #[cfg(windows)]
    let _detached_stdout = if cfg.internal_child {
        Some(lifecycle::release_bootstrap_stdout()?)
    } else {
        None
    };

    #[cfg(unix)]
    if !cfg.foreground {
        lifecycle::detach_after_connect_line()?;
        // Standard streams are now /dev/null. Avoid attempting diagnostics on
        // them in the detached child.
        cfg.verbose = 0;
    }

    if cfg.verbose > 0 {
        eprintln!(
            "zosh-server-rs: UDP {}, protocol 2, PTY backend native",
            socket.local_addr().context("reading bound UDP address")?
        );
    }

    timing::start(timing_file);
    timing::record("session_start", 0, 0);
    let result = serve_session(cfg, socket, ServerTransport::new(ocb));
    timing::record("session_end", u64::from(result.is_err()), 0);
    timing::finish();
    result
}

/// The session loop. It waits for its socket, its PTY and whatever wakes it
/// (see `session_io`), and between them sleeps until the next timer it owes
/// anything to (`next_wake`), so an idle session costs a wakeup per heartbeat
/// or keep-alive rather than one every few milliseconds.
fn serve_session(cfg: Config, socket: UdpSocket, mut transport: ServerTransport) -> Result<()> {
    // Built here, after the Unix bootstrap has forked: the Windows readers
    // are threads, and a thread must never cross a fork.
    let mut wait = LoopWait::new().context("creating the session loop's wake-up")?;
    let waker = wait.waker();
    let mut udp = UdpIo::new(socket, &waker).context("setting up the UDP socket")?;
    let mut udp_buf = vec![0u8; UDP_BUFFER];
    // The first authenticated user state decides whether this is a Zosh peer
    // that negotiated forwarding. Until then no child, PTY, or agent socket
    // exists, so an inherited bootstrap SSH_AUTH_SOCK cannot leak into a
    // stock-Mosh session.
    let mut pty: Option<PtySession> = None;
    let mut agent_server: Option<AgentServer> = None;
    let mut agent_decided = false;
    let mut terminal = TerminalState::new(INITIAL_ROWS, INITIAL_COLS);
    // Until the first client state arrives the server does not know whether it
    // is talking to something that can take scrolled-off rows, and it is
    // already producing them. It collects on spec and settles the question
    // here, once, from the first state it accepts.
    let mut scrollback_settled = false;
    let mut clipboard_supported = false;
    let mut responder = QueryResponder::new();
    let mut user_stream = UserStreamTracker::new();
    let mut peer: Option<SocketAddr> = None;
    let association_deadline = Instant::now() + ASSOCIATION_TIMEOUT;
    let network_timeout = configured_network_timeout();

    let mut dirty = true;
    let mut echo = EchoAcknowledgements::default();
    let mut enqueued_echo: u64 = 0;
    let mut local_shutdown = false;
    let mut remote_shutdown = false;
    let mut loop_timing = timing::LoopTiming::new();
    // The interval a client has asked this session to be held to, once it has
    // announced one, and when this side last put a datagram on the wire.
    let mut keep_alive: Option<Duration> = None;
    let mut last_send = Instant::now();
    // The last time a send was attempted and nothing left, which keeps a
    // keep-alive that cannot go out from waking the loop continuously.
    let mut send_failed_at: Option<Instant> = None;
    let mut frames = FramePacer::default();
    let mut sleep_guard = IdleSleepGuard::new();

    loop {
        #[cfg(test)]
        LOOP_PASSES.fetch_add(1, Ordering::Relaxed);
        loop_timing.tick();
        let phase = timing::begin();
        // Reading the program is what produces scrolled-off rows, so a client
        // that is behind on them is kept up with by not reading — the PTY
        // queue fills, its reader thread blocks, and the program waits, the
        // way it would on an SSH session whose window has closed. A peer that
        // has stopped answering altogether is not worth stalling a program
        // for; past `SCROLLBACK_STALL` the oldest rows are dropped instead.
        let hold_for_scrollback = if terminal.scrollback_over_budget() {
            if transport.last_recv().elapsed() >= SCROLLBACK_STALL {
                terminal.drop_scrollback_over_budget();
                false
            } else {
                true
            }
        } else {
            false
        };
        let pty_progress = if hold_for_scrollback {
            PtyProgress::default()
        } else if let Some(session) = pty.as_mut() {
            session.io.flush();
            drain_pty_events(
                &mut session.io,
                &mut terminal,
                &mut responder,
                clipboard_supported,
                &mut echo,
                cfg.verbose > 0,
            )?
        } else {
            PtyProgress::default()
        };
        dirty |= pty_progress.dirty;
        if pty_progress.ended
            && let Some(session) = pty.as_mut()
        {
            session.exited = true;
        }
        timing::slow("pty_drain_slow", phase);

        let confirmed_echo = echo.advance(Instant::now());

        // Drain authenticated UDP datagrams. The peer address follows only
        // an authenticated *in-order* datagram (`roam`), which implements
        // Mosh's IP/port roaming without letting either a forged or a
        // replayed packet rebind it.
        let phase = timing::begin();
        let udp_started = Instant::now();
        let mut udp_budget_exhausted = true;
        for _ in 0..128 {
            if udp_started.elapsed() >= IO_BUDGET {
                break;
            }
            match udp.recv(&mut udp_buf) {
                Ok(Some((length, addr))) => {
                    let bytes = &udp_buf[..length];
                    let outcome = transport.receive(bytes)?;
                    if outcome.authenticated {
                        timing::record("udp_authenticated", bytes.len() as u64, 0);
                    }
                    roam(&mut peer, &outcome, addr);

                    if let Some(state) = outcome.state {
                        timing::record("input_state", state.new_num, state.ack_num);
                        terminal.acknowledge(transport.acked_by_remote());
                        if transport.take_rebase_required() {
                            dirty = true;
                        }

                        if state.new_num == u64::MAX || transport.ack_num() == u64::MAX {
                            remote_shutdown = true;
                        }

                        if !remote_shutdown {
                            let accepted = user_stream.accept(&state)?;
                            timing::record(
                                "input_apply",
                                accepted.frame,
                                accepted.events.len() as u64,
                            );
                            if let Some(announced) = accepted.keep_alive_ms {
                                // The client is holding the link open
                                // against radio power management, so the
                                // answer is worth more than the delayed
                                // ack's chance to carry data with it.
                                // `send_updates` runs later in this same
                                // pass.
                                timing::record("keepalive", accepted.frame, u64::from(announced));
                                keep_alive = Some(
                                    Duration::from_millis(u64::from(announced))
                                        .clamp(KEEP_ALIVE_MIN, KEEP_ALIVE_MAX),
                                );
                                transport.force_next_send();
                            }
                            if let Some(kib) = accepted.scrollback_kib {
                                // The one negotiation the extension has.
                                timing::record(
                                    "scrollback_request",
                                    accepted.frame,
                                    u64::from(kib),
                                );
                                terminal.set_scrollback_budget((kib as usize).saturating_mul(1024));
                                scrollback_settled = true;
                            } else if !scrollback_settled {
                                // A client announces on its first instruction,
                                // and the announcement is cumulative, so a
                                // first state without one is a client that
                                // will never send one: a stock Mosh client.
                                terminal.forget_scrollback();
                                scrollback_settled = true;
                            }
                            clipboard_supported |= accepted.clipboard_version == Some(1);
                            if !agent_decided {
                                let requested = accepted
                                    .events
                                    .iter()
                                    .any(|event| matches!(event, UserEvent::AgentHello { .. }));
                                agent_decided = true;
                                if cfg.forward_agent && requested {
                                    agent_server = Some(AgentServer::new(waker.clone()));
                                }
                                pty = Some(spawn_pty_session(
                                    &cfg,
                                    agent_server.as_ref().and_then(|agent| agent.socket_path()),
                                    &waker,
                                )?);
                            }
                            let events = accepted.events;
                            for event in &events {
                                if let UserEvent::AgentResponse {
                                    connection_id,
                                    request_id,
                                    frame,
                                    closed,
                                } = event
                                    && let Some(agent) = agent_server.as_mut()
                                {
                                    agent.apply_response(
                                        *connection_id,
                                        *request_id,
                                        frame,
                                        *closed,
                                    );
                                }
                            }
                            let terminal_events = events
                                .into_iter()
                                .filter(|event| {
                                    !matches!(
                                        event,
                                        UserEvent::AgentHello { .. }
                                            | UserEvent::AgentResponse { .. }
                                    )
                                })
                                .collect();
                            if let Some(session) = pty.as_mut() {
                                apply_user_events(
                                    terminal_events,
                                    accepted.frame,
                                    session.master.as_ref(),
                                    &mut terminal,
                                    &mut session.io,
                                    &mut dirty,
                                )?;
                            }
                        }
                    }
                }
                Ok(None) => {
                    udp_budget_exhausted = false;
                    break;
                }
                Err(error) => return Err(error).context("receiving UDP datagram"),
            }
        }

        timing::slow("input_drain_slow", phase);
        let associated = peer.is_some() && transport.has_received_authenticated();
        sleep_guard.update(
            sleep_guard::peer_present(associated, transport.last_recv().elapsed()),
            cfg.verbose > 0,
        );
        if !associated && Instant::now() >= association_deadline {
            kill_pty(&mut pty, false);
            bail!("no Mosh client associated within 60 seconds");
        }

        if associated {
            if let Some(agent) = agent_server.as_mut() {
                agent.acknowledge(transport.acked_by_remote());
                if agent.poll() {
                    dirty = true;
                    transport.force_next_send();
                }
            }
            if network_timeout.is_some_and(|timeout| transport.last_recv().elapsed() >= timeout) {
                if cfg.verbose > 0 {
                    eprintln!("zosh-server-rs: network timeout expired");
                }
                kill_pty(&mut pty, false);
                break;
            }

            terminal.acknowledge(transport.acked_by_remote());

            // Build a cumulative HostMessage from the peer's acknowledged
            // visual base. SSP may discard an unsent intermediate state; every
            // newer state is independently valid from the acknowledged base.
            //
            // Built only once one may go out (see `FramePacer`): the
            // transport sends a new state the moment it has one, so a frame
            // built sooner is a frame sent sooner, and each costs a diff of
            // the whole screen, a snapshot and a datagram.
            let owed = Owed {
                screen: !local_shutdown && !remote_shutdown && dirty,
                echo: !local_shutdown && !remote_shutdown && confirmed_echo > enqueued_echo,
            };
            let now = Instant::now();
            if frames
                .due(owed, now, transport.send_interval())
                .is_some_and(|due| now >= due)
            {
                if let Some(state_num) = queue_frame(
                    &mut transport,
                    &terminal,
                    agent_server.as_ref(),
                    confirmed_echo,
                ) {
                    timing::record("host_update", state_num, confirmed_echo);
                    terminal.snapshot_for_state(state_num);
                    if let Some(agent) = agent_server.as_mut() {
                        agent.snapshot_for_state(state_num);
                    }
                    enqueued_echo = enqueued_echo.max(confirmed_echo);
                    frames.sent(now);
                } else {
                    frames.settled();
                }
                dirty = false;
            }

            // The server's own half of the keep-alive, and the reason the
            // client's is not enough on its own: replying to what arrives
            // makes this side go quiet exactly when the other side's
            // packets are the ones being delayed. This timer does not care
            // whether anything arrived.
            //
            // Like the client's, it is a floor on sending rather than an
            // extra timer: an ordinary reply resets `last_send`, so a
            // healthy session pays nothing for it.
            if keep_alive_due(
                keep_alive,
                last_send.elapsed(),
                transport.last_recv().elapsed(),
            ) {
                timing::record("keepalive_send", last_send.elapsed().as_millis() as u64, 0);
                transport.force_next_send();
            }

            match send_updates(&mut transport, &udp, peer, cfg.verbose > 1) {
                SendOutcome::Sent => {
                    last_send = Instant::now();
                    send_failed_at = None;
                }
                SendOutcome::Failed => send_failed_at = Some(Instant::now()),
                SendOutcome::Nothing => {}
            }

            if transport.crypto_exhausted() {
                kill_pty(&mut pty, false);
                bail!("Mosh OCB per-key block limit exhausted; refusing nonce/key reuse");
            }

            if remote_shutdown && transport.counterparty_shutdown_ack_sent() {
                kill_pty(&mut pty, false);
                break;
            }

            if local_shutdown
                && (transport.shutdown_acknowledged() || transport.shutdown_timed_out())
            {
                break;
            }
        }

        let phase = timing::begin();
        if let Some(session) = pty.as_mut()
            && !session.exited
            && session.child_may_have_exited()
            && session
                .child
                .try_wait()
                .context("polling PTY child")?
                .is_some()
        {
            timing::record("child_exited", 0, 0);
            session.exited = true;
        }
        timing::slow("child_poll_slow", phase);

        if pty.as_ref().is_some_and(|session| session.exited) && !local_shutdown {
            if associated && !remote_shutdown {
                local_shutdown = true;
                transport.start_shutdown();
            } else {
                break;
            }
        }

        // If the transport tells us queue compaction invalidated an older branch,
        // or the newest frame rests on a base too old to assume the peer has,
        // rebuild from the current ACK base on the next pass.
        if transport.take_rebase_required() || transport.frame_base_expired() {
            dirty = true;
        }

        // A producer wakes the loop after publishing an event, and a pending
        // wake-up also covers events published between draining the queue
        // and this wait. Never wait while this pass has left work behind: a
        // bounded drain
        // that stopped early, or PTY output that was held back for a
        // scrollback budget this pass's acknowledgements have since freed. A
        // frame that is owed is a deadline like any other: `frame_due`.
        let pty_backlog = hold_for_scrollback && !terminal.scrollback_over_budget();
        let sending = associated && !local_shutdown && !remote_shutdown;
        let owed = Owed {
            screen: sending && dirty,
            echo: sending && confirmed_echo > enqueued_echo,
        };
        let frame_due = frames.due(owed, Instant::now(), transport.send_interval());
        if !pty_progress.budget_exhausted && !udp_budget_exhausted && !pty_backlog {
            let wake = next_wake(WakeSources {
                now: Instant::now(),
                associated,
                association_deadline,
                network_timeout,
                transport: &transport,
                keep_alive,
                last_send,
                send_failed_at,
                terminal: &terminal,
                echo: &echo,
                child_poll: pty.as_ref().and_then(PtySession::child_poll_deadline),
                frame_due,
            });
            // The program is held back while the client is behind on its
            // scrollback (see the top of the pass), and then its output must
            // not end the wait: the stall deadline is what ends that.
            let holding = terminal.scrollback_over_budget()
                && transport.last_recv().elapsed() < SCROLLBACK_STALL;
            loop_timing.parking(wake.earliest());
            wait.wait(
                wake,
                &udp,
                pty.as_ref().map(|session| &session.io),
                !holding,
            )
            .context("waiting for the network or the PTY")?;
        }
    }

    kill_pty(&mut pty, true);
    Ok(())
}

#[derive(Default)]
struct PtyProgress {
    dirty: bool,
    ended: bool,
    budget_exhausted: bool,
}

/// Counts passes of the session loop, so a test can tell an idle session that
/// sleeps from one that polls.
#[cfg(test)]
static LOOP_PASSES: AtomicU64 = AtomicU64::new(0);

/// Everything the loop's clocks are read from, for `next_wake`.
#[derive(Clone, Copy)]
struct WakeSources<'a> {
    now: Instant,
    associated: bool,
    association_deadline: Instant,
    network_timeout: Option<Duration>,
    transport: &'a ServerTransport,
    keep_alive: Option<Duration>,
    last_send: Instant,
    send_failed_at: Option<Instant>,
    terminal: &'a TerminalState,
    echo: &'a EchoAcknowledgements,
    child_poll: Option<Instant>,
    /// When an owed frame may be built; consumed by the pass that builds it.
    frame_due: Option<Instant>,
}

/// Follow a roaming client to `from` if, and only if, this datagram was
/// authenticated and in order, as stock Mosh's `recv_one` does. Neither an
/// authenticated datagram nor a completed state is enough on its own: a
/// captured datagram replayed from another address passes both checks once
/// the transport has forgotten its sequence number, and would redirect the
/// session's output there.
fn roam(peer: &mut Option<SocketAddr>, outcome: &ReceiveOutcome, from: SocketAddr) {
    if outcome.authenticated && outcome.in_order {
        *peer = Some(from);
    }
}

/// The instant the loop next has to wake for if nothing arrives first: the
/// earliest of every timer a pass acts on.
///
/// A deadline that has already passed means "wake at once", so every one
/// here is either consumed by the pass it wakes — the transport sends, the
/// echo acknowledgement advances, the session ends — or left out once it has
/// passed, because a pass has already acted on it and acting again changes
/// nothing. A past deadline that is neither would wake the loop continuously.
fn next_wake(sources: WakeSources<'_>) -> WakeDeadline {
    let WakeSources { now, .. } = sources;
    let last_recv = sources.transport.last_recv();
    let mut wake = WakeDeadline::default();
    wake.at(sources.echo.next_due());
    wake.at(sources.child_poll);
    wake.at(sources.frame_due);
    if sources.terminal.scrollback_over_budget() {
        // Once passed, the pass stops holding the program back for the
        // client, which is all this deadline is for.
        wake.at(Some(last_recv + SCROLLBACK_STALL).filter(|at| *at > now));
    }
    if !sources.associated {
        wake.at(Some(sources.association_deadline));
        return wake;
    }
    wake.at(sources.transport.next_deadline());
    wake.at(
        keep_alive_deadline(sources.keep_alive, sources.last_send, last_recv).map(|due| {
            // A keep-alive that could not be sent stays due; retry it at
            // the fastest interval a client may ask for, not continuously.
            sources
                .send_failed_at
                .map_or(due, |failed| due.max(failed + KEEP_ALIVE_MIN))
        }),
    );
    wake.at(sources.network_timeout.map(|timeout| last_recv + timeout));
    // Once passed, the sleep guard has already been released.
    wake.at(Some(last_recv + sleep_guard::PRESENCE_LINGER).filter(|at| *at > now));
    wake
}

/// Whether this side owes a keep-alive of its own right now.
///
/// The point of it is what it does *not* depend on: anything arriving. A
/// server that only ever replies goes quiet exactly when the other side's
/// packets are the ones being delayed, which is the condition a keep-alive
/// exists to survive.  `KEEP_ALIVE_LINGER` bounds that independence, so a
/// client that has genuinely gone does not leave the server transmitting
/// into the void for the life of a detached session.
///
/// `since_send` is measured from the last datagram that actually left, so
/// an ordinary reply resets it and a healthy session pays nothing.
fn keep_alive_due(
    keep_alive: Option<Duration>,
    since_send: Duration,
    since_recv: Duration,
) -> bool {
    keep_alive.is_some_and(|interval| since_send >= interval && since_recv < KEEP_ALIVE_LINGER)
}

/// The instant `keep_alive_due` next turns true if nothing is sent or heard
/// before then, or `None` when it cannot: no interval is armed, or the linger
/// runs out first.
fn keep_alive_deadline(
    keep_alive: Option<Duration>,
    last_send: Instant,
    last_recv: Instant,
) -> Option<Instant> {
    let due = last_send + keep_alive?;
    (due < last_recv + KEEP_ALIVE_LINGER).then_some(due)
}

/// What a frame would carry if one were built now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Owed {
    /// The screen, or something else only a frame can carry, has changed.
    screen: bool,
    /// An echo acknowledgement has fallen due.
    echo: bool,
}

/// When the next frame may be built, the way stock Mosh's sender decides when
/// the next state may go out: no sooner than `FRAME_MINDELAY` after the
/// screen first changed, so a burst of output is one frame, and no sooner
/// than the transport's frame interval after the last frame, so a program
/// writing continuously is sent at the rate the link can show it rather than
/// at the rate it writes. An echo acknowledgement on its own waits up to
/// `ECHO_PIGGYBACK` for a screen change to travel with.
///
/// The transport would pace states itself if asked, but only by holding back
/// states already built; building them is the expensive part.
#[derive(Debug, Default)]
struct FramePacer {
    /// When the screen first changed since the last frame was built.
    screen_since: Option<Instant>,
    /// When an echo acknowledgement first fell due since then.
    echo_since: Option<Instant>,
    last_frame: Option<Instant>,
}

impl FramePacer {
    /// When the owed frame may be built, or `None` when none is owed.
    fn due(&mut self, owed: Owed, now: Instant, interval: Duration) -> Option<Instant> {
        if !owed.screen {
            self.screen_since = None;
        }
        if !owed.echo {
            self.echo_since = None;
        }
        let screen = owed
            .screen
            .then(|| *self.screen_since.get_or_insert(now) + FRAME_MINDELAY);
        let echo = owed
            .echo
            .then(|| *self.echo_since.get_or_insert(now) + ECHO_PIGGYBACK);
        let collected = match (screen, echo) {
            (Some(screen), Some(echo)) => screen.min(echo),
            (Some(at), None) | (None, Some(at)) => at,
            (None, None) => return None,
        };
        Some(
            self.last_frame
                .map_or(collected, |last| collected.max(last + interval)),
        )
    }

    /// A frame was built and handed to the transport.
    fn sent(&mut self, now: Instant) {
        self.last_frame = Some(now);
        self.settled();
    }

    /// What was owed turned out to need no frame at all.
    fn settled(&mut self) {
        self.screen_since = None;
        self.echo_since = None;
    }
}

/// What a call to `send_updates` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SendOutcome {
    /// The transport had nothing due.
    Nothing,
    /// At least one datagram left this host.
    Sent,
    /// Datagrams were built and none could be sent.
    Failed,
}

/// Send whatever the transport has due, reporting whether anything actually
/// left this host. The keep-alive timer is measured from that, so a datagram
/// the transport built but could not send must not restart it.
fn send_updates(
    transport: &mut ServerTransport,
    socket: &UdpIo,
    peer: Option<SocketAddr>,
    verbose: bool,
) -> SendOutcome {
    let phase = timing::begin();
    let datagrams = transport.tick();
    timing::slow("transport_tick_slow", phase);
    if datagrams.is_empty() {
        return SendOutcome::Nothing;
    }
    let mut sent = false;
    if let Some(addr) = peer {
        for datagram in datagrams {
            match socket.send_to(&datagram, addr) {
                Ok(n) => {
                    sent = true;
                    timing::record("udp_sent", n as u64, 0);
                }
                Err(error) => {
                    timing::record(
                        "udp_send_error",
                        error.raw_os_error().unwrap_or(0) as u64,
                        0,
                    );
                    // Reachability may disappear during roaming; SSP retries.
                    if verbose
                        && !matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                        )
                    {
                        eprintln!("zosh-server-rs: UDP send failed: {error}");
                    }
                }
            }
        }
    }
    timing::slow("transport_send_slow", phase);
    if sent {
        SendOutcome::Sent
    } else {
        SendOutcome::Failed
    }
}

fn drain_pty_events(
    pty: &mut PtyIo,
    terminal: &mut TerminalState,
    responder: &mut QueryResponder,
    clipboard_supported: bool,
    echo: &mut EchoAcknowledgements,
    verbose: bool,
) -> Result<PtyProgress> {
    let mut progress = PtyProgress {
        budget_exhausted: true,
        ..PtyProgress::default()
    };
    let started = Instant::now();
    // Bound work so a noisy process cannot monopolize the network loop.
    for _ in 0..64 {
        if started.elapsed() >= IO_BUDGET {
            break;
        }
        match pty.next_event() {
            Some(PtyEvent::Output(bytes)) => {
                timing::record("pty_output_apply", bytes.len() as u64, 0);
                terminal.process(&bytes);
                let replies = responder.feed(&bytes, terminal.cursor_position(), terminal.size());
                for query in responder.take_terminal_queries() {
                    if !clipboard_supported
                        && (zclip::osc52::is_write(&query)
                            || zclip::protocol::Frame::parse(&query).is_some())
                    {
                        continue;
                    }
                    if terminal.add_query(query.clone()) == 0
                        && let Some(frame) = zclip::protocol::Frame::parse(&query)
                    {
                        pty.queue(PtyWrite::Bytes(
                            zclip::protocol::Frame {
                                id: frame.id,
                                message: zclip::protocol::Message::Error(
                                    "zosh clipboard query backlog is full".into(),
                                ),
                            }
                            .encode(),
                        ))?;
                    }
                }
                progress.dirty = true;
                for reply in replies {
                    pty.queue(PtyWrite::Bytes(reply))?;
                }
            }
            Some(PtyEvent::InputWritten(frame)) => {
                timing::record("input_written_observed", frame, 0);
                // Start the grace period on observation, allowing queued reader
                // events to drain before the frame becomes eligible for ACK.
                echo.written(frame, Instant::now());
            }
            Some(PtyEvent::Error(error)) => {
                if verbose {
                    eprintln!("zosh-server-rs: PTY I/O ended: {error}");
                }
                progress.ended = true;
                progress.budget_exhausted = false;
                break;
            }
            Some(PtyEvent::Eof) => {
                progress.ended = true;
                progress.budget_exhausted = false;
                break;
            }
            None => {
                progress.budget_exhausted = false;
                break;
            }
        }
    }
    Ok(progress)
}

/// Build a frame and queue it, diffed from the state the peer has probably
/// received when that state can be diffed from, and from the acknowledged one
/// otherwise. Returns the new state's number, or `None` when the frame turned
/// out to carry nothing.
fn queue_frame(
    transport: &mut ServerTransport,
    terminal: &TerminalState,
    agent: Option<&AgentServer>,
    confirmed_echo: u64,
) -> Option<u64> {
    let records = |base| agent.map_or_else(Vec::new, |agent| agent.records_after(base));
    if let Some(assumed) = transport
        .frame_base()
        .filter(|state| terminal.has_snapshot(*state))
    {
        let base = Some(assumed);
        // Nothing has changed since a state the peer probably has: there is
        // nothing to send, and if that state was lost the transport resends
        // it.
        let payload = host_update(terminal, base, confirmed_echo, &records(base))?;
        if let Some(state) = transport.set_pending_on(base, payload) {
            return Some(state);
        }
        // The transport no longer accepts that base; build from the
        // acknowledged one instead.
    }
    let payload = host_update(terminal, None, confirmed_echo, &records(None))?;
    transport.set_pending_on(None, payload)
}

fn host_update(
    terminal: &TerminalState,
    base: Option<u64>,
    confirmed_echo: u64,
    agent_records: &[AgentHostRecord],
) -> Option<Vec<u8>> {
    let phase = timing::begin();
    let mut instructions = Vec::with_capacity(3);
    // Stock CompleteTerminal applies distinct instructions with an else-if
    // chain. Preserve its ordering: echo ACK, resize, host bytes.
    // Repeat the current echo ACK in every cumulative diff: an intermediate
    // update may be coalesced away or lost before the remote acknowledges it.
    if confirmed_echo > 0 {
        instructions.push(HostInstruction {
            hoststring: Vec::new(),
            width: 0,
            height: 0,
            echo_ack_num: confirmed_echo.min(i64::MAX as u64) as i64,
        });
    }
    let frame = terminal.frame_from(base)?;
    if let Some((rows, cols)) = frame.resize {
        instructions.push(HostInstruction {
            hoststring: Vec::new(),
            width: i32::from(cols),
            height: i32::from(rows),
            echo_ack_num: -1,
        });
    }
    let host_bytes = frame.host_bytes;
    if !host_bytes.is_empty() {
        instructions.push(HostInstruction {
            hoststring: host_bytes,
            width: 0,
            height: 0,
            echo_ack_num: -1,
        });
    }
    let update = encode_host_message_with_agent(&instructions, frame.queries, agent_records);
    timing::slow("host_diff_slow", phase);
    (!update.is_empty()).then_some(update)
}

fn apply_user_events(
    events: Vec<UserEvent>,
    input_frame: u64,
    master: &dyn portable_pty::MasterPty,
    terminal: &mut TerminalState,
    pty: &mut PtyIo,
    dirty: &mut bool,
) -> Result<()> {
    let mut keys = Vec::with_capacity(PTY_CHUNK);
    let mut any_keys = false;

    let flush_keys = |pty: &mut PtyIo, keys: &mut Vec<u8>| -> Result<()> {
        if keys.is_empty() {
            return Ok(());
        }
        pty.queue(PtyWrite::Bytes(std::mem::take(keys)))?;
        *keys = Vec::with_capacity(PTY_CHUNK);
        Ok(())
    };

    let mut events = events.into_iter().peekable();
    while let Some(event) = events.next() {
        match event {
            UserEvent::Byte(byte) => {
                any_keys = true;
                keys.push(byte);
                if keys.len() >= PTY_CHUNK {
                    flush_keys(pty, &mut keys)?;
                }
            }
            UserEvent::Resize { cols, rows } => {
                // Preserve UserStream ordering: bytes before a resize must reach
                // the PTY before the resize, and bytes after it must see the new
                // terminal dimensions.
                flush_keys(pty, &mut keys)?;
                validate_terminal_size(rows, cols)?;
                // A resize the next event replaces is never seen by anything:
                // no byte reaches the program between the two. A dragged pane
                // edge sends a run of them, and each one applied is a SIGWINCH
                // and a whole-screen redraw for a size that is already gone.
                if matches!(events.peek(), Some(UserEvent::Resize { .. })) {
                    continue;
                }
                master
                    .resize(PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    })
                    .context("resizing PTY/ConPTY")?;
                terminal.resize(rows, cols);
                *dirty = true;
            }
            UserEvent::TerminalResponse(bytes) => {
                // A response is a PTY byte stream event, not keyboard input;
                // flush preceding keys so the remote process sees the exact
                // UserStream order and do not attach an echo acknowledgement.
                flush_keys(pty, &mut keys)?;
                pty.queue(PtyWrite::Bytes(bytes))?;
            }
            UserEvent::AgentHello { .. } | UserEvent::AgentResponse { .. } => {
                // Negotiation and agent frames are consumed by the session
                // loop before terminal events reach this function.
            }
        }
    }

    flush_keys(pty, &mut keys)?;
    if any_keys {
        timing::record("input_queued", input_frame, 0);
        pty.queue(PtyWrite::InputFrame(input_frame))?;
    }
    Ok(())
}

fn validate_terminal_size(rows: u16, cols: u16) -> Result<()> {
    if rows == 0 || cols == 0 || rows > MAX_ROWS || cols > MAX_COLS {
        bail!("unreasonable terminal size {cols}x{rows}");
    }
    if u32::from(rows) * u32::from(cols) > MAX_CELLS {
        bail!("terminal size {cols}x{rows} exceeds the cell budget");
    }
    Ok(())
}

fn spawn_pty_session(
    cfg: &Config,
    agent_socket: Option<&Path>,
    waker: &Waker,
) -> Result<PtySession> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: INITIAL_ROWS,
            cols: INITIAL_COLS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("opening native PTY/ConPTY")?;
    let mut command = build_command(cfg);
    configure_child_environment(&mut command, cfg, agent_socket);
    let child = pair
        .slave
        .spawn_command(command)
        .context("spawning shell/command in PTY")?;
    drop(pair.slave);
    let master = pair.master;
    let io = PtyIo::open(master.as_ref(), waker)?;
    let exit_watch = ChildExitWatch::start(child.as_ref(), waker.clone());
    Ok(PtySession {
        child,
        master,
        io,
        exited: false,
        exit_watch,
        next_child_poll: Instant::now() + child_exit::FALLBACK_POLL,
    })
}

fn kill_pty(pty: &mut Option<PtySession>, wait: bool) {
    if let Some(session) = pty.as_mut()
        && !session.exited
    {
        let _ = session.child.kill();
        if wait {
            let _ = session.child.wait();
        }
        session.exited = true;
    }
}

fn build_command(cfg: &Config) -> CommandBuilder {
    if cfg.command.is_empty() {
        #[cfg(windows)]
        if let Some(shell) = windows_ssh_default_shell() {
            return CommandBuilder::new(shell);
        }
        CommandBuilder::new_default_prog()
    } else {
        CommandBuilder::from_argv(cfg.command.clone())
    }
}

/// Match the shell configured for an ordinary Windows OpenSSH login.
#[cfg(windows)]
fn windows_ssh_default_shell() -> Option<std::ffi::OsString> {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY, RegGetValueW,
    };
    use windows::core::w;

    let flags = RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY;
    let mut size = 0;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\OpenSSH"),
            w!("DefaultShell"),
            flags,
            None,
            None,
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS || size < 2 || size % 2 != 0 {
        return None;
    }
    let mut value = vec![0u16; size as usize / 2];
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\OpenSSH"),
            w!("DefaultShell"),
            flags,
            None,
            Some(value.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let end = value.iter().position(|unit| *unit == 0)?;
    (end > 0).then(|| std::ffi::OsString::from(String::from_utf16_lossy(&value[..end])))
}

fn configure_child_environment(
    command: &mut CommandBuilder,
    cfg: &Config,
    agent_socket: Option<&Path>,
) {
    // SSH bootstrap may have supplied an agent socket for native `ssh -A`.
    // It is not valid after the bootstrap connection closes, so always remove
    // it and add the private Zosh socket only after negotiation.
    command.env_remove("SSH_AUTH_SOCK");
    // Windows OpenSSH describes a child's standard handles to it in a private
    // variable. sshd set one for this server; handed on, it makes every
    // `ssh.exe` the shell starts treat a pipe on its stdin as whatever sshd's
    // handles were, and never read it — `git fetch` hung after `exec`.
    for (name, _) in std::env::vars_os() {
        if is_openssh_handle_state(&name) {
            command.env_remove(name);
        }
    }
    if let Some(agent_socket) = agent_socket {
        command.env("SSH_AUTH_SOCK", agent_socket.as_os_str());
    }
    let term = if cfg.colors >= 256 {
        "xterm-256color"
    } else {
        "xterm"
    };
    command.env("TERM", term);
    command.env("ZOSH_CLIPBOARD_CHANNEL", "1");
    if cfg.colors >= 1 << 15 {
        command.env("COLORTERM", "truecolor");
    }
    for (name, value) in &cfg.locale_env {
        command.env(name, value);
    }
}

/// Win32-OpenSSH's `<GUID>_POSIX_FD_STATE`: meaningful only to the process
/// sshd created, and wrong for anything that inherits it. Matched by suffix so
/// a rebuilt OpenSSH with another prefix is still caught.
fn is_openssh_handle_state(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy()
        .to_ascii_uppercase()
        .ends_with("_POSIX_FD_STATE")
}

fn bind_udp(bind_ip: Option<IpAddr>, low: u16, high: u16) -> Result<(UdpSocket, u16)> {
    let ip = bind_ip.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));

    if low == 0 && high == 0 {
        let socket = UdpSocket::bind(SocketAddr::new(ip, 0))
            .with_context(|| format!("binding UDP socket on {ip}:0"))?;
        let port = socket.local_addr()?.port();
        return Ok((socket, port));
    }

    // Starting at the bottom of the range every time immediately reused the
    // same one or two ports as sessions came and went. Stateful firewalls and
    // NATs can retain the old UDP association after its server exits, making
    // a fresh session on that port a black hole until the mapping expires.
    // Pick a different starting point and still try the complete range, so a
    // busy candidate never reduces the set of ports available to the server.
    let span = u32::from(high) - u32::from(low) + 1;
    let mut random = [0_u8; 2];
    getrandom::fill(&mut random).map_err(|error| anyhow!("choosing a UDP port: {error}"))?;
    let start = u32::from(u16::from_ne_bytes(random)) % span;

    let mut last_error = None;
    for attempt in 0..span {
        let port = candidate_port(low, span, start, attempt);
        match UdpSocket::bind(SocketAddr::new(ip, port)) {
            Ok(socket) => return Ok((socket, port)),
            Err(error) => last_error = Some(error),
        }
    }

    Err(anyhow!(
        "unable to bind any UDP port in {low}:{high} on {ip}: {}",
        last_error.map_or_else(|| "empty port range".to_owned(), |e| e.to_string())
    ))
}

fn candidate_port(low: u16, span: u32, start: u32, attempt: u32) -> u16 {
    let offset = (start + attempt) % span;
    u16::try_from(u32::from(low) + offset).expect("candidate stays inside the u16 port range")
}

fn configured_network_timeout() -> Option<Duration> {
    let value = std::env::var("MOSH_SERVER_NETWORK_TMOUT").ok()?;
    let seconds = value.parse::<u64>().ok()?;
    (seconds > 0).then(|| Duration::from_secs(seconds))
}

#[derive(Default)]
struct EchoAcknowledgements {
    pending: VecDeque<(u64, Instant)>,
    confirmed: u64,
}

impl EchoAcknowledgements {
    fn written(&mut self, frame: u64, now: Instant) {
        self.pending.push_back((frame, now));
    }

    /// When the oldest written frame becomes eligible for acknowledgement.
    fn next_due(&self) -> Option<Instant> {
        self.pending
            .front()
            .map(|(_, written_at)| *written_at + ECHO_DELAY)
    }

    fn advance(&mut self, now: Instant) -> u64 {
        while self
            .pending
            .front()
            .is_some_and(|(_, written_at)| now.saturating_duration_since(*written_at) >= ECHO_DELAY)
        {
            if let Some((frame, _)) = self.pending.pop_front() {
                self.confirmed = self.confirmed.max(frame);
            }
        }
        self.confirmed
    }
}

#[cfg(test)]
#[path = "tests/server.rs"]
mod tests;
