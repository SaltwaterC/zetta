use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, UdpSocket};
use std::path::{Component, Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc::{self, Receiver, Sender},
};
use std::thread;
use std::time::Duration;

use anyhow::{Context as _, Result};
use time::{OffsetDateTime, macros::format_description};

use super::{
    DEFAULT_BLOCK_SIZE, MAX_BLOCK_SIZE, MAX_RETRIES, MIN_BLOCK_SIZE, OP_ACK, OP_DATA, OP_ERROR,
    OP_RRQ, OP_WRQ, SOCKET_TIMEOUT, ack_packet, error_message, option_ack_packet, packet_block,
    packet_opcode, read_block, send_error, send_packet, set_data_packet,
    socket_operation_was_interrupted, transfer_socket, zero_terminated_fields,
};

const SERVER_POLL_INTERVAL: Duration = Duration::from_millis(200);
const LOG_POLL_INTERVAL: Duration = Duration::from_millis(100);
const MAX_CONCURRENT_TRANSFERS: usize = 32;

pub(crate) struct OpenTftpServer {
    pub(crate) reader: Box<dyn Read + Send>,
    pub(crate) writer: Box<dyn Write + Send>,
    pub(crate) address: SocketAddr,
    pub(crate) root: PathBuf,
}

pub(crate) fn start_server(root: &Path, port: u16) -> Result<OpenTftpServer> {
    let root = fs::canonicalize(root)
        .with_context(|| format!("resolving TFTP server root {}", root.display()))?;
    anyhow::ensure!(root.is_dir(), "TFTP server root is not a directory");
    let socket = UdpSocket::bind(("0.0.0.0", port))
        .with_context(|| format!("binding TFTP server to UDP port {port}"))?;
    socket.set_read_timeout(Some(SERVER_POLL_INTERVAL))?;
    let address = socket.local_addr()?;
    let active = Arc::new(AtomicBool::new(true));
    let (log_tx, log_rx) = mpsc::channel();
    let worker_root = root.clone();
    let worker_active = active.clone();
    thread::Builder::new()
        .name("tftp-server".to_owned())
        .spawn(move || server_loop(socket, worker_root, worker_active, log_tx))
        .context("starting TFTP server worker")?;

    Ok(OpenTftpServer {
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
                        "no TFTP log data",
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

#[derive(Debug)]
struct Request {
    write: bool,
    filename: String,
    mode: String,
    options: Vec<(String, String)>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct RequestKey {
    peer: SocketAddr,
    write: bool,
    filename: String,
}

struct TransferGuard {
    count: Arc<AtomicUsize>,
    active_requests: Arc<Mutex<HashSet<RequestKey>>>,
    request: RequestKey,
}

impl Drop for TransferGuard {
    fn drop(&mut self) {
        self.count.fetch_sub(1, Ordering::Relaxed);
        remove_active_request(&self.active_requests, &self.request);
    }
}

fn register_active_request(
    active_requests: &Mutex<HashSet<RequestKey>>,
    request: &RequestKey,
) -> bool {
    active_requests
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(request.clone())
}

fn remove_active_request(active_requests: &Mutex<HashSet<RequestKey>>, request: &RequestKey) {
    active_requests
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(request);
}

fn server_loop(socket: UdpSocket, root: PathBuf, active: Arc<AtomicBool>, logs: Sender<Vec<u8>>) {
    log_line(&logs, "Zetta TFTP server".to_owned());
    log_line(&logs, format!("Serving {}", root.display()));
    log_line(
        &logs,
        format!(
            "Listening on {}",
            socket
                .local_addr()
                .map_or_else(|_| "UDP".to_owned(), |address| address.to_string())
        ),
    );
    let mut buffer = vec![0; u16::MAX as usize + 1];
    let active_transfers = Arc::new(AtomicUsize::new(0));
    let active_requests = Arc::new(Mutex::new(HashSet::new()));
    while active.load(Ordering::Acquire) {
        let (size, peer) = match socket.recv_from(&mut buffer) {
            Ok(value) => value,
            Err(error) if socket_operation_was_interrupted(&error) => continue,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(error) => {
                log_line(&logs, format!("Server socket error: {error}\r\n"));
                break;
            }
        };
        let request = match parse_request(&buffer[..size]) {
            Ok(request) => request,
            Err(message) => {
                send_error(&socket, peer, 4, &message);
                log_line(
                    &logs,
                    format!("Rejected request from {peer}: {message}\r\n"),
                );
                continue;
            }
        };
        let request_key = RequestKey {
            peer,
            write: request.write,
            filename: request.filename.clone(),
        };
        if !register_active_request(&active_requests, &request_key) {
            continue;
        }
        if active_transfers
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                (count < MAX_CONCURRENT_TRANSFERS).then_some(count + 1)
            })
            .is_err()
        {
            remove_active_request(&active_requests, &request_key);
            send_error(&socket, peer, 0, "server is busy");
            log_line(
                &logs,
                format!("Rejected request from {peer}: server is busy\r\n"),
            );
            continue;
        }
        let transfer_root = root.clone();
        let transfer_active = active.clone();
        let transfer_logs = logs.clone();
        let transfer_count = active_transfers.clone();
        let transfer_requests = active_requests.clone();
        let transfer_key = request_key.clone();
        let transfer = move || {
            let _guard = TransferGuard {
                count: transfer_count,
                active_requests: transfer_requests,
                request: transfer_key,
            };
            let filename = request.filename.clone();
            let result = if request.write {
                serve_write_request(&transfer_root, peer, &request, &transfer_active)
            } else {
                serve_read_request(&transfer_root, peer, &request, &transfer_active)
            };
            match result {
                Ok(bytes) => log_line(
                    &transfer_logs,
                    format!(
                        "{} {filename:?} {} {peer} ({bytes} bytes)\r\n",
                        if request.write { "PUT" } else { "GET" },
                        if request.write { "<-" } else { "->" }
                    ),
                ),
                Err(error) => {
                    if let Ok(error_socket) = transfer_socket(peer.ip()) {
                        send_error(
                            &error_socket,
                            peer,
                            if request.write { 2 } else { 1 },
                            &format!("{error:#}"),
                        );
                    }
                    log_line(
                        &transfer_logs,
                        format!("Request {filename:?} from {peer} failed: {error:#}\r\n"),
                    );
                }
            }
        };
        if let Err(error) = thread::Builder::new()
            .name("tftp-transfer".to_owned())
            .spawn(transfer)
        {
            active_transfers.fetch_sub(1, Ordering::Relaxed);
            remove_active_request(&active_requests, &request_key);
            send_error(&socket, peer, 0, "could not start transfer");
            log_line(
                &logs,
                format!("Request from {peer} failed: could not start transfer worker: {error}\r\n"),
            );
        }
    }
}

fn log_line(logs: &Sender<Vec<u8>>, message: String) {
    let _ = logs.send(format_log_line(&message, OffsetDateTime::now_utc()).into_bytes());
}

fn format_log_line(message: &str, timestamp: OffsetDateTime) -> String {
    let timestamp = timestamp
        .format(format_description!(
            "[year]-[month]-[day] [hour]:[minute]:[second] UTC"
        ))
        .expect("the static TFTP timestamp format must be valid");
    format!(
        "[{timestamp}] {}\r\n",
        message.trim_end_matches(['\r', '\n'])
    )
}

fn parse_request(packet: &[u8]) -> std::result::Result<Request, String> {
    if packet.len() < 4 {
        return Err("request is too short".to_owned());
    }
    let opcode = u16::from_be_bytes([packet[0], packet[1]]);
    if !matches!(opcode, OP_RRQ | OP_WRQ) {
        return Err("expected a read or write request".to_owned());
    }
    let fields = zero_terminated_fields(&packet[2..])?;
    if fields.len() < 2 || fields.len() % 2 != 0 {
        return Err("request has malformed fields".to_owned());
    }
    let filename = String::from_utf8(fields[0].to_vec()).map_err(|_| "filename is not UTF-8")?;
    let mode = String::from_utf8(fields[1].to_vec())
        .map_err(|_| "transfer mode is not UTF-8")?
        .to_ascii_lowercase();
    let mut options = Vec::new();
    for pair in fields[2..].chunks_exact(2) {
        let name = String::from_utf8(pair[0].to_vec())
            .map_err(|_| "option name is not UTF-8")?
            .to_ascii_lowercase();
        let value = String::from_utf8(pair[1].to_vec()).map_err(|_| "option value is not UTF-8")?;
        options.push((name, value));
    }
    Ok(Request {
        write: opcode == OP_WRQ,
        filename,
        mode,
        options,
    })
}

fn safe_server_path(root: &Path, filename: &str) -> Result<PathBuf> {
    let relative = Path::new(filename);
    anyhow::ensure!(!relative.is_absolute(), "absolute paths are not served");
    anyhow::ensure!(
        relative
            .components()
            .all(|component| matches!(component, Component::Normal(_))),
        "paths outside the server root are not served"
    );
    let path =
        fs::canonicalize(root.join(relative)).with_context(|| format!("opening {filename:?}"))?;
    anyhow::ensure!(
        path.starts_with(root),
        "paths outside the server root are not served"
    );
    anyhow::ensure!(path.is_file(), "requested path is not a file");
    Ok(path)
}

struct PendingUpload {
    file: Option<File>,
    path: PathBuf,
    complete: bool,
}

impl PendingUpload {
    fn create(root: &Path, filename: &str) -> Result<Self> {
        let relative = Path::new(filename);
        anyhow::ensure!(!relative.is_absolute(), "absolute paths are not accepted");
        anyhow::ensure!(
            relative
                .components()
                .all(|component| matches!(component, Component::Normal(_))),
            "paths outside the server root are not accepted"
        );
        let path = root.join(relative);
        let parent = path
            .parent()
            .context("upload path has no parent directory")?;
        let canonical_parent = fs::canonicalize(parent)
            .with_context(|| format!("opening upload directory for {filename:?}"))?;
        anyhow::ensure!(
            canonical_parent.starts_with(root),
            "paths outside the server root are not accepted"
        );
        let path = canonical_parent.join(
            path.file_name()
                .context("upload path must include a file name")?,
        );
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .with_context(|| format!("creating upload {filename:?}"))?;
        Ok(Self {
            file: Some(file),
            path,
            complete: false,
        })
    }

    fn write_all(&mut self, data: &[u8]) -> Result<()> {
        self.file
            .as_mut()
            .context("upload file is already closed")?
            .write_all(data)?;
        Ok(())
    }

    fn finish(mut self) -> Result<()> {
        self.file
            .as_mut()
            .context("upload file is already closed")?
            .flush()?;
        self.file.take();
        self.complete = true;
        Ok(())
    }
}

impl Drop for PendingUpload {
    fn drop(&mut self) {
        if !self.complete {
            self.file.take();
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn serve_read_request(
    root: &Path,
    peer: SocketAddr,
    request: &Request,
    active: &AtomicBool,
) -> Result<u64> {
    anyhow::ensure!(
        request.mode == "octet",
        "only octet transfers are supported"
    );
    let path = safe_server_path(root, &request.filename)?;
    let mut file = File::open(&path)?;
    let file_size = file.metadata()?.len();
    let socket = transfer_socket(peer.ip())?;
    socket.set_read_timeout(Some(SOCKET_TIMEOUT))?;
    let (block_size, option_ack) = negotiated_options(&request.options, file_size);
    if !option_ack.is_empty() {
        let packet = option_ack_packet(&option_ack);
        send_with_ack(&socket, peer, &packet, 0, active)?;
    }

    let mut block = 1_u16;
    let mut total = 0_u64;
    let mut data = vec![0; block_size];
    let mut packet = Vec::with_capacity(block_size + 4);
    loop {
        let size = read_block(&mut file, &mut data)?;
        set_data_packet(&mut packet, block, &data[..size]);
        send_with_ack(&socket, peer, &packet, block, active)?;
        total += size as u64;
        if size < block_size {
            break;
        }
        block = block.wrapping_add(1);
    }
    Ok(total)
}

fn serve_write_request(
    root: &Path,
    peer: SocketAddr,
    request: &Request,
    active: &AtomicBool,
) -> Result<u64> {
    anyhow::ensure!(
        request.mode == "octet",
        "only octet transfers are supported"
    );
    let mut upload = PendingUpload::create(root, &request.filename)?;
    let socket = transfer_socket(peer.ip())?;
    socket.set_read_timeout(Some(SOCKET_TIMEOUT))?;
    let (block_size, expected_size, option_ack) = negotiated_write_options(&request.options);
    let mut response_packet = if option_ack.is_empty() {
        ack_packet(0).to_vec()
    } else {
        option_ack_packet(&option_ack)
    };
    let mut response = vec![0; block_size + 4];
    let mut expected_block = 1_u16;
    let mut total = 0_u64;
    loop {
        let size = receive_data_after_response(
            &socket,
            peer,
            &response_packet,
            expected_block,
            active,
            &mut response,
        )?;
        let data = response
            .get(4..size)
            .context("received a malformed DATA packet")?;
        upload.write_all(data)?;
        total += data.len() as u64;
        let acknowledgement = ack_packet(expected_block);
        if data.len() < block_size {
            if expected_size.is_some_and(|expected_size| total != expected_size) {
                send_error(
                    &socket,
                    peer,
                    0,
                    "upload size differs from the negotiated transfer size",
                );
                anyhow::bail!("upload size differs from the negotiated transfer size");
            }
            send_packet(&socket, &acknowledgement, peer)?;
            upload.finish()?;
            dally_after_final_ack(&socket, peer, expected_block, &acknowledgement);
            return Ok(total);
        }
        response_packet.clear();
        response_packet.extend_from_slice(&acknowledgement);
        expected_block = expected_block.wrapping_add(1);
    }
}

fn negotiated_options(
    options: &[(String, String)],
    file_size: u64,
) -> (usize, Vec<(String, String)>) {
    let mut block_size = DEFAULT_BLOCK_SIZE;
    let mut acknowledged = Vec::new();
    for (name, value) in options {
        match name.as_str() {
            "blksize" => {
                if let Ok(requested) = value.parse::<usize>()
                    && (MIN_BLOCK_SIZE..=MAX_BLOCK_SIZE).contains(&requested)
                {
                    block_size = requested;
                    acknowledged.push((name.clone(), requested.to_string()));
                }
            }
            "tsize" => acknowledged.push((name.clone(), file_size.to_string())),
            _ => {}
        }
    }
    (block_size, acknowledged)
}

fn negotiated_write_options(
    options: &[(String, String)],
) -> (usize, Option<u64>, Vec<(String, String)>) {
    let mut block_size = DEFAULT_BLOCK_SIZE;
    let mut transfer_size = None;
    let mut acknowledged = Vec::new();
    for (name, value) in options {
        match name.as_str() {
            "blksize" => {
                if let Ok(requested) = value.parse::<usize>()
                    && (MIN_BLOCK_SIZE..=MAX_BLOCK_SIZE).contains(&requested)
                {
                    block_size = requested;
                    acknowledged.push((name.clone(), requested.to_string()));
                }
            }
            "tsize" => {
                if let Ok(size) = value.parse::<u64>() {
                    transfer_size = Some(size);
                    acknowledged.push((name.clone(), size.to_string()));
                }
            }
            _ => {}
        }
    }
    (block_size, transfer_size, acknowledged)
}

fn send_with_ack(
    socket: &UdpSocket,
    peer: SocketAddr,
    packet: &[u8],
    expected_block: u16,
    active: &AtomicBool,
) -> Result<()> {
    let mut response = [0; 516];
    for _ in 0..MAX_RETRIES {
        anyhow::ensure!(active.load(Ordering::Acquire), "server stopped");
        send_packet(socket, packet, peer)?;
        loop {
            match socket.recv_from(&mut response) {
                Ok((size, source)) if source == peer => match packet_opcode(&response[..size]) {
                    Some(OP_ACK) if packet_block(&response[..size]) == Some(expected_block) => {
                        return Ok(());
                    }
                    Some(OP_ERROR) => {
                        anyhow::bail!("client returned {}", error_message(&response[..size]))
                    }
                    _ => continue,
                },
                Ok(_) => continue,
                Err(error) if socket_operation_was_interrupted(&error) => continue,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    break;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
    anyhow::bail!("transfer timed out waiting for ACK {expected_block}")
}

fn receive_data_after_response(
    socket: &UdpSocket,
    peer: SocketAddr,
    response_packet: &[u8],
    expected_block: u16,
    active: &AtomicBool,
    response: &mut [u8],
) -> Result<usize> {
    for _ in 0..MAX_RETRIES {
        anyhow::ensure!(active.load(Ordering::Acquire), "server stopped");
        send_packet(socket, response_packet, peer)?;
        loop {
            match socket.recv_from(response) {
                Ok((size, source)) if source == peer => {
                    let packet = &response[..size];
                    match packet_opcode(packet) {
                        Some(OP_DATA) if packet_block(packet) == Some(expected_block) => {
                            return Ok(size);
                        }
                        Some(OP_ERROR) => {
                            anyhow::bail!("client returned {}", error_message(packet))
                        }
                        _ => send_packet(socket, response_packet, peer)?,
                    }
                }
                Ok(_) => continue,
                Err(error) if socket_operation_was_interrupted(&error) => continue,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    break;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
    anyhow::bail!("transfer timed out waiting for DATA {expected_block}")
}

fn dally_after_final_ack(
    socket: &UdpSocket,
    peer: SocketAddr,
    final_block: u16,
    acknowledgement: &[u8],
) {
    let mut response = vec![0; MAX_BLOCK_SIZE + 4];
    loop {
        match socket.recv_from(&mut response) {
            Ok((size, source)) if source == peer => {
                let packet = &response[..size];
                if packet_opcode(packet) == Some(OP_DATA)
                    && packet_block(packet) == Some(final_block)
                    && send_packet(socket, acknowledgement, peer).is_err()
                {
                    return;
                }
            }
            Ok(_) => continue,
            Err(error) if socket_operation_was_interrupted(&error) => continue,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return;
            }
            Err(_) => return,
        }
    }
}

#[cfg(test)]
#[path = "../tests/tftp/server.rs"]
mod tests;
