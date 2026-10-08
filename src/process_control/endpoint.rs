//! Discovering the control endpoints of running Zetta processes, and the
//! on-disk endpoint files and sockets they are published through.
//!
//! Every scan reaps the endpoint file and socket of a process that is gone, so
//! the catalog cannot accumulate endpoints that later requests keep trying to
//! connect to.

use super::*;

/// Every live process-control endpoint in the session catalog, in directory
/// order, having reaped the endpoint file and socket of any process that is
/// gone.
///
/// The reaping is why this is one function rather than a loop per caller:
/// every request that scans the catalog has to do it, and a caller that
/// forgets leaves a stale endpoint behind that later requests keep trying to
/// connect to. It also means a caller cannot accidentally skip the
/// `CONTROL_VERSION` check and send a request to a Zetta too old to serve it.
///
/// The endpoints are collected rather than streamed so a directory-entry error
/// still aborts the request with that error, as it did when each caller ran
/// its own loop. There is one endpoint per running Zetta process, and every
/// caller is a one-shot CLI path, so the vector costs nothing worth streaming
/// for.
pub(super) fn live_control_endpoints() -> Result<Vec<ControlEndpoint>> {
    live_control_endpoints_in(&crate::background_sessions::session_catalog_dir())
}

pub(super) fn live_control_endpoints_in(directory: &Path) -> Result<Vec<ControlEndpoint>> {
    if !control_directory_is_private(directory)? {
        return Ok(Vec::new());
    }
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).context("reading Zetta process control endpoints"),
    };
    let mut endpoints = Vec::new();
    for entry in entries {
        let path = entry?.path();
        let Some(process_id) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("control-"))
            .and_then(|name| name.strip_suffix(".json"))
            .and_then(|process_id| process_id.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(endpoint) = read_endpoint_file(&path, process_id) else {
            continue;
        };
        if !process_is_running(endpoint.process_id) {
            reap_endpoint(&path);
            continue;
        }
        endpoints.push(endpoint);
    }
    Ok(endpoints)
}

/// The control endpoint for one specific Zetta process.
///
/// Fails rather than reporting absence: a caller that names a process ID read
/// it from `ZETTA_PROCESS_ID`, so a missing endpoint means the process it was
/// told to talk to is not serving, and saying so beats doing nothing quietly.
/// Use [`live_control_endpoint`] where absence is an ordinary outcome.
pub(super) fn read_control_endpoint(process_id: u32) -> Result<ControlEndpoint> {
    let endpoint_path = control_endpoint_path(process_id);
    let directory = endpoint_path
        .parent()
        .context("control endpoint has no parent")?;
    crate::private_fs::validate_private_dir(directory).with_context(|| {
        format!(
            "checking the Zetta process control directory {}",
            directory.display()
        )
    })?;
    read_endpoint_file(&endpoint_path, process_id)
}

/// [`read_control_endpoint`] for callers that treat a process which is absent
/// or gone as "nothing to do", reaping its stale endpoint and socket on the
/// way. An outdated `CONTROL_VERSION` is still an error: the process is there,
/// it just cannot serve the request.
pub(super) fn live_control_endpoint(process_id: u32) -> Result<Option<ControlEndpoint>> {
    live_control_endpoint_in(
        &crate::background_sessions::session_catalog_dir(),
        process_id,
    )
}

pub(super) fn live_control_endpoint_in(
    directory: &Path,
    process_id: u32,
) -> Result<Option<ControlEndpoint>> {
    if !control_directory_is_private(directory)? {
        return Ok(None);
    }
    let endpoint_path = directory.join(control_endpoint_name(process_id));
    let endpoint = match read_endpoint_file(&endpoint_path, process_id) {
        Ok(endpoint) => endpoint,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    if !process_is_running(process_id) {
        reap_endpoint(&endpoint_path);
        return Ok(None);
    }
    Ok(Some(endpoint))
}

/// Whether the session directory can be trusted to hold endpoints: `false`
/// when it does not exist yet, an error when somebody else could write to it.
fn control_directory_is_private(directory: &Path) -> Result<bool> {
    match crate::private_fs::validate_private_dir(directory) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| {
            format!(
                "checking the Zetta process control directory {}",
                directory.display()
            )
        }),
    }
}

/// Reads one endpoint file, trusting it only as far as its own location
/// vouches for it: a regular file this user owns, naming the process its file
/// name names, and the socket beside it.
///
/// The socket check is what keeps an endpoint from sending a client — and the
/// command, secret or passphrase it carries — to a socket somewhere else.
fn read_endpoint_file(path: &Path, process_id: u32) -> Result<ControlEndpoint> {
    let contents =
        crate::private_fs::read_private_file(path, crate::private_fs::MAX_ENDPOINT_BYTES)
            .with_context(|| {
                format!("reading Zetta process control endpoint {}", path.display())
            })?;
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&contents).context("parsing Zetta process control endpoint")?;
    anyhow::ensure!(
        endpoint.version == CONTROL_VERSION && endpoint.process_id == process_id,
        "Zetta process control endpoint is outdated"
    );
    anyhow::ensure!(
        endpoint.socket_path == control_socket_path(path),
        "Zetta process control endpoint {} names a socket that is not its own",
        path.display()
    );
    Ok(endpoint)
}

/// Removes a dead process's endpoint and the socket beside it. The socket is
/// found from the endpoint's own name, never from its contents, and is removed
/// only if it is still a socket this user owns.
fn reap_endpoint(endpoint_path: &Path) {
    let _ = fs::remove_file(endpoint_path);
    let _ = crate::private_fs::remove_stale_socket(&control_socket_path(endpoint_path));
}

pub(super) fn random_hex(byte_count: usize) -> Result<String> {
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

pub(crate) fn config_path_identity(path: &Path) -> String {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_or_else(|_| path.to_path_buf(), |directory| directory.join(path))
    };
    let absolute = fs::canonicalize(&absolute).unwrap_or(absolute);
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    #[cfg(windows)]
    return normalized
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    #[cfg(not(windows))]
    normalized.to_string_lossy().into_owned()
}

pub(super) fn control_endpoint_path(process_id: u32) -> PathBuf {
    crate::background_sessions::session_catalog_dir().join(control_endpoint_name(process_id))
}

fn control_endpoint_name(process_id: u32) -> String {
    format!("control-{process_id}.json")
}

pub(super) fn control_socket_path(endpoint_path: &Path) -> PathBuf {
    endpoint_path.with_extension("sock")
}

/// Restricts the bound control socket to the current user. Windows places the
/// endpoint under per-user `%APPDATA%`, so only unix needs an explicit mode.
pub(super) fn restrict_socket_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).with_context(|| {
            format!(
                "restricting the Zetta process control socket {}",
                path.display()
            )
        })?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Clears the way for this process's own socket. Only a socket this user owns
/// is removed; anything else at that name is an error rather than a casualty.
pub(super) fn remove_socket_if_present(path: &Path) -> Result<()> {
    crate::private_fs::remove_stale_socket(path)
        .with_context(|| format!("removing stale socket {}", path.display()))
}

pub(super) fn write_endpoint(path: &Path, endpoint: &ControlEndpoint) -> Result<()> {
    let parent = path.parent().context("control endpoint has no parent")?;
    crate::private_fs::create_private_dir(parent)?;
    let contents = serde_json::to_vec(endpoint)?;
    crate::private_fs::write_private_file(path, &contents).with_context(|| {
        format!(
            "publishing Zetta process control endpoint {}",
            path.display()
        )
    })
}

#[cfg(test)]
#[path = "../tests/process_control/endpoint.rs"]
mod tests;
