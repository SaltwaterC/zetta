//! The whole client, assembled: a UDP socket, the crypto session, the
//! packet clock, the transport state machine and the state sync.
//!
//! This is where the layers stop being independently testable pieces
//! and become a session. What it does NOT do is own a terminal: it
//! hands host output to the caller as bytes and takes user input as
//! bytes, so the same session drives a real PTY, a test, or an
//! embedding application.

use std::collections::HashSet;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

use crate::crypto::{Direction, Session as CryptoSession};
use crate::error::{MoshError, Result};
use crate::key::Base64Key;
use crate::packet::{Packet, PacketState};
use crate::prediction::PredictionEngine;
use crate::screen::{DiffScreen, OverlayCell, OverlayCursor, Screen};
use crate::sender::{Received, TransportReceiver, TransportSender};
use crate::statesync::{HostEvent, parse_host_diff};
use crate::terminal::ClientTerminal;
use crate::transport::{Fragment, FragmentAssembly, Fragmenter};

/// mosh's own overhead inside a datagram (`Connection::ADDED_BYTES`):
/// the 8-byte nonce prefix and the two timestamps.
const CONNECTION_ADDED_BYTES: usize = 8 + 4;
/// The OCB tag (`Crypto::Session::ADDED_BYTES`).
const CRYPTO_ADDED_BYTES: usize = 16;

/// The datagram size the client builds to, and where it comes from.
///
/// 1280 is not a guess: it is the minimum MTU every IPv6 link is
/// REQUIRED to carry, which makes it the largest size that crosses any
/// path without needing path-MTU discovery to have worked. mosh picked
/// it after finding that VPN traffic over some carrier wifi was dropped
/// at 1320 and above.
const DEFAULT_LINK_MTU: usize = 1280;
/// Headers to leave room for. The IPv6 figure is deliberately generous:
/// two minimum-sized extension headers that may or may not be there.
const IPV4_HEADER_LEN: usize = 20 + 8;
const IPV6_HEADER_LEN: usize = 40 + 16 + 8;

/// The size that works everywhere, and where a send that is refused for
/// being too large falls back to.
const FALLBACK_MTU: usize = 500;

/// `EMSGSIZE`, or `WSAEMSGSIZE` on Windows: this datagram is larger
/// than the path will carry.
///
/// The number differs by platform and is stable ABI on each, so it is
/// written down rather than pulled in with a C binding for one integer.
const EMSGSIZE: i32 = if cfg!(windows) {
    10040
} else if cfg!(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
)) {
    40
} else {
    90
};

/// More than this in one read is a paste, not typing.
const BULK_INPUT_BYTES: usize = 100;

/// How long a `pump` may wait on the newest socket before returning to
/// let the timers run.
const POLL_TIMEOUT_MS: u64 = 50;

/// What a wait primitive on this platform takes for a socket.
///
/// The one platform-dependent thing the library exposes, and it exists
/// so a front end can wait on the session's sockets and its own
/// terminal in the SAME call. See [`MoshSession::socket_handles`].
#[cfg(unix)]
pub type SocketHandle = std::os::fd::RawFd;
/// The same, on Windows.
#[cfg(windows)]
pub type SocketHandle = std::os::windows::io::RawSocket;

/// How many datagrams one [`MoshSession::pump`] takes before it gives
/// the rest of the client a turn.
///
/// Generous against anything legitimate: the peer sends at most one
/// frame per send interval, floored at 20 ms, and a whole repainted
/// screen is a handful of fragments even at the smallest MTU. The bound
/// exists for the case that is not legitimate, a peer retransmitting
/// because nothing has acknowledged it, where an unbounded drain never
/// ends.
const MAX_DATAGRAMS_PER_PUMP: usize = 64;

/// How often the client rotates its own source port. This is the whole
/// of client-side roaming: see [`MoshSession::hop_port`].
const PORT_HOP_INTERVAL_MS: u64 = 10_000;
/// How many old sockets to keep reading from at once.
const MAX_PORTS_OPEN: usize = 10;
/// A newest socket that has worked this long makes the old ones
/// pointless: nothing in flight can still be coming back to them.
const MAX_OLD_SOCKET_AGE_MS: u64 = 60_000;

/// How the link is doing, in the terms a user cares about.
///
/// The two are separate because they fail separately: a link that only
/// works in one direction shows up in `since_ack_ms` while
/// `since_heard_ms` stays small, and saying "no reply" rather than "no
/// contact" is the difference between a useful message and a confusing
/// one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkHealth {
    /// Milliseconds since anything at all arrived from the server.
    pub since_heard_ms: u64,
    /// Milliseconds since the server acknowledged something we sent.
    pub since_ack_ms: u64,
}

/// A live mosh session against one server.
///
/// Generic over the screen so an application can bring the terminal it
/// already has instead of carrying a second one. The standalone client
/// uses [`Vt100Screen`](crate::screen::Vt100Screen).
pub struct MoshSession<S: Screen> {
    /// Our local sockets, oldest first. Packets go out the NEWEST; all
    /// of them are read, because a reply to something sent from an
    /// older port comes back to that port.
    sockets: Vec<UdpSocket>,
    /// When the newest socket was opened, which is what paces the next
    /// rotation and what ages the old ones out.
    last_port_choice: u64,
    server: SocketAddr,
    crypto: CryptoSession,
    packets: PacketState,
    sender: TransportSender,
    receiver: TransportReceiver,
    fragmenter: Fragmenter,
    assembly: FragmentAssembly,
    started: Instant,
    /// The client's own copy of the screen. Diffs land on the state
    /// they name, and only the difference against what is displayed
    /// reaches the terminal.
    terminal: ClientTerminal<S>,
    /// What the client GUESSES the keystrokes it just sent will do, so
    /// the user sees them before the server has answered.
    prediction: PredictionEngine,
    /// The datagram size being built to. Starts from the link MTU and
    /// drops to [`FALLBACK_MTU`] if the path refuses that size.
    mtu: usize,
    /// When we sent the newest state the server has acknowledged.
    /// How long ago that was is how long it has been since a round trip
    /// actually completed, and that is what decides whether the client
    /// starts looking for a new source port.
    last_roundtrip_success: u64,
    /// Set once the peer's shutdown state has been seen.
    peer_shut_down: bool,
    /// Query IDs already handed to the outer terminal. Host states are
    /// cumulative, so the same query can arrive in several accepted states.
    seen_terminal_queries: HashSet<u64>,
}

impl<S: Screen> MoshSession<S> {
    /// Connect using a screen the caller supplies, already the shape
    /// the session should start at.
    pub fn connect_with_screen(host: &str, port: u16, key: &Base64Key, blank: S) -> Result<Self> {
        let server = (host, port)
            .to_socket_addrs()
            .map_err(|_| MoshError::BadAddress)?
            .next()
            .ok_or(MoshError::BadAddress)?;
        Ok(Self {
            sockets: vec![bind_socket(server)?],
            last_port_choice: 0,
            mtu: DEFAULT_LINK_MTU
                - if server.is_ipv6() {
                    IPV6_HEADER_LEN
                } else {
                    IPV4_HEADER_LEN
                },
            server,
            crypto: CryptoSession::new(key),
            packets: PacketState::default(),
            sender: TransportSender::new(),
            receiver: TransportReceiver::new(),
            fragmenter: Fragmenter::default(),
            assembly: FragmentAssembly::default(),
            started: Instant::now(),
            terminal: ClientTerminal::new(blank),
            prediction: PredictionEngine::new(),
            last_roundtrip_success: 0,
            peer_shut_down: false,
            seen_terminal_queries: HashSet::new(),
        })
    }

    /// Milliseconds since the session began. mosh's timestamps are
    /// monotonic and only ever compared to each other, so any epoch
    /// works as long as it never goes backwards.
    fn now(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// Queue user input. It goes out on the next [`Self::pump`], after
    /// the collect window, so a burst of typing costs one packet.
    pub fn send_input(&mut self, bytes: &[u8]) {
        let now = self.now();
        // Predict BEFORE the bytes are folded into a state. A
        // prediction expires at `local_frame_sent + 1`, which is the
        // number the state carrying these very bytes will be given, so
        // the order here is what makes a prediction judgeable at all.
        self.prediction
            .set_local_frame_sent(self.sender.sent_state_last());
        // A paste is not typing, and guessing at it is worse than
        // useless: a hundred predictions land at once, most of them
        // wrong, and the repair costs more than the echo would have.
        // mosh calls more than this in ONE read bulk, throws away the
        // guesses already in flight and predicts none of it. The size
        // of a single read is the whole signal, which is why the rule
        // lives here rather than inside the engine.
        if bytes.len() > BULK_INPUT_BYTES {
            self.prediction.reset();
        } else if let Some(screen) = self.terminal.confirmed() {
            self.prediction.new_user_bytes(bytes, screen, now);
        }
        self.sender.state_mut().push_bytes(bytes);
    }

    /// Queue a response returned by the local terminal for a forwarded query.
    /// Responses bypass prediction and are still part of the cumulative
    /// UserStream, so a retransmitted state cannot write one twice remotely.
    pub fn send_terminal_response(&mut self, bytes: &[u8]) {
        self.sender.state_mut().push_terminal_response(bytes);
    }

    /// What to prepend to a window title before passing it on. Empty
    /// by default: the mechanism is here, the text is the caller's.
    pub fn set_title_prefix(&mut self, prefix: impl Into<String>) {
        self.terminal.set_title_prefix(prefix);
    }

    /// The window title the host has asked for, unprefixed.
    pub fn title(&self) -> Option<String> {
        self.terminal.title()
    }

    /// The prediction engine, to set a display preference on.
    pub fn prediction_mut(&mut self) -> &mut PredictionEngine {
        &mut self.prediction
    }

    /// How long it has been since the server was heard from, and since
    /// it last acknowledged us.
    pub fn link_health(&self) -> LinkHealth {
        let now = self.now();
        LinkHealth {
            since_heard_ms: now.saturating_sub(self.sender.last_heard()),
            since_ack_ms: now.saturating_sub(self.sender.sent_state_acked_timestamp()),
        }
    }

    /// The prediction engine, to ask what it is currently doing.
    pub fn prediction(&self) -> &PredictionEngine {
        &self.prediction
    }

    /// How long the caller may wait before the display needs another
    /// look even with nothing arriving from the network, because a
    /// prediction that STALLS is itself a signal.
    pub fn prediction_wait_ms(&self) -> Option<u64> {
        self.prediction.wait_time_ms()
    }

    /// Queue a terminal resize.
    pub fn send_resize(&mut self, width: i32, height: i32) {
        self.sender.state_mut().push_resize(width, height);
    }

    /// Hold the session to a packet every `interval_ms`, or `None` for
    /// mosh's own three-second heartbeat.
    ///
    /// See [`TransportSender::set_keep_alive`]. Nothing else about the
    /// session changes: the keep-alive rides the ordinary send path, and
    /// the acknowledgement it draws is the peer's ordinary `ack_num`,
    /// which [`Self::link_health`] already reports.
    pub fn set_keep_alive(&mut self, interval_ms: Option<u64>) {
        self.sender.set_keep_alive(interval_ms);
    }

    /// Begin the shutdown handshake.
    pub fn shutdown(&mut self) {
        let now = self.now();
        self.sender.start_shutdown(now);
    }

    /// True once the peer has acknowledged our shutdown, announced its
    /// own, or stopped answering long enough that it never will.
    pub fn finished(&self) -> bool {
        self.peer_shut_down
            || self.sender.shutdown_acknowledged()
            || self.sender.shutdown_timed_out(self.now())
    }

    /// Judge the outstanding guesses and collect the ones worth
    /// showing.
    ///
    /// Culling reads the CONFIRMED screen, never one already carrying
    /// an overlay: that would let every prediction validate itself and
    /// the display would drift with nothing reporting an error.
    fn overlay(&mut self) -> (Vec<OverlayCell>, Option<(u16, u16)>) {
        let now = self.now();
        match self.terminal.confirmed() {
            Some(screen) => {
                self.prediction.cull(screen, now);
                self.prediction.overlay(screen)
            }
            None => (Vec::new(), None),
        }
    }

    /// Bring the client's displayed screen up to date, predictions and
    /// all, without producing any bytes.
    ///
    /// This is the path for an application that owns the grid it draws
    /// from: call this, then draw [`Self::displayed`]. No diff is
    /// computed and nothing round-trips through escape sequences.
    pub fn advance(&mut self) {
        self.advance_with(&[]);
    }

    /// The same, with the caller's own cells on top.
    pub fn advance_with(&mut self, extra: &[OverlayCell]) {
        let (overlay, cursor) = self.compose(extra);
        self.terminal.advance(&overlay, cursor);
    }

    /// Put the caller's cells over the predictions, and work out what
    /// should become of the cursor.
    ///
    /// The rule is mosh's, generalized: if something painted covers the
    /// cell the cursor would sit in, the cursor is hidden. mosh checks
    /// the ROW, because its status bar spans one; checking the cell
    /// itself gives the same answer for a full-width bar and a better
    /// one for anything narrower. The caller never has to say.
    fn compose(&mut self, extra: &[OverlayCell]) -> (Vec<OverlayCell>, OverlayCursor) {
        let (mut overlay, predicted) = self.overlay();
        overlay.extend_from_slice(extra);

        // Where the cursor will end up: what the prediction asks for,
        // or where the confirmed screen already has it.
        let effective = predicted.or_else(|| self.terminal.confirmed().map(Screen::cursor));
        let cursor = match effective {
            Some((row, col)) if extra.iter().any(|c| c.row == row && c.col == col) => {
                OverlayCursor::Hidden
            }
            Some((row, col)) if predicted.is_some() => OverlayCursor::At(row, col),
            _ => OverlayCursor::Unchanged,
        };
        (overlay, cursor)
    }

    /// What the display should be showing right now, predictions
    /// included.
    pub fn displayed(&self) -> &S {
        self.terminal.displayed()
    }

    /// The text of the screen as the SERVER has confirmed it. What the
    /// terminal shows may legitimately be ahead of this by whatever is
    /// currently predicted; [`Self::displayed_text`] is that.
    pub fn screen_text(&self) -> String {
        self.terminal.screen_text()
    }

    /// The text a correctly painted terminal must be showing right now,
    /// predictions included.
    pub fn displayed_text(&self) -> String {
        self.terminal.displayed_text()
    }

    /// The source port packets are leaving from right now. It CHANGES
    /// over the life of a session, and that is the point.
    pub fn local_port(&self) -> Option<u16> {
        self.sockets.last()?.local_addr().ok().map(|a| a.port())
    }

    /// How many sockets are currently being read from.
    pub fn open_sockets(&self) -> usize {
        self.sockets.len()
    }

    // ------------------------------------------------------------ //
    // roaming
    // ------------------------------------------------------------ //

    /// Rotate the local source port if it is time.
    ///
    /// This is the whole of client-side roaming, and it is worth being
    /// precise about why it works. A mosh client NEVER re-targets: it
    /// keeps talking to the address it was given. The SERVER re-targets,
    /// onto the source of any datagram that passes its authentication.
    /// So when a laptop changes networks, what tells the server where to
    /// answer is simply the next packet arriving from somewhere new, and
    /// a fresh source port is what forces a NAT to mint a fresh mapping
    /// for it. Nothing is negotiated and nothing is announced.
    ///
    /// Start looking for a new source port right now, without waiting
    /// for the link to be judged failed.
    ///
    /// The ordinary rotation only fires after ten seconds with no
    /// completed round trip, which is the right default for a client
    /// that cannot know WHY answers stopped. An application that does
    /// know, because the operating system just told it the network
    /// changed, can say so and save the session those ten seconds.
    /// mosh has no equivalent, having no application to tell it.
    pub fn roam_now(&mut self) {
        let now = self.now();
        self.hop_port(now);
    }

    /// Rotation is a RECOVERY move, not a habit. Both clocks have to
    /// have run out: ten seconds since the last rotation, AND ten
    /// seconds since a round trip last completed. A healthy session
    /// keeps one source port for its whole life; one that has stopped
    /// getting answers starts trying new ones, which is exactly the
    /// state a laptop is in a moment after it changes networks.
    fn maybe_hop_port(&mut self, now: u64) {
        if now.saturating_sub(self.last_port_choice) <= PORT_HOP_INTERVAL_MS {
            return;
        }
        if now.saturating_sub(self.last_roundtrip_success) <= PORT_HOP_INTERVAL_MS {
            return;
        }
        self.hop_port(now);
    }

    /// Open a new source port and start sending from it, keeping the
    /// old ones open to receive whatever is still in flight to them.
    ///
    /// A bind that fails leaves the session exactly as it was, still
    /// sending from the port it had: losing the ability to rotate is
    /// worth strictly less than the session.
    fn hop_port(&mut self, now: u64) {
        let Ok(socket) = bind_socket(self.server) else {
            return;
        };
        if let Some(previous) = self.sockets.last() {
            // Only the newest socket ever waits; the ones behind it are
            // drained instantly at the top of a pump.
            let _ = previous.set_nonblocking(true);
        }
        self.last_port_choice = now;
        self.sockets.push(socket);
        self.prune_sockets(now);
    }

    /// Close sockets that can no longer be useful.
    fn prune_sockets(&mut self, now: u64) {
        if self.sockets.len() <= 1 {
            return;
        }
        // The newest one has been working long enough that nothing can
        // still be arriving at the others.
        if now.saturating_sub(self.last_port_choice) > MAX_OLD_SOCKET_AGE_MS {
            let drop_to = self.sockets.len() - 1;
            self.sockets.drain(..drop_to);
            return;
        }
        if self.sockets.len() > MAX_PORTS_OPEN {
            let drop_to = self.sockets.len() - MAX_PORTS_OPEN;
            self.sockets.drain(..drop_to);
        }
    }

    /// Drive the session once: read whatever has arrived, then send
    /// whatever is due. Returns the host events that arrived, which
    /// the caller may inspect; the screen itself is kept internally
    /// and read with [`Self::render`].
    ///
    /// Call it often; it never blocks longer than the socket's read
    /// timeout, and the timers decide what actually goes out.
    pub fn pump(&mut self) -> Result<Vec<HostEvent>> {
        let wait = self.wait_time_ms().clamp(1, POLL_TIMEOUT_MS);
        self.cycle(Some(Duration::from_millis(wait)))
    }

    /// One cycle with no waiting at all: take what has already arrived,
    /// run the timers, send what is due.
    ///
    /// For a caller that does its own waiting. [`Self::pump`] can only
    /// wait on this session's sockets, so a front end using it learns
    /// about a keystroke no sooner than the next time that wait expires,
    /// and a fixed poll interval is what makes an otherwise fast link
    /// feel slow. mosh waits on its socket and its terminal TOGETHER;
    /// [`Self::socket_handles`] and [`Self::wait_time_ms`] are what let a
    /// caller do the same, and this is the cycle to run afterwards.
    pub fn pump_ready(&mut self) -> Result<Vec<HostEvent>> {
        self.cycle(None)
    }

    /// The sockets this session reads, so a caller can wait on them
    /// alongside whatever else it has to watch.
    ///
    /// Every one of them, not just the newest: a reply to a datagram
    /// sent from an older port comes back to THAT port, which is what
    /// keeps a source-port rotation from costing a round trip.
    ///
    /// Nothing here requires a caller to use this. [`Self::pump`] waits
    /// by itself, and an application driving the session from its own
    /// event loop never needs to look at a socket.
    #[cfg(any(unix, windows))]
    pub fn socket_handles(&self) -> Vec<SocketHandle> {
        #[cfg(unix)]
        use std::os::fd::AsRawFd as _;
        #[cfg(windows)]
        use std::os::windows::io::AsRawSocket as _;
        #[cfg(unix)]
        return self.sockets.iter().map(|s| s.as_raw_fd()).collect();
        #[cfg(windows)]
        return self.sockets.iter().map(|s| s.as_raw_socket()).collect();
    }

    /// How long there is before this session needs to send something.
    ///
    /// mosh's `wait_time`, and the timeout to give a wait that also
    /// watches a terminal. Recalculates before answering, so input
    /// queued a moment ago is accounted for.
    pub fn wait_time_ms(&mut self) -> u64 {
        let now = self.now();
        let (srtt, rto) = (self.packets.rtt.srtt(), self.packets.rtt.rto());
        self.sender.wait_time(now, srtt, rto)
    }

    fn cycle(&mut self, wait: Option<Duration>) -> Result<Vec<HostEvent>> {
        let mut events = Vec::new();
        let mut buf = [0u8; crate::crypto::RECEIVE_MTU];

        // Only the newest socket ever waits; the older ones are already
        // non-blocking, so what they hold is taken instantly.
        if let Some(newest) = self.sockets.last() {
            match wait {
                Some(d) => {
                    let _ = newest.set_nonblocking(false);
                    let _ = newest.set_read_timeout(Some(d));
                }
                None => {
                    let _ = newest.set_nonblocking(true);
                }
            }
        }

        // Drain every socket, oldest first: several datagrams can land
        // between two calls, and dropping them would look like packet
        // loss the protocol then has to repair. Only the LAST socket
        // waits (it carries the read timeout); the older ones are
        // non-blocking, so what they hold is taken instantly and the
        // one pause of a pump still happens once, at the end.
        //
        // BOUNDED, and that bound is load-bearing rather than
        // defensive. Draining until the socket runs dry means draining
        // until the PEER pauses, and an unacknowledged peer does not
        // pause: it retransmits at frame rate. Since a frame then
        // arrives well inside the read timeout, the read never times
        // out, the drain never ends and this call never returns.
        // Nothing below gets to acknowledge anything, which is what was
        // keeping the peer retransmitting, so it holds. The caller
        // starves with it: no keystroke is read and none is sent, and
        // the session goes on looking alive.
        //
        // Stopping early drops nothing. What is left stays in the
        // socket and is taken by the next pump, one frame later.
        let mut received = false;
        let mut idx = 0;
        let mut taken = 0usize;
        while idx < self.sockets.len() && taken < MAX_DATAGRAMS_PER_PUMP {
            while let Ok((n, from)) = self.sockets[idx].recv_from(&mut buf) {
                taken += 1;
                // A client never re-targets (only servers roam), so a
                // datagram from anywhere else is not ours; a forged or
                // corrupted one is dropped just as quietly, since
                // anyone at all can send us bytes.
                if from == self.server {
                    received = true;
                    if let Ok(mut got) = self.handle_datagram(&buf[..n]) {
                        events.append(&mut got);
                    }
                }
                if taken >= MAX_DATAGRAMS_PER_PUMP {
                    break;
                }
            }
            idx += 1;
        }

        let now = self.now();
        if received {
            // mosh prunes on every successful receive, and that call
            // site is the only thing that makes the age rule reachable:
            // a hop refreshes `last_port_choice` on its way in, so
            // pruning from there can never find an old socket.
            self.prune_sockets(now);
        }
        let (srtt, rto) = (self.packets.rtt.srtt(), self.packets.rtt.rto());
        // The frame interval doubles as the prediction engine's read on
        // how slow the link is, which is what decides whether guesses
        // are worth showing at all.
        self.prediction
            .set_send_interval(TransportSender::interval(srtt));
        if let Some(inst) = self.sender.tick(now, srtt, rto) {
            let budget = self.mtu - CONNECTION_ADDED_BYTES - CRYPTO_ADDED_BYTES;
            let mut too_large = false;
            for frag in self.fragmenter.fragment(&inst, budget)? {
                let out = self.packets.new_packet(now, frag.to_bytes());
                let datagram = self.crypto.encrypt(
                    out.seq,
                    Direction::ToServer,
                    &out.packet.to_plaintext(),
                )?;
                let socket = self.sockets.last().expect("a session always has a socket");
                if let Err(error) = socket.send_to(&datagram, self.server)
                    && error.raw_os_error() == Some(EMSGSIZE)
                {
                    too_large = true;
                }
            }
            if too_large {
                // The path will not carry a datagram this size. Fall
                // back to the one that works everywhere and stay there,
                // as mosh does. The datagrams just refused are lost,
                // and that costs nothing: this protocol resends STATE,
                // not packets, so the next tick rebuilds them smaller.
                self.mtu = FALLBACK_MTU;
            }
            // The rotation decision lives with the send, as it does in
            // mosh's `Connection::send`.
            self.maybe_hop_port(now);
        }
        self.prediction
            .set_local_frame_sent(self.sender.sent_state_last());
        Ok(events)
    }

    fn handle_datagram(&mut self, datagram: &[u8]) -> Result<Vec<HostEvent>> {
        let incoming = self.crypto.decrypt(datagram)?;
        // Anti-reflection: a packet marked as ours cannot have come
        // from the server.
        if incoming.direction != Direction::ToClient {
            return Err(MoshError::WrongDirection);
        }
        let packet = Packet::from_plaintext(&incoming.plaintext)?;
        let now = self.now();
        self.packets.accept(now, incoming.seq, &packet, false);
        if packet.payload.is_empty() {
            return Ok(Vec::new());
        }

        let frag = Fragment::from_bytes(&packet.payload)?;
        if !self.assembly.add(frag) {
            return Ok(Vec::new());
        }
        let Some(inst) = self.assembly.take()? else {
            return Ok(Vec::new());
        };
        inst.check_version()?;

        if let Some(ack) = inst.ack_num {
            self.sender.process_acknowledgement(ack);
            self.prediction.set_local_frame_acked(ack);
        }
        if inst.new_num == Some(crate::transport::SHUTDOWN_NUM) {
            self.peer_shut_down = true;
        }
        // A completed round trip: the peer is answering what we sent.
        // How long ago the newest ACKNOWLEDGED state went out is what
        // tells the roaming logic whether the link is working.
        self.last_roundtrip_success = self.sender.sent_state_acked_timestamp();
        let (num, diff, in_order) = match self.receiver.process(&inst, now) {
            Received::Apply { num, diff } => (num, diff, true),
            Received::ApplyOutOfOrder { num, diff } => (num, diff, false),
            // A duplicate is still evidence the peer is alive, and it
            // must be acknowledged again or the peer keeps resending.
            Received::Duplicate => {
                self.sender.set_ack_num(self.receiver.latest(), false, now);
                return Ok(Vec::new());
            }
            Received::Unresolvable => return Ok(Vec::new()),
        };
        {
            let has_data = !diff.is_empty();
            // Acknowledge the HIGHEST state held, never this one:
            // an out-of-order arrival is older than something we
            // already have, and naming it would ask the server to
            // resend what is already on screen.
            if in_order {
                self.sender
                    .set_ack_num(self.receiver.latest(), has_data, now);
            }
            let mut events = if has_data {
                parse_host_diff(&diff)?
            } else {
                Vec::new()
            };
            self.deduplicate_terminal_queries(&mut events);

            // EVERY state gets a screen, including one whose diff
            // changed nothing on it: the server is free to compute
            // a later diff from that state, and a client that only
            // remembered the states that painted something would
            // have to drop it. That is the bug this comment exists
            // to prevent a second time.
            let mut painted: &[u8] = &[];
            for event in &events {
                match event {
                    // ROWS then COLS, which is height then width.
                    // The wire carries the pair the other way round,
                    // and passing it through in wire order gives a
                    // screen with its dimensions transposed: every
                    // later diff then lands on a shape the server
                    // never painted, so the terminal fills with
                    // fragments of the right characters in the wrong
                    // places and stops echoing what is typed.
                    HostEvent::Resize { width, height } => {
                        self.terminal.resize(*height as u16, *width as u16);
                    }
                    HostEvent::Bytes(bytes) => painted = bytes,
                    // The one clock a prediction can be judged
                    // against: the newest state of ours the server
                    // has run through its own terminal. Ordinary
                    // acknowledgement only proves it ARRIVED.
                    HostEvent::EchoAck(num) => {
                        self.prediction.set_local_frame_late_acked(*num);
                    }
                    HostEvent::TerminalQuery { .. } => {}
                }
            }
            // The diff belongs to the state it was computed FROM,
            // not to whatever is on screen now.
            let old_num = inst.old_num.unwrap_or(0);
            self.terminal.apply_diff(old_num, num, painted);
            if let Some(throwaway) = inst.throwaway_num {
                self.terminal.forget_before(throwaway);
            }
            Ok(events)
        }
    }

    fn deduplicate_terminal_queries(&mut self, events: &mut Vec<HostEvent>) {
        events.retain(|event| match event {
            HostEvent::TerminalQuery { id, .. } => self.seen_terminal_queries.insert(*id),
            _ => true,
        });
    }
}

impl<S: DiffScreen> MoshSession<S> {
    /// The bytes to write to a real terminal: the difference between
    /// what it shows and the newest state the server has sent, with
    /// whatever predictions are worth showing painted on top. Empty
    /// when nothing changed, which is what makes a retransmitted frame
    /// cost nothing on screen.
    pub fn render(&mut self) -> Vec<u8> {
        self.render_with(&[])
    }

    /// The same, with the caller's own cells painted on top of the
    /// predictions.
    ///
    /// This is how a front end draws something of its own over the
    /// session, the way mosh paints its status bar: the extra cells go
    /// on LAST, so they win wherever they overlap a prediction.
    pub fn render_with(&mut self, extra: &[OverlayCell]) -> Vec<u8> {
        let (overlay, cursor) = self.compose(extra);
        self.terminal.render(&overlay, cursor)
    }

    /// Repaint the whole screen, for a terminal whose state cannot be
    /// known (first paint, or coming back from a suspend).
    pub fn repaint(&mut self) -> Vec<u8> {
        self.terminal.repaint()
    }
}

#[cfg(feature = "vt100-screen")]
impl MoshSession<crate::screen::Vt100Screen> {
    /// Connect to a server that has already printed `MOSH CONNECT`.
    /// No handshake travels here: the SSH bootstrap already agreed the
    /// key, and the first datagram we send is a normal session packet.
    pub fn connect(host: &str, port: u16, key: &Base64Key) -> Result<Self> {
        Self::connect_with_size(host, port, key, 80, 24)
    }

    /// Connect with the terminal size known up front, so the first
    /// frames land on a screen of the right shape.
    pub fn connect_with_size(
        host: &str,
        port: u16,
        key: &Base64Key,
        cols: u16,
        rows: u16,
    ) -> Result<Self> {
        Self::connect_with_screen(host, port, key, crate::screen::Vt100Screen::new(rows, cols))
    }
}

/// A fresh local socket for talking to `server`.
///
/// The family follows the server's, the port is left to the OS (a mosh
/// client never binds a fixed one), and the read timeout is what paces
/// [`MoshSession::pump`]. Only the newest socket keeps that timeout;
/// [`MoshSession::hop_port`] switches the one it replaces to
/// non-blocking.
fn bind_socket(server: SocketAddr) -> Result<UdpSocket> {
    let bind = if server.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    };
    let socket = UdpSocket::bind(bind).map_err(|_| MoshError::BadAddress)?;
    socket
        .set_read_timeout(Some(Duration::from_millis(POLL_TIMEOUT_MS)))
        .map_err(|_| MoshError::BadAddress)?;
    Ok(socket)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session pointed at a port nothing is listening on. Everything
    /// below is about the client's own sockets, which exist and behave
    /// whether or not anything answers.
    fn offline_session() -> MoshSession<crate::screen::Vt100Screen> {
        let key = Base64Key::from_printable("AAAAAAAAAAAAAAAAAAAAAA").expect("valid key");
        MoshSession::connect("127.0.0.1", 1, &key).expect("bind a local socket")
    }

    #[test]
    fn a_paste_is_not_guessed_at() {
        let mut session = offline_session();
        session
            .prediction_mut()
            .set_display_preference(crate::prediction::DisplayPreference::Always);
        session.send_input(b"ls");
        assert!(session.prediction().active(), "typing is guessed at");
        // A hundred and one bytes in one read is somebody pasting.
        session.send_input(&[b'x'; BULK_INPUT_BYTES + 1]);
        assert!(
            !session.prediction().active(),
            "a paste should throw the guesses away, not add a hundred more"
        );
    }

    fn painted(row: u16, col: u16) -> OverlayCell {
        OverlayCell {
            row,
            col,
            cell: crate::screen::Cell {
                contents: "x".into(),
                rendition: Default::default(),
            },
            underline: false,
        }
    }

    #[test]
    fn something_painted_over_the_cursor_hides_it() {
        // A cursor blinking inside a status message reads as a glitch,
        // and the caller never has to say so: covering the cell it
        // would sit in IS the instruction.
        let mut session = offline_session();
        assert_eq!(
            session.terminal.confirmed().map(Screen::cursor),
            Some((0, 0))
        );
        let (_, cursor) = session.compose(&[painted(0, 0)]);
        assert_eq!(cursor, OverlayCursor::Hidden);
    }

    #[test]
    fn something_painted_elsewhere_leaves_the_cursor_alone() {
        // The inverse is where a careless rule would bite: hiding on
        // the ROW would blank the cursor for any overlay anywhere on
        // that line, including one cell of it.
        let mut session = offline_session();
        let (_, cursor) = session.compose(&[painted(0, 40)]);
        assert_eq!(cursor, OverlayCursor::Unchanged);
        let (_, cursor) = session.compose(&[painted(5, 0)]);
        assert_eq!(cursor, OverlayCursor::Unchanged);
    }

    #[test]
    fn an_empty_overlay_changes_nothing() {
        let mut session = offline_session();
        let (cells, cursor) = session.compose(&[]);
        assert!(cells.is_empty());
        assert_eq!(cursor, OverlayCursor::Unchanged);
    }

    #[test]
    fn retransmitted_terminal_query_ids_are_suppressed_without_assuming_order() {
        let mut session = offline_session();
        let mut events = vec![
            HostEvent::TerminalQuery {
                id: 2,
                bytes: b"two".to_vec(),
            },
            HostEvent::TerminalQuery {
                id: 1,
                bytes: b"one".to_vec(),
            },
            HostEvent::TerminalQuery {
                id: 2,
                bytes: b"two".to_vec(),
            },
        ];
        session.deduplicate_terminal_queries(&mut events);
        assert_eq!(events.len(), 2);

        let mut retransmission = vec![HostEvent::TerminalQuery {
            id: 1,
            bytes: b"one".to_vec(),
        }];
        session.deduplicate_terminal_queries(&mut retransmission);
        assert!(retransmission.is_empty());
    }

    #[test]
    fn the_datagram_size_follows_the_address_family() {
        let key = Base64Key::from_printable("AAAAAAAAAAAAAAAAAAAAAA").expect("valid key");
        let v4 = MoshSession::connect("127.0.0.1", 1, &key).expect("bind");
        // 1280 minus IP and UDP headers: mosh's own 1252.
        assert_eq!(v4.mtu, 1252);

        let v6 = MoshSession::connect("::1", 1, &key).expect("bind");
        // Less, because the IPv6 allowance covers extension headers
        // that may or may not be on the path.
        assert_eq!(v6.mtu, 1216);
        assert!(v6.mtu < v4.mtu);
    }

    #[test]
    fn the_fallback_is_a_size_that_works_everywhere() {
        // What a path that refuses the larger datagram drops us to, and
        // what the client used unconditionally before it learned to
        // ask for more.
        assert_eq!(FALLBACK_MTU, 500);
        let budget = FALLBACK_MTU - CONNECTION_ADDED_BYTES - CRYPTO_ADDED_BYTES;
        assert_eq!(budget, 472);
    }

    #[test]
    fn the_larger_datagram_carries_far_more_per_fragment() {
        // The point of asking for it. A screenful of output is one
        // instruction, and every fragment it needs is another packet
        // whose loss costs the whole thing.
        let v4_budget = 1252 - CONNECTION_ADDED_BYTES - CRYPTO_ADDED_BYTES;
        let fallback_budget = FALLBACK_MTU - CONNECTION_ADDED_BYTES - CRYPTO_ADDED_BYTES;
        assert!(
            v4_budget > fallback_budget * 2,
            "{v4_budget} vs {fallback_budget}: not worth the risk if it were close"
        );
    }

    #[test]
    fn a_session_starts_on_one_port() {
        let session = offline_session();
        assert_eq!(session.open_sockets(), 1);
        assert!(session.local_port().is_some());
    }

    #[test]
    fn nothing_rotates_before_the_interval() {
        let mut session = offline_session();
        let port = session.local_port();
        session.maybe_hop_port(PORT_HOP_INTERVAL_MS);
        assert_eq!(session.open_sockets(), 1);
        assert_eq!(session.local_port(), port);
    }

    #[test]
    fn the_source_port_moves_once_the_interval_passes() {
        let mut session = offline_session();
        let before = session.local_port().expect("a port");
        session.maybe_hop_port(PORT_HOP_INTERVAL_MS + 1);
        let after = session.local_port().expect("a port");
        assert_ne!(
            before, after,
            "the client should be sending from somewhere new"
        );
        // And the old one stays open: a reply to something sent from it
        // comes back to IT, not to the new port.
        assert_eq!(session.open_sockets(), 2);
    }

    #[test]
    fn a_working_link_never_changes_port() {
        let mut session = offline_session();
        let port = session.local_port();
        // A round trip completed a moment ago, so the link is fine and
        // there is nothing to recover from. mosh keeps its port for the
        // whole life of a healthy session; rotating is what it does
        // when answers STOP coming.
        let now = 5 * PORT_HOP_INTERVAL_MS;
        session.last_roundtrip_success = now - 1;
        session.maybe_hop_port(now);
        assert_eq!(session.open_sockets(), 1);
        assert_eq!(session.local_port(), port);

        // Ten seconds of silence later, it starts looking.
        session.maybe_hop_port(now + PORT_HOP_INTERVAL_MS + 2);
        assert_eq!(session.open_sockets(), 2);
    }

    #[test]
    fn rotations_are_paced_from_the_last_one() {
        let mut session = offline_session();
        let mut now = PORT_HOP_INTERVAL_MS + 1;
        session.maybe_hop_port(now);
        assert_eq!(session.open_sockets(), 2);
        // A moment later is not another interval.
        now += 1;
        session.maybe_hop_port(now);
        assert_eq!(session.open_sockets(), 2);
        now += PORT_HOP_INTERVAL_MS;
        session.maybe_hop_port(now);
        assert_eq!(session.open_sockets(), 3);
    }

    #[test]
    fn the_client_never_reads_from_more_than_ten_ports() {
        let mut session = offline_session();
        let mut now = 0;
        for _ in 0..20 {
            now += PORT_HOP_INTERVAL_MS + 1;
            session.maybe_hop_port(now);
        }
        assert_eq!(session.open_sockets(), MAX_PORTS_OPEN);
        // The one being sent from is always the newest, never a
        // survivor of the pruning.
        assert!(session.local_port().is_some());
    }

    #[test]
    fn a_socket_that_has_worked_long_enough_retires_the_old_ones() {
        let mut session = offline_session();
        session.maybe_hop_port(PORT_HOP_INTERVAL_MS + 1);
        session.maybe_hop_port(2 * PORT_HOP_INTERVAL_MS + 2);
        assert_eq!(session.open_sockets(), 3);
        let newest = session.local_port();

        // This is the call site a successful receive makes. Nothing can
        // still be arriving at ports abandoned a minute ago.
        let now = 2 * PORT_HOP_INTERVAL_MS + 2 + MAX_OLD_SOCKET_AGE_MS + 1;
        session.prune_sockets(now);
        assert_eq!(session.open_sockets(), 1);
        assert_eq!(session.local_port(), newest, "the survivor is the newest");
    }

    #[test]
    fn pruning_a_lone_socket_leaves_it_alone() {
        let mut session = offline_session();
        let port = session.local_port();
        session.prune_sockets(MAX_OLD_SOCKET_AGE_MS * 10);
        assert_eq!(session.open_sockets(), 1);
        assert_eq!(session.local_port(), port);
    }
}
