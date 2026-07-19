use std::{
    fs::{self, File},
    io::{self, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Duration,
};

use anyhow::{Context as _, Result};
use time::{OffsetDateTime, macros::format_description};

pub(crate) const DEFAULT_HTTP_PORT: u16 = 8000;
const SERVER_POLL_INTERVAL: Duration = Duration::from_millis(50);
const CLIENT_TIMEOUT: Duration = Duration::from_secs(5);
const LOG_POLL_INTERVAL: Duration = Duration::from_millis(100);
const MAX_REQUEST_SIZE: usize = 64 * 1024;
const MAX_CONCURRENT_REQUESTS: usize = 32;

pub(crate) struct OpenHttpServer {
    pub(crate) reader: Box<dyn Read + Send>,
    pub(crate) writer: Box<dyn Write + Send>,
    pub(crate) address: SocketAddr,
    pub(crate) root: PathBuf,
}

pub(crate) fn start_http_server(root: &Path, port: u16) -> Result<OpenHttpServer> {
    let root = fs::canonicalize(root)
        .with_context(|| format!("resolving HTTP server root {}", root.display()))?;
    anyhow::ensure!(root.is_dir(), "HTTP server root is not a directory");
    let listener = TcpListener::bind(("0.0.0.0", port))
        .with_context(|| format!("binding HTTP server to TCP port {port}"))?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    let active = Arc::new(AtomicBool::new(true));
    let (log_tx, log_rx) = mpsc::channel();
    let worker_root = root.clone();
    let worker_active = active.clone();
    thread::Builder::new()
        .name("http-server".to_owned())
        .spawn(move || server_loop(listener, worker_root, worker_active, log_tx))
        .context("starting HTTP server worker")?;

    Ok(OpenHttpServer {
        reader: Box::new(LogReader {
            receiver: log_rx,
            pending: Vec::new(),
            offset: 0,
        }),
        writer: Box::new(ServerControl { active }),
        address,
        root,
    })
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
    }
}

struct RequestGuard(Arc<AtomicUsize>);

impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

fn server_loop(
    listener: TcpListener,
    root: PathBuf,
    active: Arc<AtomicBool>,
    logs: Sender<Vec<u8>>,
) {
    log_line(&logs, "Zetta HTTP server".to_owned());
    log_line(&logs, format!("Serving {} (read only)", root.display()));
    log_line(
        &logs,
        format!(
            "Listening on http://{}/",
            listener
                .local_addr()
                .map_or_else(|_| "TCP".to_owned(), |address| address.to_string())
        ),
    );
    let request_count = Arc::new(AtomicUsize::new(0));
    while active.load(Ordering::Acquire) {
        let (stream, peer) = match listener.accept() {
            Ok(connection) => connection,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(SERVER_POLL_INTERVAL);
                continue;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                log_line(&logs, format!("Server socket error: {error}"));
                break;
            }
        };
        if request_count
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                (count < MAX_CONCURRENT_REQUESTS).then_some(count + 1)
            })
            .is_err()
        {
            let mut stream = stream;
            let _ = write_error_response(&mut stream, 503, "Service Unavailable", false);
            log_line(
                &logs,
                format!("Rejected request from {peer}: server is busy"),
            );
            continue;
        }
        let request_root = root.clone();
        let request_logs = logs.clone();
        let worker_count = request_count.clone();
        let worker = move || {
            let _guard = RequestGuard(worker_count);
            if let Err(error) = serve_connection(stream, peer, &request_root, &request_logs) {
                log_line(
                    &request_logs,
                    format!("Request from {peer} failed: {error}"),
                );
            }
        };
        if let Err(error) = thread::Builder::new()
            .name("http-request".to_owned())
            .spawn(worker)
        {
            request_count.fetch_sub(1, Ordering::Relaxed);
            log_line(
                &logs,
                format!("Request from {peer} failed: could not start worker: {error}"),
            );
        }
    }
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
}

fn serve_connection(
    mut stream: TcpStream,
    peer: SocketAddr,
    root: &Path,
    logs: &Sender<Vec<u8>>,
) -> Result<()> {
    // Listener nonblocking state is not consistently inherited across platforms.
    // Request workers use blocking I/O with explicit timeouts.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CLIENT_TIMEOUT))?;
    let request = match read_request(&mut stream) {
        Ok(Some(request)) => request,
        Ok(None) => return Ok(()),
        Err(error) => {
            log_line(logs, format!("Rejected request from {peer}: {error}"));
            write_error_response(&mut stream, 400, "Bad Request", false)?;
            return Ok(());
        }
    };
    let head_only = request.method == "HEAD";
    if request.method != "GET" && !head_only {
        write_method_not_allowed(&mut stream)?;
        log_line(
            logs,
            format!("{:?} {:?} -> 405 {peer}", request.method, request.target),
        );
        return Ok(());
    }
    let content = match resolve_request_path(root, &request.target) {
        Ok(content) => content,
        Err(ResolveError::Forbidden) => {
            write_error_response(&mut stream, 403, "Forbidden", head_only)?;
            log_line(
                logs,
                format!("{:?} {:?} -> 403 {peer}", request.method, request.target),
            );
            return Ok(());
        }
        Err(ResolveError::NotFound) => {
            write_error_response(&mut stream, 404, "Not Found", head_only)?;
            log_line(
                logs,
                format!("{:?} {:?} -> 404 {peer}", request.method, request.target),
            );
            return Ok(());
        }
    };
    match content {
        ResolvedContent::File(path) => {
            serve_file(&mut stream, &path, head_only, peer, &request, logs)
        }
        ResolvedContent::Directory(path) => {
            serve_directory(&mut stream, &path, head_only, peer, &request, logs)
        }
    }
}

fn serve_file(
    stream: &mut TcpStream,
    path: &Path,
    head_only: bool,
    peer: SocketAddr,
    request: &HttpRequest,
    logs: &Sender<Vec<u8>>,
) -> Result<()> {
    let file = File::open(path)?;
    let size = file.metadata()?.len();
    let content_type = content_type(path);
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nContent-Type: {content_type}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n\r\n"
    )?;
    if !head_only {
        io::copy(&mut BufReader::new(file), &mut *stream)?;
    }
    stream.flush()?;
    log_line(
        logs,
        format!(
            "{:?} {:?} -> 200 {peer} ({size} bytes)",
            request.method, request.target
        ),
    );
    Ok(())
}

fn serve_directory(
    stream: &mut TcpStream,
    path: &Path,
    head_only: bool,
    peer: SocketAddr,
    request: &HttpRequest,
    logs: &Sender<Vec<u8>>,
) -> Result<()> {
    let body = directory_index(path, &request.target)?;
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n\r\n",
        body.len()
    )?;
    if !head_only {
        stream.write_all(body.as_bytes())?;
    }
    stream.flush()?;
    log_line(
        logs,
        format!(
            "{:?} {:?} -> 200 {peer} (directory index)",
            request.method, request.target
        ),
    );
    Ok(())
}

fn read_request(stream: &mut TcpStream) -> std::result::Result<Option<HttpRequest>, String> {
    let mut bytes = Vec::with_capacity(1024);
    let mut buffer = [0; 1024];
    let header_end = loop {
        let count = match stream.read(&mut buffer) {
            Ok(count) => count,
            Err(error)
                if bytes.is_empty()
                    && matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(format!("reading HTTP request: {error}")),
        };
        if count == 0 {
            if bytes.is_empty() {
                return Ok(None);
            }
            return Err("connection closed before the HTTP request was complete".to_owned());
        }
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            if end + 4 > MAX_REQUEST_SIZE {
                return Err("HTTP request headers are too large".to_owned());
            }
            break end;
        }
        if bytes.len() >= MAX_REQUEST_SIZE {
            return Err("HTTP request headers are too large".to_owned());
        }
    };
    parse_request_line(&bytes[..header_end]).map(Some)
}

fn parse_request_line(headers: &[u8]) -> std::result::Result<HttpRequest, String> {
    let line_end = headers
        .iter()
        .position(|byte| *byte == b'\n')
        .unwrap_or(headers.len());
    let line = headers[..line_end]
        .strip_suffix(b"\r")
        .unwrap_or(&headers[..line_end]);
    // Header field values are opaque to this server and may legally contain
    // non-UTF-8 bytes. Only the request line needs text parsing.
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
    Ok(HttpRequest {
        method: method.to_owned(),
        target: target.to_owned(),
    })
}

#[derive(Debug, PartialEq, Eq)]
enum ResolveError {
    Forbidden,
    NotFound,
}

#[derive(Debug, PartialEq, Eq)]
enum ResolvedContent {
    File(PathBuf),
    Directory(PathBuf),
}

fn resolve_request_path(
    root: &Path,
    target: &str,
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
    let path = fs::canonicalize(root.join(relative)).map_err(|_| ResolveError::NotFound)?;
    if !path.starts_with(root) {
        return Err(ResolveError::Forbidden);
    }
    if path.is_dir() {
        return match fs::canonicalize(path.join("index.html")) {
            Ok(index) if !index.starts_with(root) => Err(ResolveError::Forbidden),
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

fn directory_index(path: &Path, target: &str) -> Result<String> {
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

fn write_error_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    head_only: bool,
) -> io::Result<()> {
    let body = format!("{status} {reason}\n");
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

fn write_method_not_allowed(stream: &mut TcpStream) -> io::Result<()> {
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
#[path = "tests/http_server.rs"]
mod tests;
