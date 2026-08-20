//! Reconnect a catalogued session in a Zetta window.
//!
//! The session catalog belongs to `zmux`, but the window that displays a
//! session belongs to Zetta. Reconnect therefore reads the catalog here and
//! sends the final request over Zetta's local process-control socket. Keeping
//! this client in the shared crate makes `zmux reconnect` and `zetta mux
//! reconnect` identical, including their routing when the command is run from
//! inside a Zetta terminal.

use std::{env, fs, path::PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::{
    auth::SessionSecret,
    catalog::{parse_session_identifier, read_session_catalogs},
    paths,
    protocol::{BackgroundSessionCatalog, CONTROL_VERSION},
};

#[derive(Debug)]
struct SessionTarget {
    process_id: u32,
    runner_id: u64,
    session_id: u64,
    authentication_required: bool,
    /// Whether the multiplexer holds this session rather than a Zetta
    /// process. The published process is then the multiplexer's, which has no
    /// Zetta control endpoint to receive a reconnect request.
    multiplexer_held: bool,
    /// The Zetta process a backgrounded session is scoped to, if it is not
    /// shared. Only that window may attach it.
    scoped_to: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ReconnectOrigin {
    process_id: u32,
    attention_id: u64,
}

/// The outcome returned by a Zetta process-control endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReconnectSessionResult {
    Reconnected,
    AuthenticationFailed,
    SessionNotFound,
    StillStarting,
    Rejected,
}

/// Opens a catalogued session in a Zetta window.
pub fn run_reconnect_session(identifier: &str) -> Result<()> {
    #[cfg(any(unix, windows))]
    {
        run_reconnect_session_supported(identifier)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = identifier;
        anyhow::bail!("session reconnect is not supported on this platform");
    }
}

#[cfg(any(unix, windows))]
fn run_reconnect_session_supported(identifier: &str) -> Result<()> {
    let catalogs = read_session_catalogs(&paths::session_catalog_dir())?;
    let target = find_session(&catalogs, identifier)?;
    let origin = reconnect_origin();
    let secret = target
        .authentication_required
        .then(crate::secret_prompt::prompt_for_reconnect_secret)
        .transpose()?;
    // A session the multiplexer holds is published under its process, which
    // has no Zetta control endpoint. Route through the originating Zetta
    // process when this command came from a Zetta terminal; asking the
    // multiplexer's own identifier would fail with a missing endpoint file.
    let result = if target.multiplexer_held {
        request_multiplexer_reconnect(
            target.session_id,
            target.scoped_to,
            origin.map(|origin| (origin.process_id, origin.attention_id)),
            secret,
        )?
    } else {
        request_reconnect_session(
            target.process_id,
            target.runner_id,
            target.session_id,
            origin
                .filter(|origin| origin.process_id == target.process_id)
                .map(|origin| origin.attention_id),
            secret,
        )?
    };
    match result {
        ReconnectSessionResult::Reconnected => {
            println!("Reconnected session {identifier}.");
            Ok(())
        }
        ReconnectSessionResult::AuthenticationFailed => anyhow::bail!(
            "could not reconnect session {identifier:?}: the session secret was incorrect"
        ),
        ReconnectSessionResult::SessionNotFound => anyhow::bail!(
            "could not reconnect session {identifier:?}: the session no longer exists"
        ),
        ReconnectSessionResult::StillStarting => anyhow::bail!(
            "could not reconnect session {identifier:?}: the session is still starting; try again shortly"
        ),
        ReconnectSessionResult::Rejected => anyhow::bail!(
            "could not reconnect session {identifier:?}: Zetta rejected the reconnect request"
        ),
    }
}

fn find_session(catalogs: &[BackgroundSessionCatalog], identifier: &str) -> Result<SessionTarget> {
    let target = if identifier.contains(':') {
        let identifier = parse_session_identifier(identifier)?;
        catalogs.iter().find_map(|catalog| {
            (catalog.process_id == identifier.process_id
                && catalog.runner_id == identifier.runner_id)
                .then(|| {
                    catalog
                        .sessions
                        .iter()
                        .find(|session| session.id == identifier.session_id)
                })
                .flatten()
                .map(|session| SessionTarget {
                    process_id: identifier.process_id,
                    runner_id: identifier.runner_id,
                    session_id: identifier.session_id,
                    authentication_required: session.authentication_required,
                    multiplexer_held: !process_is_zetta(identifier.process_id),
                    scoped_to: session.scoped_to,
                })
        })
    } else {
        let session_id = identifier
            .parse::<u64>()
            .context("session ID must be PROCESS:RUNNER:SESSION")?;
        let matches = catalogs
            .iter()
            .flat_map(|catalog| {
                catalog.sessions.iter().filter_map(|session| {
                    (session.id == session_id).then_some(SessionTarget {
                        process_id: catalog.process_id,
                        runner_id: catalog.runner_id,
                        session_id,
                        authentication_required: session.authentication_required,
                        multiplexer_held: !process_is_zetta(catalog.process_id),
                        scoped_to: session.scoped_to,
                    })
                })
            })
            .collect::<Vec<_>>();
        anyhow::ensure!(
            !matches.is_empty(),
            "background session {identifier:?} was not found"
        );
        anyhow::ensure!(
            matches.len() == 1,
            "session ID {identifier:?} is ambiguous; use the full PROCESS:RUNNER:SESSION ID"
        );
        Some(matches.into_iter().next().expect("one match was checked"))
    };
    target.with_context(|| format!("background session {identifier:?} was not found"))
}

fn reconnect_origin() -> Option<ReconnectOrigin> {
    parse_reconnect_origin(
        &env::var("ZETTA_PROCESS_ID").ok()?,
        &env::var("ZETTA_ATTENTION_ID").ok()?,
    )
}

fn parse_reconnect_origin(process_id: &str, attention_id: &str) -> Option<ReconnectOrigin> {
    let process_id = process_id.parse().ok()?;
    let attention_id = attention_id.parse().ok()?;
    (process_id != 0 && attention_id != 0).then_some(ReconnectOrigin {
        process_id,
        attention_id,
    })
}

fn process_is_zetta(process_id: u32) -> bool {
    control_endpoint_path(process_id).is_file()
}

#[cfg(any(unix, windows))]
fn request_reconnect_session(
    process_id: u32,
    runner_id: u64,
    session_id: u64,
    attention_id: Option<u64>,
    secret: Option<SessionSecret>,
) -> Result<ReconnectSessionResult> {
    let endpoint_path = control_endpoint_path(process_id);
    let contents = fs::read(&endpoint_path).with_context(|| {
        format!(
            "reading Zetta process control endpoint {}",
            endpoint_path.display()
        )
    })?;
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&contents).context("parsing Zetta process control endpoint")?;
    anyhow::ensure!(
        endpoint.version == CONTROL_VERSION && endpoint.process_id == process_id,
        "Zetta process control endpoint is outdated"
    );
    send_reconnect_session_request(&endpoint, runner_id, session_id, attention_id, secret)
}

/// Asks a running Zetta process to attach a session held by the multiplexer.
///
/// A session's catalog process is the daemon, not a Zetta process, so the
/// command must choose a window endpoint. An invocation from a Zetta terminal
/// prefers that process; an external invocation tries every current window.
#[cfg(any(unix, windows))]
fn request_multiplexer_reconnect(
    session_id: u64,
    scoped_to: Option<u32>,
    origin: Option<(u32, u64)>,
    secret: Option<SessionSecret>,
) -> Result<ReconnectSessionResult> {
    let directory = paths::session_catalog_dir();
    let entries = fs::read_dir(&directory)
        .with_context(|| format!("looking for a Zetta window in {}", directory.display()))?;

    let mut endpoints = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_control = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("control-") && name.ends_with(".json"));
        if !is_control {
            continue;
        }
        let Ok(contents) = fs::read(&path) else {
            continue;
        };
        let Ok(endpoint) = serde_json::from_slice::<ControlEndpoint>(&contents) else {
            continue;
        };
        if endpoint.version == CONTROL_VERSION && process_is_running(endpoint.process_id) {
            endpoints.push(endpoint);
        }
    }
    anyhow::ensure!(
        !endpoints.is_empty(),
        "no running Zetta window can attach a multiplexer session. Any window still running an \
         older Zetta cannot: restart Zetta, or install the current build, and try again."
    );
    // A private backgrounded session belongs to its owner. Sharing changes
    // that state; reconnect is the separate action that opens the session.
    if let Some(owner) = scoped_to {
        endpoints.retain(|endpoint| endpoint.process_id == owner);
        anyhow::ensure!(
            !endpoints.is_empty(),
            "session {session_id} is scoped to Zetta process {owner}, which is not running. Run \
             `zmux share {session_id}` to make it shared, then `zmux reconnect {session_id}` to \
             open it from another window."
        );
    } else if let Some((origin_process, _)) = origin {
        endpoints.retain(|endpoint| endpoint.process_id == origin_process);
        anyhow::ensure!(
            !endpoints.is_empty(),
            "the Zetta process that ran `zmux reconnect` is no longer running"
        );
    }

    let mut last_error = None;
    for endpoint in &endpoints {
        let attention_id = origin.and_then(|(process_id, attention_id)| {
            (endpoint.process_id == process_id).then_some(attention_id)
        });
        match send_reconnect_session_request(endpoint, 0, session_id, attention_id, secret.clone())
        {
            Ok(result) => return Ok(result),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("no Zetta window accepted the session")))
}

#[cfg(any(unix, windows))]
fn send_reconnect_session_request(
    endpoint: &ControlEndpoint,
    runner_id: u64,
    session_id: u64,
    attention_id: Option<u64>,
    secret: Option<SessionSecret>,
) -> Result<ReconnectSessionResult> {
    let mut stream = ControlStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    let mut request = ControlRequest {
        token: endpoint.token.clone(),
        command: "reconnect_session".to_owned(),
        runner_id: Some(runner_id),
        session_id: Some(session_id),
        secret: secret.as_ref().map(|secret| secret.expose().to_owned()),
        icon: None,
        pane_theme: None,
        pane_overlay: None,
        pane_overlay_font_size: None,
        pane_overlay_opacity: None,
        pane_overlay_color: None,
        attention_id,
        attention_summary: None,
        attention_body: None,
        tab_name: None,
        worktree_name: None,
        config_path: None,
        split: None,
        profile: None,
        theme: None,
        pane_request: None,
    };
    let result = write_message(&mut stream, &request).and_then(|()| {
        let response = read_message::<ControlResponse>(&mut stream)?;
        Ok(match response.status.as_str() {
            "ok" => ReconnectSessionResult::Reconnected,
            "authentication_failed" => ReconnectSessionResult::AuthenticationFailed,
            "session_not_found" => ReconnectSessionResult::SessionNotFound,
            "session_starting" => ReconnectSessionResult::StillStarting,
            _ => ReconnectSessionResult::Rejected,
        })
    });
    if let Some(secret) = request.secret.as_mut() {
        use zeroize::Zeroize as _;
        secret.zeroize();
    }
    result
}

#[cfg(any(unix, windows))]
fn read_message<T: serde::de::DeserializeOwned>(stream: &mut ControlStream) -> Result<T> {
    use std::io::{BufRead as _, BufReader, Read as _};
    use zeroize::Zeroizing;

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

#[cfg(any(unix, windows))]
fn write_message(stream: &mut ControlStream, message: &impl serde::Serialize) -> Result<()> {
    use std::io::Write as _;

    serde_json::to_writer(&mut *stream, message)?;
    stream.write_all(b"\n")?;
    Ok(())
}

fn control_endpoint_path(process_id: u32) -> PathBuf {
    paths::session_catalog_dir().join(format!("control-{process_id}.json"))
}

#[cfg(any(unix, windows))]
fn process_is_running(process_id: u32) -> bool {
    use sysinfo::{Pid, ProcessesToUpdate, System};

    let process_id = Pid::from_u32(process_id);
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[process_id]), true);
    system.process(process_id).is_some()
}

#[cfg(any(unix, windows))]
type ControlStream = platform::Stream;

#[cfg(any(unix, windows))]
mod platform {
    #[cfg(unix)]
    pub(super) type Stream = std::os::unix::net::UnixStream;
    #[cfg(windows)]
    pub(super) type Stream = uds_windows::UnixStream;
}

#[cfg(any(unix, windows))]
const CONTROL_CLIENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(16);
#[cfg(any(unix, windows))]
const MAX_CONTROL_MESSAGE_BYTES: usize = 256 * 1024;

#[derive(Clone, Deserialize)]
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
    runner_id: Option<u64>,
    session_id: Option<u64>,
    secret: Option<String>,
    icon: Option<String>,
    pane_theme: Option<String>,
    pane_overlay: Option<String>,
    pane_overlay_font_size: Option<String>,
    pane_overlay_opacity: Option<u8>,
    pane_overlay_color: Option<String>,
    attention_id: Option<u64>,
    attention_summary: Option<String>,
    attention_body: Option<String>,
    tab_name: Option<String>,
    worktree_name: Option<String>,
    config_path: Option<String>,
    split: Option<String>,
    profile: Option<String>,
    theme: Option<String>,
    pane_request: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ControlResponse {
    status: String,
}

#[cfg(test)]
#[path = "tests/reconnect.rs"]
mod tests;
