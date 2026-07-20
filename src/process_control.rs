use std::{
    fs,
    io::{Read as _, Write as _},
    net::{SocketAddr, TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, SystemTime},
};

use anyhow::{Context as _, Result};
use futures::channel::mpsc::UnboundedSender;
use serde::{Deserialize, Serialize};
use sysinfo::{Pid, ProcessesToUpdate, System};

const CONTROL_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProcessControlCommand {
    OpenWindow,
}

#[derive(Serialize, Deserialize)]
struct ControlEndpoint {
    version: u32,
    process_id: u32,
    address: SocketAddr,
    token: String,
}

#[derive(Serialize, Deserialize)]
struct ControlRequest {
    token: String,
    command: String,
}

pub(crate) struct ProcessControlServer {
    endpoint_path: PathBuf,
    address: SocketAddr,
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
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .context("binding the Zetta process control listener")?;
        let address = listener.local_addr()?;
        let token = format!(
            "{:x}-{:x}-{:x}",
            std::process::id(),
            address.port(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let endpoint = ControlEndpoint {
            version: CONTROL_VERSION,
            process_id: std::process::id(),
            address,
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
                    let mut request = Vec::new();
                    if std::io::Read::by_ref(&mut stream)
                        .take(4096)
                        .read_to_end(&mut request)
                        .is_err()
                    {
                        continue;
                    }
                    let accepted = decode_control_request(&request, &token)
                        .is_some_and(|command| commands.unbounded_send(command).is_ok());
                    let _ = stream.write_all(if accepted { b"ok" } else { b"rejected" });
                }
            })
            .context("starting the Zetta process control thread")?;
        Ok(Self {
            endpoint_path,
            address,
            stopping,
            thread: Some(thread),
        })
    }
}

fn decode_control_request(request: &[u8], token: &str) -> Option<ProcessControlCommand> {
    let request = serde_json::from_slice::<ControlRequest>(request).ok()?;
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
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_millis(100));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.endpoint_path);
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
            continue;
        }
        if send_open_window_request(&endpoint).unwrap_or(false) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn send_open_window_request(endpoint: &ControlEndpoint) -> Result<bool> {
    let mut stream = TcpStream::connect_timeout(&endpoint.address, Duration::from_millis(300))?;
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    stream.write_all(&serde_json::to_vec(&ControlRequest {
        token: endpoint.token.clone(),
        command: "open_window".to_owned(),
    })?)?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response == "ok")
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

fn write_endpoint(path: &PathBuf, endpoint: &ControlEndpoint) -> Result<()> {
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
