//! Minimal client for the two Zetta-only notification integrations.
//!
//! Notification delivery is useful in every terminal. When Zetta's inherited
//! target variables are present, this client additionally asks the owning
//! process whether the tab is silent and focuses that tab after a body click.

use std::{
    fs,
    io::{BufRead as _, BufReader, Read as _, Write as _},
    path::PathBuf,
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

#[derive(Deserialize)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    attention_id: Option<u64>,
}

#[derive(Deserialize)]
struct ControlResponse {
    status: String,
    #[serde(default)]
    silent_mode: bool,
}

pub(super) fn request_process_silent_mode(
    process_id: u32,
    attention_id: Option<u64>,
) -> Result<bool> {
    anyhow::ensure!(process_id != 0, "process ID must be positive");
    anyhow::ensure!(attention_id != Some(0), "attention ID must be positive");
    let endpoint = read_control_endpoint(process_id)?;
    let response = send_request(&endpoint, "get_silent_mode", attention_id)?;
    anyhow::ensure!(
        response.status == "ok",
        "Zetta rejected the silent-mode query"
    );
    Ok(response.silent_mode)
}

pub(super) fn request_process_focus_tab(process_id: u32, attention_id: u64) -> Result<bool> {
    anyhow::ensure!(process_id != 0, "process ID must be positive");
    anyhow::ensure!(attention_id != 0, "attention ID must be positive");
    let endpoint = read_control_endpoint(process_id)?;
    Ok(send_request(&endpoint, "focus_tab", Some(attention_id))?.status == "ok")
}

fn read_control_endpoint(process_id: u32) -> Result<ControlEndpoint> {
    let path = control_endpoint_path(process_id);
    let contents = fs::read(&path)
        .with_context(|| format!("reading Zetta process control endpoint {}", path.display()))?;
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&contents).context("parsing Zetta process control endpoint")?;
    anyhow::ensure!(
        endpoint.version == zmux::protocol::CONTROL_VERSION && endpoint.process_id == process_id,
        "Zetta process control endpoint is outdated"
    );
    Ok(endpoint)
}

fn send_request(
    endpoint: &ControlEndpoint,
    command: &str,
    attention_id: Option<u64>,
) -> Result<ControlResponse> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)
        .context("connecting to the Zetta process control endpoint")?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: command.to_owned(),
            attention_id,
        },
    )?;
    read_message(&mut stream)
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
