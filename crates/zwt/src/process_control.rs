use std::{
    fs,
    io::{BufRead as _, BufReader, Read as _, Write as _},
    path::{Path, PathBuf},
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::net::UnixStream;
#[cfg(windows)]
use uds_windows::UnixStream;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

const MAX_CONTROL_MESSAGE_BYTES: usize = 256 * 1024;
const CONTROL_CLIENT_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeNameRequest {
    pub attention_id: u64,
    pub name: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ControlEndpoint {
    version: u32,
    process_id: u32,
    socket_path: PathBuf,
    token: String,
}

#[derive(Serialize)]
struct ControlRequest {
    token: String,
    command: String,
    attention_id: u64,
    worktree_name: Option<String>,
}

#[derive(Deserialize)]
struct ControlResponse {
    status: String,
}

/// Ask the Zetta process identified by `process_id` to update its originating
/// tab's worktree title. This is deliberately a small, best-effort client:
/// `zwt` remains useful when Zetta is not running or its endpoint has gone
/// away.
pub fn request_process_worktree_name(
    process_id: u32,
    request: WorktreeNameRequest,
) -> Result<bool> {
    let endpoint_path = control_endpoint_path(process_id);
    request_process_worktree_name_at(&endpoint_path, process_id, request)
}

fn request_process_worktree_name_at(
    endpoint_path: &Path,
    process_id: u32,
    request: WorktreeNameRequest,
) -> Result<bool> {
    anyhow::ensure!(process_id != 0, "process ID must be positive");
    anyhow::ensure!(request.attention_id != 0, "attention ID must be positive");
    let contents = fs::read(endpoint_path).with_context(|| {
        format!(
            "reading Zetta process control endpoint {}",
            endpoint_path.display()
        )
    })?;
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&contents).context("parsing Zetta process control endpoint")?;
    anyhow::ensure!(
        endpoint.version == zmux::protocol::CONTROL_VERSION && endpoint.process_id == process_id,
        "Zetta process control endpoint is outdated"
    );
    send_set_worktree_name_request(&endpoint, &request)
}

fn send_set_worktree_name_request(
    endpoint: &ControlEndpoint,
    request: &WorktreeNameRequest,
) -> Result<bool> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)
        .context("connecting to the Zetta process control endpoint")?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: "set_worktree_name".to_owned(),
            attention_id: request.attention_id,
            worktree_name: request.name.clone(),
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

fn control_endpoint_path(process_id: u32) -> PathBuf {
    zmux::paths::session_catalog_dir().join(format!("control-{process_id}.json"))
}

#[cfg(test)]
#[path = "tests/process_control.rs"]
mod tests;
