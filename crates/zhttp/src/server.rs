//! The read-only static HTTP engine shared by `zhttp server` and the
//! **Start HTTP Server** action.
//!
//! It listens on `0.0.0.0` without authentication by design: its purpose is
//! handing firmware images and other build output to devices on the LAN. What
//! it does guard against is a LAN peer, or a web page a LAN browser visits,
//! using it for more than that:
//!
//! - **Slots.** At most [`MAX_CONCURRENT_REQUESTS`] (32) requests are served at
//!   once, and at most [`MAX_REQUESTS_PER_PEER`] (8) from one peer address, so a
//!   single slow peer cannot occupy every slot and stall a flash in progress.
//!   Eight leaves room for a device plus a browser's six parallel connections.
//! - **Deadlines.** Every read and write waits at most [`CLIENT_TIMEOUT`] (5 s)
//!   for the peer, and that alone bounds nothing: a peer sending one byte every
//!   four seconds never trips it. So the whole request head must also arrive
//!   within [`HEADER_DEADLINE`] (10 s) of the connection being accepted, and a
//!   response must keep pace with a floor rate: the first `n` bytes must be sent
//!   within [`RESPONSE_GRACE`] (30 s) plus `n` at
//!   [`RESPONSE_MIN_BYTES_PER_SECOND`] (16 KiB/s). A floor rather than a fixed
//!   cap is what keeps a legitimate large image working: 16 KiB/s is an order
//!   of magnitude below a bootloader on a 10 Mbit/s link, it gives a 64 MiB image
//!   over an hour, and a device that pauses to erase flash spends the time it
//!   banked by reading faster earlier. A peer reading a byte at a time falls
//!   below the floor within the grace period and loses its slot.
//! - **Dot-prefixed names** (`.git`, `.env`, `.ssh`) are refused unless
//!   `--serve-dotfiles` is given, because a build directory is often also a
//!   checkout. Both the request path and the path it resolves to are checked,
//!   so a symlink such as `cfg -> .config` cannot reach a directory the request
//!   could not name, and directory indexes leave such entries out.
//! - **Host check.** A request whose `Host` names something other than this
//!   machine is answered with 421, which stops a DNS-rebinding page in a LAN
//!   browser from reading the served directory under its own origin. Accepted
//!   are: no `Host` at all (HTTP/1.0 bootloaders), the address the connection
//!   arrived on, any loopback address, `localhost`, this machine's host name
//!   and `<hostname>.local`, and every `--allow-host` name; `--allow-any-host`
//!   turns the check off. The address the connection arrived on is exactly
//!   "this machine's own address" for that request, so nothing needs to
//!   enumerate interfaces.
//!
//! The per-request cost of all four is one mutex-guarded map update at accept,
//! one scan of the head for `Host`, and a socket-timeout update only when a
//! deadline is closer than [`CLIENT_TIMEOUT`].

use std::{
    collections::HashMap,
    fs::{self, File},
    io::{self, Read, Write},
    net::{IpAddr, SocketAddr, TcpListener, TcpStream},
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result};
use time::{OffsetDateTime, macros::format_description};

const SHUTDOWN_WAKE_TIMEOUT: Duration = Duration::from_millis(250);
/// Longest any single read or write waits for the peer.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(5);
/// Longest the whole request head may take to arrive.
const HEADER_DEADLINE: Duration = Duration::from_secs(10);
/// Time a response may take beyond what [`RESPONSE_MIN_BYTES_PER_SECOND`]
/// allows for its length.
const RESPONSE_GRACE: Duration = Duration::from_secs(30);
/// Slowest sustained rate a response may be read at once the grace is spent.
const RESPONSE_MIN_BYTES_PER_SECOND: u64 = 16 * 1024;
const LOG_POLL_INTERVAL: Duration = Duration::from_millis(100);
const MAX_REQUEST_SIZE: usize = 64 * 1024;
const MAX_CONCURRENT_REQUESTS: usize = 32;
const MAX_REQUESTS_PER_PEER: usize = 8;
const COPY_BUFFER_SIZE: usize = 64 * 1024;

/// The slow-client limits above, as `zetta http server --help` states them, so
/// the help cannot drift from the numbers the server enforces.
pub fn http_server_limits_help() -> String {
    format!(
        "Requests\nmust arrive within {} seconds, and responses must be read at {} KiB/s on\naverage after a {}-second grace period; one client address may hold at most\n{} of the {} concurrent requests.",
        HEADER_DEADLINE.as_secs(),
        RESPONSE_MIN_BYTES_PER_SECOND / 1024,
        RESPONSE_GRACE.as_secs(),
        MAX_REQUESTS_PER_PEER,
        MAX_CONCURRENT_REQUESTS,
    )
}

pub struct OpenHttpServer {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub address: SocketAddr,
    pub root: PathBuf,
}

/// What the person starting the server chose; everything defaults to the
/// restrictive side.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HttpServerOptions {
    /// Serve names with a dot-prefixed component (`--serve-dotfiles`).
    pub serve_dotfiles: bool,
    /// Extra `Host` names to answer, already normalised by
    /// [`normalize_allowed_host`] (`--allow-host`).
    pub allowed_hosts: Vec<String>,
    /// Answer every `Host` name (`--allow-any-host`).
    pub allow_any_host: bool,
}

/// The timing and slot limits, separate from [`HttpServerOptions`] so tests
/// can shorten them without a user-facing knob.
#[derive(Clone, Copy, Debug)]
struct ServerLimits {
    client_timeout: Duration,
    header_deadline: Duration,
    response_grace: Duration,
    response_min_bytes_per_second: u64,
    max_requests: usize,
    max_requests_per_peer: usize,
}

impl ServerLimits {
    const DEFAULT: Self = Self {
        client_timeout: CLIENT_TIMEOUT,
        header_deadline: HEADER_DEADLINE,
        response_grace: RESPONSE_GRACE,
        response_min_bytes_per_second: RESPONSE_MIN_BYTES_PER_SECOND,
        max_requests: MAX_CONCURRENT_REQUESTS,
        max_requests_per_peer: MAX_REQUESTS_PER_PEER,
    };
}

pub fn start_http_server(
    root: &Path,
    port: u16,
    options: HttpServerOptions,
) -> Result<OpenHttpServer> {
    start_http_server_with_limits(root, port, options, ServerLimits::DEFAULT)
}

fn start_http_server_with_limits(
    root: &Path,
    port: u16,
    options: HttpServerOptions,
    limits: ServerLimits,
) -> Result<OpenHttpServer> {
    let root = fs::canonicalize(root)
        .with_context(|| format!("resolving HTTP server root {}", root.display()))?;
    anyhow::ensure!(root.is_dir(), "HTTP server root is not a directory");
    let listener = TcpListener::bind(("0.0.0.0", port))
        .with_context(|| format!("binding HTTP server to TCP port {port}"))?;
    // The accept loop blocks rather than polling, so an idle server costs no
    // wakeups and an incoming connection is served without waiting out a poll
    // interval. `ServerControl` unblocks the pending accept on shutdown.
    let address = listener.local_addr()?;
    let active = Arc::new(AtomicBool::new(true));
    let (log_tx, log_rx) = mpsc::channel();
    let hostname = gethostname::gethostname();
    let context = Arc::new(ServerContext {
        root: root.clone(),
        serve_dotfiles: options.serve_dotfiles,
        hosts: HostPolicy::new(&options, hostname.to_str()),
        limits,
        logs: log_tx,
    });
    let worker_active = active.clone();
    thread::Builder::new()
        .name("http-server".to_owned())
        .spawn(move || server_loop(&listener, &context, &worker_active))
        .context("starting HTTP server worker")?;

    Ok(OpenHttpServer {
        reader: Box::new(LogReader {
            receiver: log_rx,
            pending: Vec::new(),
            offset: 0,
        }),
        writer: Box::new(ServerControl { active, address }),
        address,
        root,
    })
}

/// Everything a request worker reads, shared rather than cloned per request.
struct ServerContext {
    root: PathBuf,
    serve_dotfiles: bool,
    hosts: HostPolicy,
    limits: ServerLimits,
    logs: Sender<Vec<u8>>,
}

struct LogReader {
    receiver: Receiver<Vec<u8>>,
    pending: Vec<u8>,
    offset: usize,
}

impl Read for LogReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.offset == self.pending.len() {
            match self.receiver.recv_timeout(LOG_POLL_INTERVAL) {
                Ok(message) => {
                    self.pending = message;
                    self.offset = 0;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "no HTTP log data",
                    ));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(0),
            }
        }
        let count = output.len().min(self.pending.len() - self.offset);
        output[..count].copy_from_slice(&self.pending[self.offset..self.offset + count]);
        self.offset += count;
        Ok(count)
    }
}

struct ServerControl {
    active: Arc<AtomicBool>,
    address: SocketAddr,
}

impl Write for ServerControl {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        Ok(input.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for ServerControl {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
        wake_accept_loop(self.address);
    }
}

/// The worker parks in a blocking `accept`, so clearing `active` alone would
/// not be observed until the next connection. A throwaway loopback connection
/// returns the pending `accept` immediately, letting the worker see the cleared
/// flag and exit.
fn wake_accept_loop(address: SocketAddr) {
    let wake_address = match address {
        SocketAddr::V4(_) => SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, address.port())),
        SocketAddr::V6(_) => SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, address.port())),
    };
    if let Ok(stream) = TcpStream::connect_timeout(&wake_address, SHUTDOWN_WAKE_TIMEOUT) {
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
}

/// Requests in progress, overall and per peer address.
struct RequestSlots {
    in_use: Mutex<SlotCounts>,
    max_total: usize,
    max_per_peer: usize,
}

#[derive(Default)]
struct SlotCounts {
    total: usize,
    per_peer: HashMap<IpAddr, usize>,
}

#[derive(Debug, PartialEq, Eq)]
enum SlotRefusal {
    ServerBusy,
    PeerBusy,
}

impl RequestSlots {
    fn new(max_total: usize, max_per_peer: usize) -> Arc<Self> {
        Arc::new(Self {
            in_use: Mutex::new(SlotCounts::default()),
            max_total,
            max_per_peer,
        })
    }

    fn acquire(self: &Arc<Self>, peer: IpAddr) -> std::result::Result<SlotGuard, SlotRefusal> {
        let mut counts = self
            .in_use
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if counts.total >= self.max_total {
            return Err(SlotRefusal::ServerBusy);
        }
        let peer_count = counts.per_peer.entry(peer).or_default();
        if *peer_count >= self.max_per_peer {
            return Err(SlotRefusal::PeerBusy);
        }
        *peer_count += 1;
        counts.total += 1;
        Ok(SlotGuard {
            slots: self.clone(),
            peer,
        })
    }

    fn release(&self, peer: IpAddr) {
        let mut counts = self
            .in_use
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        counts.total = counts.total.saturating_sub(1);
        if let Some(count) = counts.per_peer.get_mut(&peer) {
            *count -= 1;
            if *count == 0 {
                counts.per_peer.remove(&peer);
            }
        }
    }
}

struct SlotGuard {
    slots: Arc<RequestSlots>,
    peer: IpAddr,
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        self.slots.release(self.peer);
    }
}

fn server_loop(listener: &TcpListener, context: &Arc<ServerContext>, active: &AtomicBool) {
    log_startup(listener, context);
    let slots = RequestSlots::new(
        context.limits.max_requests,
        context.limits.max_requests_per_peer,
    );
    while active.load(Ordering::Acquire) {
        let (mut stream, peer) = match listener.accept() {
            Ok(connection) => connection,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                log_line(&context.logs, format!("Server socket error: {error}"));
                break;
            }
        };
        // A shutdown wakeup arrives as a real connection; recheck before
        // spending a worker on it.
        if !active.load(Ordering::Acquire) {
            break;
        }
        let slot = match slots.acquire(peer.ip()) {
            Ok(slot) => slot,
            Err(refusal) => {
                let _ = write_error_response(&mut stream, 503, "Service Unavailable", "", false);
                let reason = match refusal {
                    SlotRefusal::ServerBusy => "server is busy".to_owned(),
                    SlotRefusal::PeerBusy => format!(
                        "{} already has {} requests in progress",
                        peer.ip(),
                        context.limits.max_requests_per_peer
                    ),
                };
                log_line(
                    &context.logs,
                    format!("Rejected request from {peer}: {reason}"),
                );
                continue;
            }
        };
        let request_context = context.clone();
        let worker = move || {
            let _slot = slot;
            if let Err(error) = serve_connection(stream, peer, &request_context) {
                log_line(
                    &request_context.logs,
                    format!("Request from {peer} failed: {error}"),
                );
            }
        };
        // A failed spawn drops the closure, and with it the slot.
        if let Err(error) = thread::Builder::new()
            .name("http-request".to_owned())
            .spawn(worker)
        {
            log_line(
                &context.logs,
                format!("Request from {peer} failed: could not start worker: {error}"),
            );
        }
    }
}

fn log_startup(listener: &TcpListener, context: &ServerContext) {
    let logs = &context.logs;
    log_line(logs, "Zetta HTTP server".to_owned());
    log_line(
        logs,
        format!("Serving {} (read only)", context.root.display()),
    );
    log_line(
        logs,
        if context.serve_dotfiles {
            "Serving dot-prefixed names such as .git and .env".to_owned()
        } else {
            "Refusing dot-prefixed names such as .git and .env (--serve-dotfiles serves them)"
                .to_owned()
        },
    );
    log_line(logs, context.hosts.describe());
    log_line(
        logs,
        format!(
            "Listening on http://{}/",
            listener
                .local_addr()
                .map_or_else(|_| "TCP".to_owned(), |address| address.to_string())
        ),
    );
}

fn log_line(logs: &Sender<Vec<u8>>, message: String) {
    let timestamp = OffsetDateTime::now_utc()
        .format(format_description!(
            "[year]-[month]-[day] [hour]:[minute]:[second] UTC"
        ))
        .expect("the static HTTP timestamp format must be valid");
    let message = message.trim_end_matches(['\r', '\n']);
    let _ = logs.send(format!("[{timestamp}] {message}\r\n").into_bytes());
}

#[derive(Debug, PartialEq, Eq)]
struct HttpRequest {
    method: String,
    target: String,
    /// The `Host` header's value; `None` when the request carried none.
    host: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum RequestError {
    Malformed(String),
    /// Part of a head arrived, but not all of it before the deadline or a
    /// read timeout.
    TimedOut,
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(message) => formatter.write_str(message),
            Self::TimedOut => formatter.write_str("the request head did not arrive in time"),
        }
    }
}

fn serve_connection(stream: TcpStream, peer: SocketAddr, context: &ServerContext) -> Result<()> {
    let accepted = Instant::now();
    // Listener nonblocking state is not consistently inherited across platforms.
    // Request workers use blocking I/O with explicit timeouts.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(context.limits.client_timeout))?;
    stream.set_write_timeout(Some(context.limits.client_timeout))?;
    let local = stream.local_addr()?.ip();
    let read = read_request(
        &stream,
        accepted + context.limits.header_deadline,
        context.limits.client_timeout,
    );
    let mut response = ResponseWriter::new(&stream, &context.limits);
    let request = match read {
        Ok(Some(request)) => request,
        Ok(None) => return Ok(()),
        Err(error) => {
            log_line(
                &context.logs,
                format!("Rejected request from {peer}: {error}"),
            );
            let (status, reason) = match error {
                RequestError::Malformed(_) => (400, "Bad Request"),
                RequestError::TimedOut => (408, "Request Timeout"),
            };
            write_error_response(&mut response, status, reason, "", false)?;
            return Ok(());
        }
    };
    let head_only = request.method == "HEAD";
    let summary = format!("{:?} {:?}", request.method, request.target);
    if !context.hosts.allows(request.host.as_deref(), local) {
        let host = request.host.as_deref().unwrap_or_default();
        let detail = HostPolicy::refusal(host);
        write_error_response(
            &mut response,
            421,
            "Misdirected Request",
            &detail,
            head_only,
        )?;
        log_line(
            &context.logs,
            format!(
                "{summary} -> 421 {peer} (Host {host:?} is not this machine; --allow-host admits it)"
            ),
        );
        return Ok(());
    }
    if request.method != "GET" && !head_only {
        write_method_not_allowed(&mut response)?;
        log_line(&context.logs, format!("{summary} -> 405 {peer}"));
        return Ok(());
    }
    let content = match resolve_request_path(&context.root, &request.target, context.serve_dotfiles)
    {
        Ok(content) => content,
        Err(error) => {
            let (status, reason, note) = match error {
                ResolveError::Forbidden => (403, "Forbidden", ""),
                ResolveError::DotPrefixed => (
                    403,
                    "Forbidden",
                    " (dot-prefixed name; --serve-dotfiles serves it)",
                ),
                ResolveError::NotFound => (404, "Not Found", ""),
            };
            write_error_response(&mut response, status, reason, "", head_only)?;
            log_line(&context.logs, format!("{summary} -> {status} {peer}{note}"));
            return Ok(());
        }
    };
    let served = match content {
        ResolvedContent::File(path) => serve_file(&mut response, &path, head_only)?,
        ResolvedContent::Directory(path) => serve_directory(
            &mut response,
            &path,
            &request.target,
            context.serve_dotfiles,
            head_only,
        )?,
    };
    log_line(&context.logs, format!("{summary} -> 200 {peer} ({served})"));
    Ok(())
}

/// Writes a response under its deadline: the first `n` bytes must be sent by
/// `start + response_grace + n / response_min_bytes_per_second`.
///
/// The socket's write timeout stays at `client_timeout` until the deadline is
/// nearer than that, so a transfer keeping pace costs no extra syscalls.
struct ResponseWriter<'a> {
    stream: &'a TcpStream,
    start: Instant,
    written: u64,
    limits: ServerLimits,
    timeout: Duration,
}

impl<'a> ResponseWriter<'a> {
    fn new(stream: &'a TcpStream, limits: &ServerLimits) -> Self {
        Self {
            stream,
            start: Instant::now(),
            written: 0,
            limits: *limits,
            timeout: limits.client_timeout,
        }
    }

    /// `None` when the deadline is too far off for `Instant` to represent.
    fn deadline_for(&self, bytes: u64) -> Option<Instant> {
        self.start
            .checked_add(response_allowance(&self.limits, bytes))
    }
}

/// How long a response may take to send its first `bytes` bytes.
fn response_allowance(limits: &ServerLimits, bytes: u64) -> Duration {
    let rate = u128::from(limits.response_min_bytes_per_second.max(1));
    let nanos = u128::from(bytes) * 1_000_000_000 / rate;
    limits.response_grace + Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

impl Write for ResponseWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let deadline = self.deadline_for(self.written.saturating_add(buffer.len() as u64));
        let remaining = deadline.map_or(Duration::MAX, |deadline| {
            deadline.saturating_duration_since(Instant::now())
        });
        if remaining.is_zero() {
            return Err(response_deadline_error());
        }
        let timeout = remaining.min(self.limits.client_timeout);
        if timeout != self.timeout {
            self.stream.set_write_timeout(Some(timeout))?;
            self.timeout = timeout;
        }
        match self.stream.write(buffer) {
            Ok(count) => {
                self.written += count as u64;
                Ok(count)
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) && deadline.is_some_and(|deadline| Instant::now() >= deadline) =>
            {
                Err(response_deadline_error())
            }
            Err(error) => Err(error),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

fn response_deadline_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        "the client read the response too slowly",
    )
}

fn serve_file(response: &mut ResponseWriter<'_>, path: &Path, head_only: bool) -> Result<String> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let content_type = content_type(path);
    write!(
        response,
        "HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nContent-Type: {content_type}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n\r\n"
    )?;
    if !head_only {
        // A plain read/write loop rather than `io::copy`, so every chunk goes
        // through the response deadline.
        let mut buffer = vec![0; COPY_BUFFER_SIZE];
        loop {
            let count = match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            };
            response.write_all(&buffer[..count])?;
        }
    }
    response.flush()?;
    Ok(format!("{size} bytes"))
}

fn serve_directory(
    response: &mut ResponseWriter<'_>,
    path: &Path,
    target: &str,
    serve_dotfiles: bool,
    head_only: bool,
) -> Result<String> {
    let body = directory_index(path, target, serve_dotfiles)?;
    write!(
        response,
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n\r\n",
        body.len()
    )?;
    if !head_only {
        response.write_all(body.as_bytes())?;
    }
    response.flush()?;
    Ok("directory index".to_owned())
}

/// Reads the request head, which has to be complete by `deadline` however
/// steadily its bytes arrive. Each read also waits at most `client_timeout`.
///
/// Returns `Ok(None)` for a connection that sent nothing at all, which is what
/// browsers' speculative connections and the shutdown wakeup look like.
fn read_request(
    mut stream: &TcpStream,
    deadline: Instant,
    client_timeout: Duration,
) -> std::result::Result<Option<HttpRequest>, RequestError> {
    let mut bytes = Vec::with_capacity(1024);
    let mut buffer = [0; 1024];
    let mut timeout = client_timeout;
    let header_end = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err(RequestError::TimedOut)
            };
        }
        if remaining < timeout {
            timeout = remaining;
            stream.set_read_timeout(Some(timeout)).map_err(|error| {
                RequestError::Malformed(format!("reading HTTP request: {error}"))
            })?;
        }
        let count = match stream.read(&mut buffer) {
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return if bytes.is_empty() {
                    Ok(None)
                } else {
                    Err(RequestError::TimedOut)
                };
            }
            Err(error) => {
                return Err(RequestError::Malformed(format!(
                    "reading HTTP request: {error}"
                )));
            }
        };
        if count == 0 {
            if bytes.is_empty() {
                return Ok(None);
            }
            return Err(RequestError::Malformed(
                "connection closed before the HTTP request was complete".to_owned(),
            ));
        }
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            if end + 4 > MAX_REQUEST_SIZE {
                return Err(RequestError::Malformed(
                    "HTTP request headers are too large".to_owned(),
                ));
            }
            break end;
        }
        if bytes.len() >= MAX_REQUEST_SIZE {
            return Err(RequestError::Malformed(
                "HTTP request headers are too large".to_owned(),
            ));
        }
    };
    parse_request_head(&bytes[..header_end])
        .map(Some)
        .map_err(RequestError::Malformed)
}

fn parse_request_head(head: &[u8]) -> std::result::Result<HttpRequest, String> {
    let mut lines = head
        .split(|byte| *byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line));
    let line = lines.next().unwrap_or_default();
    // Header field values are opaque to this server and may legally contain
    // non-UTF-8 bytes. Only the request line and `Host` need text parsing.
    let line = std::str::from_utf8(line).map_err(|_| "HTTP request line is not UTF-8")?;
    let mut fields = line.split_ascii_whitespace();
    let method = fields.next().ok_or("HTTP method is missing")?;
    let target = fields.next().ok_or("HTTP request target is missing")?;
    let version = fields.next().ok_or("HTTP version is missing")?;
    if fields.next().is_some() || !matches!(version, "HTTP/1.0" | "HTTP/1.1") {
        return Err("HTTP request line is malformed".to_owned());
    }
    if !target.starts_with('/') {
        return Err("only origin-form HTTP request targets are supported".to_owned());
    }
    let mut host = None;
    for line in lines {
        let Some(colon) = line.iter().position(|byte| *byte == b':') else {
            continue;
        };
        if !line[..colon].eq_ignore_ascii_case(b"host") {
            continue;
        }
        if host.is_some() {
            return Err("HTTP request has more than one Host header".to_owned());
        }
        let value =
            std::str::from_utf8(&line[colon + 1..]).map_err(|_| "HTTP Host header is not UTF-8")?;
        host = Some(value.trim_matches([' ', '\t']).to_owned());
    }
    Ok(HttpRequest {
        method: method.to_owned(),
        target: target.to_owned(),
        host,
    })
}

/// Which `Host` names the server answers to. See the module documentation.
struct HostPolicy {
    any: bool,
    /// Normalised names: `localhost`, this machine's names, and `--allow-host`.
    names: Vec<String>,
}

impl HostPolicy {
    fn new(options: &HttpServerOptions, hostname: Option<&str>) -> Self {
        let mut names = vec!["localhost".to_owned()];
        if let Some(full) = hostname.and_then(|name| normalize_allowed_host(name).ok()) {
            let short = full.split('.').next().unwrap_or(&full).to_owned();
            names.push(format!("{short}.local"));
            if short != full {
                names.push(short);
            }
            names.push(full);
        }
        for name in &options.allowed_hosts {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        Self {
            any: options.allow_any_host,
            names,
        }
    }

    /// Whether to answer a request carrying `host` that arrived on `local`.
    fn allows(&self, host: Option<&str>, local: IpAddr) -> bool {
        if self.any {
            return true;
        }
        // HTTP/1.0 clients send no Host, and HTTP/1.1 sends an empty one when
        // the target has no authority. Neither names another site, which is
        // what a rebinding page cannot avoid doing.
        let Some(host) = host.filter(|host| !host.is_empty()) else {
            return true;
        };
        let Some(name) = normalize_host_header(host) else {
            return false;
        };
        if let Ok(address) = name.parse::<IpAddr>()
            && (address.is_loopback() || address.to_canonical() == local.to_canonical())
        {
            return true;
        }
        self.names.contains(&name)
    }

    fn describe(&self) -> String {
        if self.any {
            "Answering every Host name (--allow-any-host)".to_owned()
        } else {
            format!(
                "Answering Host names {}, and this machine's addresses (--allow-host NAME adds one)",
                self.names.join(", ")
            )
        }
    }

    /// The body text for a 421, naming the flag that admits `host`.
    fn refusal(host: &str) -> String {
        match normalize_host_header(host) {
            Some(name) => format!(
                "This server does not answer to the host name {name:?}.\nIf that name is this machine, start it with `zetta http server --allow-host {name}`,\nor with --allow-any-host to answer every name.\n"
            ),
            None => format!("The Host header {host:?} is not a valid host name.\n"),
        }
    }
}

/// The host part of a `Host` header value: the port and IPv6 brackets
/// removed, lowercased, without a trailing dot. `None` when it is malformed.
fn normalize_host_header(value: &str) -> Option<String> {
    let value = value.trim();
    let all_digits = |port: &str| port.bytes().all(|byte| byte.is_ascii_digit());
    let host = if let Some(rest) = value.strip_prefix('[') {
        let (inner, after) = rest.split_once(']')?;
        if !after.is_empty() && !after.strip_prefix(':').is_some_and(all_digits) {
            return None;
        }
        inner
    } else {
        match value.split_once(':') {
            Some((host, port)) if all_digits(port) => host,
            Some(_) => return None,
            None => value,
        }
    };
    let host = host.strip_suffix('.').unwrap_or(host);
    // Address literals compare as addresses, so `[FE80::0001]` and an
    // `--allow-host fe80::1` meet.
    if let Ok(address) = host.parse::<IpAddr>() {
        return Some(address.to_string());
    }
    let valid = !host.is_empty()
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'));
    valid.then(|| host.to_ascii_lowercase())
}

/// Normalises an `--allow-host` value the way [`normalize_host_header`]
/// normalises what a request carries, so the two compare equal. A port is
/// refused rather than dropped, because it suggests the user expects it to
/// matter.
pub fn normalize_allowed_host(value: &str) -> std::result::Result<String, String> {
    let trimmed = value.trim();
    let bracketed = trimmed
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'));
    let candidate = bracketed.unwrap_or(trimmed);
    let is_ipv6 = candidate.parse::<std::net::Ipv6Addr>().is_ok();
    if candidate.contains(':') && !is_ipv6 {
        return Err(format!(
            "--allow-host takes a host name or address without a port, not {value:?}"
        ));
    }
    let header = if is_ipv6 {
        format!("[{candidate}]")
    } else {
        candidate.to_owned()
    };
    normalize_host_header(&header)
        .ok_or_else(|| format!("--allow-host {value:?} is not a valid host name or address"))
}

#[derive(Debug, PartialEq, Eq)]
enum ResolveError {
    Forbidden,
    /// A component of the request, or of the path it resolves to, starts with
    /// a dot and dotfiles are not served.
    DotPrefixed,
    NotFound,
}

#[derive(Debug, PartialEq, Eq)]
enum ResolvedContent {
    File(PathBuf),
    Directory(PathBuf),
}

/// Whether any component of `relative` starts with a dot.
///
/// Downloads refuse these unless `--serve-dotfiles` is given. Containment
/// already stops a peer escaping the root, but a build directory that is also
/// a checkout holds `.git/` (with any credential in its remote URL), `.env`
/// files, and other configuration a firmware download has no reason to need.
/// The TFTP server's uploads apply the same test to the names they create.
fn has_dot_prefixed_component(relative: &Path) -> bool {
    relative.components().any(|component| {
        matches!(component, Component::Normal(name) if name.as_encoded_bytes().starts_with(b"."))
    })
}

/// Whether the canonical `path` reaches a dot-prefixed name below `root`.
fn resolves_through_dot_component(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root)
        .is_ok_and(has_dot_prefixed_component)
}

fn resolve_request_path(
    root: &Path,
    target: &str,
    serve_dotfiles: bool,
) -> std::result::Result<ResolvedContent, ResolveError> {
    let target = target.split_once('?').map_or(target, |(path, _)| path);
    let decoded = percent_decode(target).ok_or(ResolveError::Forbidden)?;
    let relative = decoded.strip_prefix('/').ok_or(ResolveError::Forbidden)?;
    let relative = Path::new(relative);
    if relative.is_absolute()
        || !relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(ResolveError::Forbidden);
    }
    // Checked before the filesystem is touched, so a refusal says nothing
    // about whether the name exists.
    if !serve_dotfiles && has_dot_prefixed_component(relative) {
        return Err(ResolveError::DotPrefixed);
    }
    let path = fs::canonicalize(root.join(relative)).map_err(|_| ResolveError::NotFound)?;
    if !path.starts_with(root) {
        return Err(ResolveError::Forbidden);
    }
    // Checked again after resolution: a symlink without a dot, such as
    // `cfg -> .config`, would otherwise reach the directory the request could
    // not name.
    if !serve_dotfiles && resolves_through_dot_component(root, &path) {
        return Err(ResolveError::DotPrefixed);
    }
    if path.is_dir() {
        return match fs::canonicalize(path.join("index.html")) {
            Ok(index) if !index.starts_with(root) => Err(ResolveError::Forbidden),
            Ok(index) if !serve_dotfiles && resolves_through_dot_component(root, &index) => {
                Err(ResolveError::DotPrefixed)
            }
            Ok(index) if index.is_file() => Ok(ResolvedContent::File(index)),
            Ok(_) => Ok(ResolvedContent::Directory(path)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Ok(ResolvedContent::Directory(path))
            }
            Err(_) => Err(ResolveError::NotFound),
        };
    }
    if !path.is_file() {
        return Err(ResolveError::NotFound);
    }
    Ok(ResolvedContent::File(path))
}

fn directory_index(path: &Path, target: &str, serve_dotfiles: bool) -> Result<String> {
    let decoded_path = target.split_once('?').map_or(target, |(path, _)| path);
    let decoded_path = percent_decode(decoded_path).context("decoding directory URL")?;
    let mut base_url = url_encode_path(&decoded_path);
    if !base_url.ends_with('/') {
        base_url.push('/');
    }
    let mut entries = fs::read_dir(path)
        .with_context(|| format!("reading directory {}", path.display()))?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().into_string().ok()?;
            if !serve_dotfiles && name.starts_with('.') {
                return None;
            }
            let is_directory = entry.file_type().ok()?.is_dir();
            Some((name, is_directory))
        })
        .collect::<Vec<_>>();
    entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));

    let title = format!("Index of {}", html_escape(&decoded_path));
    let mut body = format!(
        "<!doctype html>\n<html>\n<head><meta charset=\"utf-8\"><title>{title}</title></head>\n<body>\n<h1>{title}</h1>\n<ul>\n"
    );
    if decoded_path != "/" {
        body.push_str("<li><a href=\"../\">../</a></li>\n");
    }
    for (name, is_directory) in entries {
        let suffix = if is_directory { "/" } else { "" };
        let href = format!("{base_url}{}{suffix}", url_encode_segment(&name));
        let label = format!("{}{suffix}", html_escape(&name));
        body.push_str(&format!("<li><a href=\"{href}\">{label}</a></li>\n"));
    }
    body.push_str("</ul>\n</body>\n</html>\n");
    Ok(body)
}

fn html_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn url_encode_path(value: &str) -> String {
    value
        .split('/')
        .map(url_encode_segment)
        .collect::<Vec<_>>()
        .join("/")
}

fn url_encode_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(b"0123456789ABCDEF"[(byte >> 4) as usize]));
            encoded.push(char::from(b"0123456789ABCDEF"[(byte & 0x0f) as usize]));
        }
    }
    encoded
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex_value(*bytes.get(index + 1)?)?;
            let low = hex_value(*bytes.get(index + 2)?)?;
            decoded.push(high * 16 + low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    let decoded = String::from_utf8(decoded).ok()?;
    (!decoded.contains('\0')).then_some(decoded)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("css") => "text/css; charset=utf-8",
        Some("gif") => "image/gif",
        Some("htm" | "html") => "text/html; charset=utf-8",
        Some("ico") => "image/x-icon",
        Some("jpeg" | "jpg") => "image/jpeg",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("txt" | "log") => "text/plain; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("xml") => "application/xml",
        _ => "application/octet-stream",
    }
}

/// Writes a plain-text error whose body is the status line followed by
/// `detail`, which is either empty or ends in a newline.
fn write_error_response(
    stream: &mut impl Write,
    status: u16,
    reason: &str,
    detail: &str,
    head_only: bool,
) -> io::Result<()> {
    let body = format!("{status} {reason}\n{detail}");
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n\r\n",
        body.len()
    )?;
    if !head_only {
        stream.write_all(body.as_bytes())?;
    }
    stream.flush()
}

fn write_method_not_allowed(stream: &mut impl Write) -> io::Result<()> {
    let body = b"405 Method Not Allowed\n";
    write!(
        stream,
        "HTTP/1.1 405 Method Not Allowed\r\nAllow: GET, HEAD\r\nContent-Length: {}\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()
}

#[cfg(test)]
#[path = "tests/server.rs"]
mod tests;
