use crate::args::Config;
use crate::lifecycle;
use crate::protocol::ServerTransport;
use crate::terminal_state::{QueryResponder, TerminalState};
use crate::user_stream::{UserEvent, UserStreamTracker};
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use moshcatty::Ocb;
use moshcatty::pb::HostInstruction;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

const INITIAL_ROWS: u16 = 24;
const INITIAL_COLS: u16 = 80;
const ASSOCIATION_TIMEOUT: Duration = Duration::from_secs(60);
const LOOP_SLEEP: Duration = Duration::from_millis(5);
// Match stock Mosh's late acknowledgement grace period. PTY output is not
// proof that the shell has processed a particular input frame.
const ECHO_DELAY: Duration = Duration::from_millis(50);
const MAX_ROWS: u16 = 1024;
const MAX_COLS: u16 = 1024;
const MAX_CELLS: u32 = 262_144;
const UDP_BUFFER: usize = 65_535;
const PTY_CHUNK: usize = 8192;
const PTY_QUEUE_DEPTH: usize = 256;

pub fn run(mut cfg: Config) -> Result<()> {
    let (socket, port) = bind_udp(cfg.bind_ip, cfg.port_low, cfg.port_high)?;
    socket
        .set_nonblocking(true)
        .context("setting UDP socket nonblocking")?;

    let mut key = [0u8; 16];
    getrandom::fill(&mut key).map_err(|error| anyhow!("generating Mosh session key: {error}"))?;
    let key_text = STANDARD_NO_PAD.encode(key);
    let ocb = Ocb::new(&key).context("initializing AES-128-OCB3")?;
    key.fill(0);

    // This is the bootstrap contract parsed by the stock mosh wrapper.
    println!("MOSH CONNECT {port} {key_text}");
    std::io::stdout().flush().context("flushing MOSH CONNECT")?;

    #[cfg(unix)]
    if !cfg.foreground {
        lifecycle::detach_after_connect_line()?;
        // Standard streams are now /dev/null. Avoid attempting diagnostics on
        // them in the detached child.
        cfg.verbose = 0;
    }

    if cfg.verbose > 0 {
        eprintln!(
            "mosh-server-rs: UDP {}, protocol 2, PTY backend native",
            socket.local_addr().context("reading bound UDP address")?
        );
    }

    serve_session(cfg, socket, ServerTransport::new(ocb))
}

fn serve_session(cfg: Config, socket: UdpSocket, mut transport: ServerTransport) -> Result<()> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: INITIAL_ROWS,
            cols: INITIAL_COLS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("opening native PTY/ConPTY")?;

    let mut command = build_command(&cfg);
    configure_child_environment(&mut command, &cfg);

    let mut child = pair
        .slave
        .spawn_command(command)
        .context("spawning shell/command in PTY")?;
    drop(pair.slave);

    let reader = pair
        .master
        .try_clone_reader()
        .context("cloning PTY reader")?;
    let writer = pair.master.take_writer().context("taking PTY writer")?;
    let master = pair.master;

    let (pty_event_tx, pty_event_rx) = mpsc::sync_channel::<PtyEvent>(PTY_QUEUE_DEPTH);
    let (pty_write_tx, pty_write_rx) = mpsc::sync_channel::<PtyWrite>(PTY_QUEUE_DEPTH);
    spawn_pty_reader(reader, pty_event_tx.clone());
    spawn_pty_writer(writer, pty_write_rx, pty_event_tx);

    let mut terminal = TerminalState::new(INITIAL_ROWS, INITIAL_COLS);
    let mut responder = QueryResponder::new();
    let mut user_stream = UserStreamTracker::new();
    let mut udp_buf = vec![0u8; UDP_BUFFER];
    let mut peer: Option<SocketAddr> = None;
    let association_deadline = Instant::now() + ASSOCIATION_TIMEOUT;
    let network_timeout = configured_network_timeout();

    let mut dirty = true;
    let mut echo = EchoAcknowledgements::default();
    let mut enqueued_echo: u64 = 0;
    let mut local_shutdown = false;
    let mut remote_shutdown = false;
    let mut child_exited = false;

    loop {
        let pty = drain_pty_events(
            &pty_event_rx,
            &mut terminal,
            &mut responder,
            &pty_write_tx,
            &mut echo,
            cfg.verbose > 0,
        )?;
        dirty |= pty.dirty;
        child_exited |= pty.ended;

        let confirmed_echo = echo.advance(Instant::now());

        // Drain authenticated UDP datagrams. The peer address is deliberately
        // updated only after Mosh crypto accepted the packet, which implements
        // Mosh's IP/port roaming without allowing unauthenticated rebinding.
        for _ in 0..128 {
            match socket.recv_from(&mut udp_buf) {
                Ok((n, addr)) => {
                    let outcome = transport.receive(&udp_buf[..n])?;
                    if outcome.authenticated {
                        peer = Some(addr);
                    }

                    if let Some(state) = outcome.state {
                        peer = Some(addr);
                        terminal.acknowledge(transport.acked_by_remote());
                        if transport.take_rebase_required() {
                            dirty = true;
                        }

                        if state.new_num == u64::MAX || transport.ack_num() == u64::MAX {
                            remote_shutdown = true;
                        }

                        if !remote_shutdown {
                            let accepted = user_stream.accept(&state)?;
                            apply_user_events(
                                accepted.events,
                                accepted.frame,
                                master.as_ref(),
                                &mut terminal,
                                &pty_write_tx,
                                &mut dirty,
                            )?;
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error).context("receiving UDP datagram"),
            }
        }

        let associated = peer.is_some() && transport.has_received_authenticated();
        if !associated && Instant::now() >= association_deadline {
            let _ = child.kill();
            bail!("no Mosh client associated within 60 seconds");
        }

        if associated {
            if network_timeout.is_some_and(|timeout| transport.last_recv().elapsed() >= timeout) {
                if cfg.verbose > 0 {
                    eprintln!("mosh-server-rs: network timeout expired");
                }
                let _ = child.kill();
                break;
            }

            terminal.acknowledge(transport.acked_by_remote());

            // Build a cumulative HostMessage from the peer's acknowledged
            // visual base. SSP may discard an unsent intermediate state; every
            // newer state is independently valid from the acknowledged base.
            if !local_shutdown && !remote_shutdown && (dirty || confirmed_echo > enqueued_echo) {
                if let Some(payload) = host_update(&terminal, confirmed_echo) {
                    let state_num = transport.set_pending(payload);
                    terminal.snapshot_for_state(state_num);
                    enqueued_echo = enqueued_echo.max(confirmed_echo);
                }
                dirty = false;
            }

            let datagrams = transport.tick();
            if let Some(addr) = peer {
                for datagram in datagrams {
                    match socket.send_to(&datagram, addr) {
                        Ok(_) => {}
                        Err(error)
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                            ) => {}
                        Err(error) => {
                            // UDP reachability can disappear temporarily during
                            // roaming. Keep the session alive and let SSP retry.
                            if cfg.verbose > 1 {
                                eprintln!("mosh-server-rs: UDP send failed: {error}");
                            }
                        }
                    }
                }
            }

            if transport.crypto_exhausted() {
                let _ = child.kill();
                bail!("Mosh OCB per-key block limit exhausted; refusing nonce/key reuse");
            }

            if remote_shutdown && transport.counterparty_shutdown_ack_sent() {
                let _ = child.kill();
                break;
            }

            if local_shutdown
                && (transport.shutdown_acknowledged() || transport.shutdown_timed_out())
            {
                break;
            }
        }

        if !child_exited && child.try_wait().context("polling PTY child")?.is_some() {
            child_exited = true;
        }

        if child_exited && !local_shutdown {
            if associated && !remote_shutdown {
                local_shutdown = true;
                transport.start_shutdown();
            } else {
                break;
            }
        }

        // If the transport tells us queue compaction invalidated an older branch,
        // rebuild from the current ACK base on the next pass.
        if transport.take_rebase_required() {
            dirty = true;
        }

        thread::sleep(LOOP_SLEEP);
    }

    if !child_exited {
        let _ = child.kill();
        let _ = child.wait();
    }
    Ok(())
}

#[derive(Default)]
struct PtyProgress {
    dirty: bool,
    ended: bool,
}

fn drain_pty_events(
    events: &Receiver<PtyEvent>,
    terminal: &mut TerminalState,
    responder: &mut QueryResponder,
    writes: &SyncSender<PtyWrite>,
    echo: &mut EchoAcknowledgements,
    verbose: bool,
) -> Result<PtyProgress> {
    let mut progress = PtyProgress::default();
    // Bound work so a noisy process cannot monopolize the network loop.
    for _ in 0..64 {
        match events.try_recv() {
            Ok(PtyEvent::Output(bytes)) => {
                terminal.process(&bytes);
                progress.dirty = true;
                for reply in responder.feed(&bytes, terminal.cursor_position(), terminal.size()) {
                    queue_pty_write(writes, reply)?;
                }
            }
            Ok(PtyEvent::InputWritten(frame)) => {
                // Start the grace period on observation, allowing queued reader
                // events to drain before the frame becomes eligible for ACK.
                echo.written(frame, Instant::now());
            }
            Ok(PtyEvent::Error(error)) => {
                if verbose {
                    eprintln!("mosh-server-rs: PTY I/O ended: {error}");
                }
                progress.ended = true;
                break;
            }
            Ok(PtyEvent::Eof) | Err(TryRecvError::Disconnected) => {
                progress.ended = true;
                break;
            }
            Err(TryRecvError::Empty) => break,
        }
    }
    Ok(progress)
}

fn host_update(terminal: &TerminalState, confirmed_echo: u64) -> Option<Vec<u8>> {
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
    if let Some((rows, cols)) = terminal.resize_from_ack() {
        instructions.push(HostInstruction {
            hoststring: Vec::new(),
            width: i32::from(cols),
            height: i32::from(rows),
            echo_ack_num: -1,
        });
    }
    let host_bytes = terminal.diff_from_ack();
    if !host_bytes.is_empty() {
        instructions.push(HostInstruction {
            hoststring: host_bytes,
            width: 0,
            height: 0,
            echo_ack_num: -1,
        });
    }
    (!instructions.is_empty()).then(|| HostInstruction::encode_message(&instructions))
}

fn apply_user_events(
    events: Vec<UserEvent>,
    input_frame: u64,
    master: &dyn portable_pty::MasterPty,
    terminal: &mut TerminalState,
    pty_write_tx: &SyncSender<PtyWrite>,
    dirty: &mut bool,
) -> Result<()> {
    let mut keys = Vec::with_capacity(PTY_CHUNK);
    let mut any_keys = false;

    let flush_keys = |keys: &mut Vec<u8>| -> Result<()> {
        if keys.is_empty() {
            return Ok(());
        }
        queue_pty_write(pty_write_tx, std::mem::take(keys))?;
        *keys = Vec::with_capacity(PTY_CHUNK);
        Ok(())
    };

    for event in events {
        match event {
            UserEvent::Byte(byte) => {
                any_keys = true;
                keys.push(byte);
                if keys.len() >= PTY_CHUNK {
                    flush_keys(&mut keys)?;
                }
            }
            UserEvent::Resize { cols, rows } => {
                // Preserve UserStream ordering: bytes before a resize must reach
                // the PTY before the resize, and bytes after it must see the new
                // terminal dimensions.
                flush_keys(&mut keys)?;
                validate_terminal_size(rows, cols)?;
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
        }
    }

    flush_keys(&mut keys)?;
    if any_keys {
        queue_pty_request(pty_write_tx, PtyWrite::InputFrame(input_frame))?;
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

fn queue_pty_write(tx: &SyncSender<PtyWrite>, data: Vec<u8>) -> Result<()> {
    queue_pty_request(tx, PtyWrite::Bytes(data))
}

fn queue_pty_request(tx: &SyncSender<PtyWrite>, request: PtyWrite) -> Result<()> {
    match tx.try_send(request) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => {
            bail!("PTY input queue saturated; child is not consuming terminal input")
        }
        Err(TrySendError::Disconnected(_)) => bail!("PTY writer has stopped"),
    }
}

fn spawn_pty_reader(mut reader: Box<dyn Read + Send>, tx: SyncSender<PtyEvent>) {
    thread::spawn(move || {
        let mut buf = [0u8; PTY_CHUNK];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    let _ = tx.send(PtyEvent::Eof);
                    break;
                }
                Ok(n) => {
                    if tx.send(PtyEvent::Output(buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    let _ = tx.send(PtyEvent::Error(error.to_string()));
                    break;
                }
            }
        }
    });
}

fn spawn_pty_writer(
    mut writer: Box<dyn Write + Send>,
    rx: Receiver<PtyWrite>,
    event_tx: SyncSender<PtyEvent>,
) {
    thread::spawn(move || {
        while let Ok(request) = rx.recv() {
            match request {
                PtyWrite::Bytes(data) => {
                    if let Err(error) = writer.write_all(&data) {
                        let _ = event_tx.send(PtyEvent::Error(error.to_string()));
                        return;
                    }
                }
                PtyWrite::InputFrame(frame) => {
                    if event_tx.send(PtyEvent::InputWritten(frame)).is_err() {
                        return;
                    }
                }
            }
        }
    });
}

fn build_command(cfg: &Config) -> CommandBuilder {
    if cfg.command.is_empty() {
        CommandBuilder::new_default_prog()
    } else {
        CommandBuilder::from_argv(cfg.command.clone())
    }
}

fn configure_child_environment(command: &mut CommandBuilder, cfg: &Config) {
    let term = if cfg.colors >= 256 {
        "xterm-256color"
    } else {
        "xterm"
    };
    command.env("TERM", term);
    if cfg.colors >= 1 << 15 {
        command.env("COLORTERM", "truecolor");
    }
    for (name, value) in &cfg.locale_env {
        command.env(name, value);
    }
}

fn bind_udp(bind_ip: Option<IpAddr>, low: u16, high: u16) -> Result<(UdpSocket, u16)> {
    let ip = bind_ip.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));

    if low == 0 && high == 0 {
        let socket = UdpSocket::bind(SocketAddr::new(ip, 0))
            .with_context(|| format!("binding UDP socket on {ip}:0"))?;
        let port = socket.local_addr()?.port();
        return Ok((socket, port));
    }

    let mut last_error = None;
    for port in low..=high {
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

fn configured_network_timeout() -> Option<Duration> {
    let value = std::env::var("MOSH_SERVER_NETWORK_TMOUT").ok()?;
    let seconds = value.parse::<u64>().ok()?;
    (seconds > 0).then(|| Duration::from_secs(seconds))
}

enum PtyEvent {
    Output(Vec<u8>),
    InputWritten(u64),
    Eof,
    Error(String),
}

enum PtyWrite {
    Bytes(Vec<u8>),
    InputFrame(u64),
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
