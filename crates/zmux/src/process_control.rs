//! The client half of a Zetta window's process control endpoint.
//!
//! Each Zetta process publishes `control-PID.json` in the session catalog
//! directory, naming a socket beside it and the token a request must carry.
//! `zmux reconnect`, `zntfy` and `zwt` all talk to that socket; this module is
//! the one reader of the endpoint file and the one framing of a request, so the
//! checks below apply to every caller rather than to whichever copy was
//! hardened last.

use std::{
    io::{BufRead as _, BufReader, Read as _, Write as _},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use zeroize::Zeroizing;

use crate::{paths, private_fs, protocol::CONTROL_VERSION};

#[cfg(unix)]
type ControlStream = std::os::unix::net::UnixStream;
#[cfg(windows)]
type ControlStream = uds_windows::UnixStream;

const MAX_CONTROL_MESSAGE_BYTES: usize = 256 * 1024;

/// A running Zetta process's control endpoint, as its file published it.
#[derive(Clone, Deserialize)]
pub struct ControlEndpoint {
    version: u32,
    process_id: u32,
    socket_path: PathBuf,
    token: String,
}

impl ControlEndpoint {
    /// The Zetta process that published this endpoint.
    pub fn process_id(&self) -> u32 {
        self.process_id
    }
}

/// The reply every control command carries; commands that answer more than a
/// status deserialize their own response type instead.
#[derive(Deserialize)]
pub struct ControlResponse {
    pub status: String,
}

#[derive(Serialize)]
struct Envelope<'a, F> {
    token: &'a str,
    command: &'a str,
    #[serde(flatten)]
    fields: &'a F,
}

/// Where Zetta process `process_id` publishes its control endpoint.
pub fn control_endpoint_path(process_id: u32) -> PathBuf {
    paths::session_catalog_dir().join(format!("control-{process_id}.json"))
}

/// The endpoint Zetta process `process_id` published, after checking that the
/// directory holding it is private.
pub fn endpoint_for_process(process_id: u32) -> Result<ControlEndpoint> {
    anyhow::ensure!(process_id != 0, "process ID must be positive");
    let directory = paths::session_catalog_dir();
    private_fs::validate_private_dir(&directory)
        .with_context(|| format!("checking Zetta's session directory {}", directory.display()))?;
    read_control_endpoint(&control_endpoint_path(process_id), process_id)
}

/// The process a `control-PID.json` file name says published it.
pub(crate) fn control_endpoint_process_id(path: &Path) -> Option<u32> {
    path.file_name()?
        .to_str()?
        .strip_prefix("control-")?
        .strip_suffix(".json")?
        .parse()
        .ok()
}

/// Reads the control endpoint Zetta process `process_id` published at `path`.
///
/// The file is trusted on the same terms as the window's own reader in
/// `process_control/endpoint.rs`: a private regular file of bounded size, from
/// the process its name says, naming the socket beside it. A request can carry
/// a session secret, so a socket path the file's contents chose is never used.
pub fn read_control_endpoint(path: &Path, process_id: u32) -> Result<ControlEndpoint> {
    let contents = private_fs::read_private_file(path, private_fs::MAX_ENDPOINT_BYTES)
        .with_context(|| format!("reading Zetta process control endpoint {}", path.display()))?;
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&contents).context("parsing Zetta process control endpoint")?;
    anyhow::ensure!(
        endpoint.version == CONTROL_VERSION && endpoint.process_id == process_id,
        "Zetta process control endpoint is outdated"
    );
    anyhow::ensure!(
        endpoint.socket_path == path.with_extension("sock"),
        "Zetta process control endpoint {} names a socket that is not its own",
        path.display()
    );
    Ok(endpoint)
}

/// Every control endpoint in `directory` whose process is still running,
/// after checking that the directory itself is private.
pub(crate) fn running_control_endpoints(directory: &Path) -> std::io::Result<Vec<ControlEndpoint>> {
    private_fs::validate_private_dir(directory)?;
    Ok(std::fs::read_dir(directory)?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let process_id = control_endpoint_process_id(&path)?;
            read_control_endpoint(&path, process_id).ok()
        })
        .filter(|endpoint| crate::process_status::is_running(endpoint.process_id))
        .collect())
}

/// Sends `command` with `fields` beside it to `endpoint` and reads one reply.
///
/// `fields` is flattened into the request object next to the token and the
/// command name, so a caller describes only what its command adds.
pub fn request<R: DeserializeOwned>(
    endpoint: &ControlEndpoint,
    command: &str,
    fields: &impl Serialize,
    timeout: Duration,
) -> Result<R> {
    let mut stream = ControlStream::connect(&endpoint.socket_path)
        .context("connecting to the Zetta process control endpoint")?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    write_message(
        &mut stream,
        &Envelope {
            token: &endpoint.token,
            command,
            fields,
        },
    )?;
    read_message(&mut stream)
}

fn read_message<T: DeserializeOwned>(stream: &mut ControlStream) -> Result<T> {
    let mut bytes = Zeroizing::new(Vec::new());
    let mut reader = BufReader::new(&mut *stream).take((MAX_CONTROL_MESSAGE_BYTES + 1) as u64);
    reader.read_until(b'\n', &mut bytes)?;
    anyhow::ensure!(
        bytes.last() == Some(&b'\n'),
        "process control message is too long or incomplete"
    );
    bytes.pop();
    serde_json::from_slice(&bytes).context("parsing process control message")
}

fn write_message(stream: &mut ControlStream, message: &impl Serialize) -> Result<()> {
    // The request can carry a session secret, so it is serialized into a
    // buffer that is wiped afterwards rather than streamed in pieces.
    let bytes = Zeroizing::new(serde_json::to_vec(message)?);
    stream.write_all(&bytes)?;
    stream.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/process_control.rs"]
mod tests;
