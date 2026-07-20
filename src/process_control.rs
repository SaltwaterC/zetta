use std::{
    fs,
    io::{BufRead as _, BufReader, Read as _, Write as _},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(windows)]
use std::os::windows::net::{UnixListener, UnixStream};

use anyhow::{Context as _, Result};
use futures::channel::mpsc::UnboundedSender;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sysinfo::{Pid, ProcessesToUpdate, System};

const CONTROL_VERSION: u32 = 2;
const MAX_CONTROL_MESSAGE_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProcessControlCommand {
    OpenWindow,
}

#[derive(Serialize, Deserialize)]
struct ControlEndpoint {
    version: u32,
    process_id: u32,
    socket_path: PathBuf,
    token: String,
}

#[derive(Serialize, Deserialize)]
struct ControlRequest {
    token: String,
    command: String,
}

#[derive(Serialize, Deserialize)]
struct ControlResponse {
    status: String,
}

pub(crate) struct ProcessControlServer {
    endpoint_path: PathBuf,
    socket_path: PathBuf,
    stopping: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl ProcessControlServer {
    pub(crate) fn start(commands: UnboundedSender<ProcessControlCommand>) -> Result<Self> {
        Self::start_at(commands, control_endpoint_path(std::process::id()))
    }

    fn start_at(
        commands: UnboundedSender<ProcessControlCommand>,
        endpoint_path: PathBuf,
    ) -> Result<Self> {
        let parent = endpoint_path
            .parent()
            .context("control endpoint has no parent")?;
        fs::create_dir_all(parent)?;
        let socket_path = control_socket_path(&endpoint_path);
        remove_socket_if_present(&socket_path)?;
        let listener = UnixListener::bind(&socket_path)
            .context("binding the Zetta process control listener")?;
        let token = random_hex(32).context("generating the Zetta process control token")?;
        let endpoint = ControlEndpoint {
            version: CONTROL_VERSION,
            process_id: std::process::id(),
            socket_path: socket_path.clone(),
            token: token.clone(),
        };
        write_endpoint(&endpoint_path, &endpoint)?;
        let stopping = Arc::new(AtomicBool::new(false));
        let stopping_for_thread = stopping.clone();
        let thread = thread::Builder::new()
            .name("zetta-process-control".to_owned())
            .spawn(move || {
                for stream in listener.incoming() {
                    if stopping_for_thread.load(Ordering::Acquire) {
                        break;
                    }
                    let Ok(mut stream) = stream else {
                        continue;
                    };
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                    let accepted = handle_control_request(&mut stream, &token)
                        .is_some_and(|command| commands.unbounded_send(command).is_ok());
                    let status = if accepted { "ok" } else { "rejected" };
                    let _ = write_message(
                        &mut stream,
                        &ControlResponse {
                            status: status.to_owned(),
                        },
                    );
                }
            })
            .context("starting the Zetta process control thread")?;
        Ok(Self {
            endpoint_path,
            socket_path,
            stopping,
            thread: Some(thread),
        })
    }
}

fn handle_control_request(stream: &mut UnixStream, token: &str) -> Option<ProcessControlCommand> {
    let request = read_message::<ControlRequest>(stream).ok()?;
    decode_control_request(&request, token)
}

fn decode_control_request(request: &ControlRequest, token: &str) -> Option<ProcessControlCommand> {
    if request.token != token {
        return None;
    }
    match request.command.as_str() {
        "open_window" => Some(ProcessControlCommand::OpenWindow),
        _ => None,
    }
}

impl Drop for ProcessControlServer {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        let _ = UnixStream::connect(&self.socket_path);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.endpoint_path);
        let _ = fs::remove_file(&self.socket_path);
    }
}

pub(crate) fn request_existing_process_window() -> Result<bool> {
    let directory = crate::background_sessions::session_catalog_dir();
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("reading Zetta process control endpoints"),
    };
    for entry in entries {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("control-") && name.ends_with(".json"))
        {
            continue;
        }
        let endpoint = match fs::read(&path)
            .ok()
            .and_then(|contents| serde_json::from_slice::<ControlEndpoint>(&contents).ok())
        {
            Some(endpoint) if endpoint.version == CONTROL_VERSION => endpoint,
            _ => continue,
        };
        if !process_is_running(endpoint.process_id) {
            let _ = fs::remove_file(path);
            let _ = fs::remove_file(endpoint.socket_path);
            continue;
        }
        if send_open_window_request(&endpoint).unwrap_or(false) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn send_open_window_request(endpoint: &ControlEndpoint) -> Result<bool> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    stream.set_write_timeout(Some(Duration::from_millis(500)))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: "open_window".to_owned(),
        },
    )?;
    let response = read_message::<ControlResponse>(&mut stream)?;
    Ok(response.status == "ok")
}

fn read_message<T: DeserializeOwned>(stream: &mut UnixStream) -> Result<T> {
    let mut bytes = Vec::new();
    let mut reader = BufReader::new(stream).take((MAX_CONTROL_MESSAGE_BYTES + 1) as u64);
    reader.read_until(b'\n', &mut bytes)?;
    anyhow::ensure!(
        bytes.last() == Some(&b'\n'),
        "process control message is too long or incomplete"
    );
    bytes.pop();
    serde_json::from_slice(&bytes).context("parsing process control message")
}

fn write_message(stream: &mut UnixStream, message: &impl Serialize) -> Result<()> {
    serde_json::to_writer(&mut *stream, message)?;
    stream.write_all(b"\n")?;
    Ok(())
}

fn random_hex(byte_count: usize) -> Result<String> {
    let mut bytes = vec![0; byte_count];
    getrandom::fill(&mut bytes)?;
    Ok(encode_hex(&bytes))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0xf) as usize] as char);
    }
    encoded
}

fn process_is_running(process_id: u32) -> bool {
    let process_id = Pid::from_u32(process_id);
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[process_id]), true);
    system.process(process_id).is_some()
}

fn control_endpoint_path(process_id: u32) -> PathBuf {
    crate::background_sessions::session_catalog_dir().join(format!("control-{process_id}.json"))
}

fn control_socket_path(endpoint_path: &Path) -> PathBuf {
    endpoint_path.with_extension("sock")
}

fn remove_socket_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("removing stale socket {}", path.display()))
        }
    }
}

fn write_endpoint(path: &Path, endpoint: &ControlEndpoint) -> Result<()> {
    let parent = path.parent().context("control endpoint has no parent")?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension("json.tmp");
    let contents = serde_json::to_vec(endpoint)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?
            .write_all(&contents)?;
    }
    #[cfg(not(unix))]
    fs::write(&temporary, contents)?;
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(temporary, path)?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/process_control.rs"]
mod tests;
