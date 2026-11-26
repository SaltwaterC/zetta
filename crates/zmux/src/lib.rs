//! Zetta's session multiplexer.
//!
//! `zmux` owns background terminal sessions: the processes they are running,
//! the pane geometry they are arranged in, and the Argon2id verifiers that gate
//! reattaching a protected one. It is deliberately free of GPUI, of any async
//! runtime, and of anything else the terminal emulator needs but a session
//! holder does not, so it can also run on a small remote host.
//!
//! The same code is reachable two ways: as the `zmux` binary, and as
//! `zetta mux`, which forwards to [`run`].

pub mod auth;
#[cfg(feature = "session-persistence")]
pub mod auto_protect;
pub mod catalog;
pub mod headless;
pub mod logging;
pub mod paths;
#[cfg(feature = "session-persistence")]
pub mod persistence;
pub mod private_fs;
#[cfg(any(unix, windows))]
pub mod process_control;
mod process_status;
pub mod protocol;
pub mod reconnect;
pub mod relay;
pub mod remote;
mod remote_host;
pub mod retention;

mod cli;
pub mod client;
use cli::run_command;
mod daemon_spawn;
#[cfg(windows)]
mod hosted_console;
#[cfg(target_os = "macos")]
mod macos_application;
pub mod messages;
mod mux_bridge;
#[cfg(windows)]
pub mod pty_host;
pub mod secret_prompt;
pub mod server;
pub mod transport;
#[cfg(unix)]
pub mod upgrade;
#[cfg(windows)]
#[path = "upgrade_windows.rs"]
pub mod upgrade;
pub mod versions;

use std::ffi::OsString;

use anyhow::Context as _;
use anyhow::Result;

/// Set to `1` in shells launched by a Zetta process that keeps sessions local
/// instead of using the multiplexer daemon.
pub const NO_MUX_ENVIRONMENT_VARIABLE: &str = "ZETTA_NO_MUX";

use zetta_cli_help::format_help_table;

fn usage(no_mux: bool) -> String {
    let commands = if no_mux {
        format_help_table([
            ("list", "List the available background session catalogs"),
            (
                "endpoint --json",
                "Print the running daemon endpoint as JSON",
            ),
            (
                "reconnect SESSION_ID",
                "Open a backgrounded session in a Zetta window",
            ),
        ])
    } else {
        format_help_table([
            ("list", "List the sessions this multiplexer is holding"),
            (
                "profiles",
                "List the profiles available on this host, or on --ssh-target, without starting a daemon",
            ),
            (
                "create",
                "Create a headless shared session from a layout JSON file or a single profile",
            ),
            (
                "endpoint --json",
                "Print the running daemon endpoint as machine-readable JSON",
            ),
            (
                "proxy-mux [--forward-agent]",
                "Carry every daemon connection of one remote client over stdin\nand stdout, for clients using Windows OpenSSH and for every\nclient of a Windows host; with --forward-agent, link the agent\nthat SSH session forwarded for this host's shells. Run by Zetta,\nnot by hand",
            ),
            (
                "attach SSH_TARGET SESSION_ID",
                "Open a shared remote session through OpenSSH",
            ),
            (
                "relay-pane SESSION_ID PANE_ID",
                "Copy one shared pane between this host's multiplexer and this\nterminal. Zetta runs it on the far side of a Mosh session so a\nremote pane's output travels over Mosh while its control traffic\nstays on SSH; it is not something to run by hand.",
            ),
            (
                "resume SESSION",
                "Restore an encrypted disk record with fresh shells; original processes are not resumed",
            ),
            (
                "stop",
                "Stop the multiplexer. It refuses while it is holding a\nsession, because stopping it ends everything running in one;\n--force stops it anyway, ending them. Stopping a multiplexer that is not\nrunning is not an error.",
            ),
            (
                "reconnect SESSION_ID",
                "Open a backgrounded session in a Zetta window. A scoped\nsession must be shared first.",
            ),
            (
                "share SESSION_ID",
                "Make a backgrounded session joinable by every Zetta\nprocess. This changes its scope; it does not open it.",
            ),
            (
                "unshare SESSION_ID",
                "Scope a shared session back to the window that last held it",
            ),
            (
                "kill SESSION_ID",
                "End a live session and everything running in it, or discard a stale disk record",
            ),
            (
                "forget SESSION_ID",
                "Remove a live session or stale disk record from the catalog",
            ),
        ])
    };
    let options = if no_mux {
        format_help_table([
            ("-j, --json", "Print machine-readable JSON (with list)"),
            (
                "-I, --ids-only",
                "Print one numeric session ID per line (with list)",
            ),
            (
                "-H, --ssh-target TARGET",
                "OpenSSH destination for a remote list or administration command",
            ),
            ("-p, --port PORT", "SSH port for a remote command"),
            ("-h, --help", "Print help"),
            (
                "-v, --version",
                "Print the version, and the protocol it speaks",
            ),
        ])
    } else {
        format_help_table([
            (
                "-u, --upgrade",
                "Replace the running multiplexer, keeping its sessions",
            ),
            (
                "-f, --force",
                "Stop even while sessions are running (with stop)",
            ),
            ("-j, --json", "Print machine-readable JSON (with list)"),
            (
                "-I, --ids-only",
                "Print one numeric session ID per line (with list)",
            ),
            (
                "-S, --secret-stdin",
                "Read an optional create/relay secret from the first line of standard input",
            ),
            (
                "-w, --viewer-stdin",
                "Read the client ID of the window a pane is being relayed to\nfrom standard input, after the secret when one is also read\n(with relay-pane)",
            ),
            (
                "--layout PATH",
                "Create from a headless layout JSON file; use - for standard input",
            ),
            (
                "--profile NAME",
                "Create one pane using a host profile (create shorthand)",
            ),
            ("--title TEXT", "Title for a newly created headless session"),
            (
                "--working-directory PATH",
                "Starting directory for a single-pane create",
            ),
            (
                "--env KEY=VALUE",
                "Environment override for a single-pane create; may be repeated",
            ),
            (
                "-H, --ssh-target TARGET",
                "OpenSSH destination for a remote list or administration command",
            ),
            ("-p, --port PORT", "SSH port for a remote command"),
            (
                "-P, --protocol NAME",
                "What carries an attached remote session's panes: ssh (the\ndefault) or zosh. Finding and attaching the session is OpenSSH\neither way; zosh gives each pane a Mosh link of its own.",
            ),
            (
                "-k, --keep-alive",
                &format!(
                    "Hold each Zosh pane's link to a packet every {KEEP_ALIVE_DEFAULT_MS} ms\n(=MS to change, {KEEP_ALIVE_MIN_MS}-{KEEP_ALIVE_MAX_MS}); needs --protocol zosh"
                ),
            ),
            (
                "--forward-agent / --no-forward-agent",
                "Enable or disable SSH-agent forwarding for attached remote panes",
            ),
            (
                "-r, --retention MODE",
                "What to keep of a detached pane's output:\nnone, memory (default), or disk",
            ),
            (
                "-i, --identity PATH",
                "Age identity file for resuming a disk record and for\nopening a session protected by your key; may be\nrepeated. Defaults to the configured identity.",
            ),
            ("-h, --help", "Print help"),
            (
                "-v, --version",
                "Print the version, and the protocol it speaks",
            ),
        ])
    };
    format!(
        "Zetta session multiplexer\n\nUsage: zmux [COMMAND]\n       zetta mux [COMMAND]\n\nCommands:\n{commands}\n\nOptions:\n{options}"
    )
}

/// The two protocol names a remote session's panes can travel over.
///
/// Checked here so `zmux attach --protocol nonsense` fails at the command line
/// rather than in whichever window picked the request up. What each name
/// *means* is the window's business.
const REMOTE_PROTOCOL_SSH: &str = "ssh";
const REMOTE_PROTOCOL_ZOSH: &str = "zosh";

/// The interval `-k` asks for when it is given no value, and the bounds Mosh's
/// own frame interval and heartbeat set on one.
const KEEP_ALIVE_DEFAULT_MS: u64 = 500;
const KEEP_ALIVE_MIN_MS: u64 = 20;
const KEEP_ALIVE_MAX_MS: u64 = 3000;

fn parse_remote_protocol(value: &str) -> Result<String> {
    let protocol = value.trim().to_ascii_lowercase();
    anyhow::ensure!(
        protocol == REMOTE_PROTOCOL_SSH || protocol == REMOTE_PROTOCOL_ZOSH,
        "--protocol must be {REMOTE_PROTOCOL_SSH} or {REMOTE_PROTOCOL_ZOSH}"
    );
    Ok(protocol)
}

fn parse_keep_alive_interval(value: &str) -> Result<u64> {
    let interval = value
        .trim()
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("--keep-alive must be a whole number of milliseconds"))?;
    anyhow::ensure!(
        (KEEP_ALIVE_MIN_MS..=KEEP_ALIVE_MAX_MS).contains(&interval),
        "--keep-alive must be between {KEEP_ALIVE_MIN_MS} and {KEEP_ALIVE_MAX_MS} milliseconds"
    );
    Ok(interval)
}

fn parse_environment_assignment(value: &str) -> Result<(String, String)> {
    let (name, value) = value
        .split_once('=')
        .context("--env must be written as KEY=VALUE")?;
    anyhow::ensure!(!name.is_empty(), "--env variable name must not be empty");
    anyhow::ensure!(
        name.chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_'),
        "--env variable names may contain only letters, numbers, and underscores"
    );
    Ok((name.to_owned(), value.to_owned()))
}

fn no_mux_environment() -> bool {
    std::env::var(NO_MUX_ENVIRONMENT_VARIABLE).is_ok_and(|value| value == "1")
}

const SESSION_ID_HELP: &str = "SESSION_ID is either the bare numeric session ID or the stable PROCESS:RUNNER:SESSION catalog identifier printed by `zmux list`. The bare form is accepted only when the numeric ID is unambiguous; use the full form when more than one catalog contains it. `share` changes a scoped session to shared mode and offers an optional secret; press Enter on an empty prompt for no protection. Entered characters are shown as `*`, and Ctrl-C cancels the prompt. `reconnect` is the command that opens it in a Zetta window. `resume` accepts the opaque numeric record ID from disk retention; `kill` and `forget` can discard one when its daemon session is gone.";
const NO_MUX_SESSION_ID_HELP: &str = "SESSION_ID is either the bare numeric session ID or the stable PROCESS:RUNNER:SESSION catalog identifier printed by `zmux list`. The bare form is accepted only when the numeric ID is unambiguous; use the full form when more than one catalog contains it. `reconnect` is the command that opens it in a Zetta window.";

enum SessionArgument {
    Bare(u64),
    Full(catalog::SessionIdentifier),
}

impl SessionArgument {
    fn parse(value: &str) -> Result<Self> {
        if value.contains(':') {
            return Ok(Self::Full(catalog::parse_session_identifier(value)?));
        }
        Ok(Self::Bare(value.parse::<u64>().map_err(|_| {
            anyhow::anyhow!("session ID must be a positive whole number, not {value:?}")
        })?))
    }
}

impl std::fmt::Display for SessionArgument {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bare(session_id) => session_id.fmt(formatter),
            Self::Full(identifier) => identifier.fmt(formatter),
        }
    }
}

fn resolve_session_id(
    client: &client::Client,
    argument: SessionArgument,
    directory: &std::path::Path,
) -> Result<u64> {
    match argument {
        SessionArgument::Bare(session_id) => {
            let catalogs = catalog::read_session_catalogs(directory)?;
            ensure_unambiguous_session_id(&catalogs, session_id)?;
            Ok(session_id)
        }
        SessionArgument::Full(identifier) => {
            let catalogs = catalog::read_session_catalogs(directory)?;
            let found = catalogs.iter().any(|catalog| {
                catalog.process_id == identifier.process_id
                    && catalog.process_id == client.process_id()
                    && catalog.runner_id == identifier.runner_id
                    && catalog
                        .sessions
                        .iter()
                        .any(|session| session.id == identifier.session_id)
            });
            anyhow::ensure!(
                found,
                "background session {identifier} is not held by the running multiplexer"
            );
            Ok(identifier.session_id)
        }
    }
}

#[cfg(feature = "session-persistence")]
fn resolve_restorable_id(client: &client::Client, argument: SessionArgument) -> Result<u64> {
    let (_, records) = client.list_with_restorable()?;
    let id = match argument {
        SessionArgument::Bare(id) => id,
        SessionArgument::Full(identifier) => {
            anyhow::bail!(
                "disk session records use their opaque numeric ID; use `resume {}`",
                identifier.session_id
            )
        }
    };
    anyhow::ensure!(
        records
            .iter()
            .any(|record| record.id == id && record.restorable),
        "disk session record {id} is not restorable"
    );
    Ok(id)
}

#[cfg(feature = "session-persistence")]
fn resolve_forget_id(argument: SessionArgument, directory: &std::path::Path) -> Result<u64> {
    match argument {
        SessionArgument::Bare(id) => {
            let catalogs = catalog::read_session_catalogs(directory)?;
            if catalogs
                .iter()
                .flat_map(|catalog| catalog.sessions.iter())
                .any(|session| session.id == id)
            {
                ensure_unambiguous_session_id(&catalogs, id)?;
                return Ok(id);
            }
            anyhow::ensure!(
                persistence::read_opaque_records(directory)?
                    .iter()
                    .any(|record| record.id == id),
                "session {id} does not exist"
            );
            Ok(id)
        }
        SessionArgument::Full(identifier) => {
            let catalogs = catalog::read_session_catalogs(directory)?;
            anyhow::ensure!(
                catalogs.iter().any(|catalog| {
                    catalog.process_id == identifier.process_id
                        && catalog.runner_id == identifier.runner_id
                        && catalog
                            .sessions
                            .iter()
                            .any(|session| session.id == identifier.session_id)
                }),
                "background session {identifier} is not held by a live catalog"
            );
            Ok(identifier.session_id)
        }
    }
}

#[cfg(feature = "session-persistence")]
fn is_missing_session_error(error: &anyhow::Error, session_id: u64) -> bool {
    let expected = format!("session {session_id} does not exist");
    error.chain().any(|cause| cause.to_string() == expected)
}

fn ensure_unambiguous_session_id(
    catalogs: &[protocol::BackgroundSessionCatalog],
    session_id: u64,
) -> Result<()> {
    let matches = catalogs
        .iter()
        .flat_map(|catalog| {
            catalog
                .sessions
                .iter()
                .filter(move |session| session.id == session_id)
        })
        .count();
    anyhow::ensure!(
        matches <= 1,
        "session ID {session_id} is ambiguous; use the full PROCESS:RUNNER:SESSION ID"
    );
    Ok(())
}

fn print_remote_sessions(sessions: &[protocol::BackgroundSessionSummary]) {
    if sessions.is_empty() {
        println!("No remote sessions.");
        return;
    }
    for line in remote_session_lines(sessions) {
        println!("{line}");
    }
}

/// One line per session. Titles come from the remote host, so they are escaped
/// like the local catalog's before they reach the terminal.
fn remote_session_lines(sessions: &[protocol::BackgroundSessionSummary]) -> Vec<String> {
    sessions
        .iter()
        .map(|session| {
            let protected = if session.authentication_required {
                ", protected"
            } else {
                ""
            };
            format!(
                "{}  {}  ({} pane{}{})",
                session.id,
                catalog::display_text(&session.title),
                session.panes.len(),
                if session.panes.len() == 1 { "" } else { "s" },
                protected,
            )
        })
        .collect()
}

fn print_profiles(profiles: &[String], json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string(profiles)?);
    } else if profiles.is_empty() {
        println!("No profiles available.");
    } else {
        for profile in profiles {
            println!("{profile}");
        }
    }
    Ok(())
}

fn run_profiles(remote_target: Option<&str>, port: Option<u16>, json: bool) -> Result<()> {
    let profiles = if let Some(target) = remote_target {
        remote::RemoteTransport::for_creation(remote::RemoteTarget::new(target).with_port(port))?
            .query_profiles()?
    } else {
        zetta_profiles::profiles(&paths::platform_config_dir().join("config.json"))
            .into_iter()
            .map(|profile| profile.name)
            .collect()
    };
    print_profiles(&profiles, json)
}

struct CreateOptions<'a> {
    layout_path: Option<std::path::PathBuf>,
    profile: Option<String>,
    title: Option<String>,
    working_directory: Option<std::path::PathBuf>,
    environment: std::collections::HashMap<String, String>,
    secret_from_stdin: bool,
    remote_target: Option<&'a str>,
    port: Option<u16>,
    retention: retention::Retention,
    json: bool,
}

fn run_create(options: CreateOptions<'_>) -> Result<()> {
    let CreateOptions {
        layout_path,
        profile,
        title,
        working_directory,
        environment,
        secret_from_stdin,
        remote_target,
        port,
        retention,
        json,
    } = options;
    let mut spec = if let Some(path) = layout_path {
        headless::parse_path(&path)?
    } else {
        headless::single_pane(
            title.clone().unwrap_or_default(),
            profile,
            working_directory,
            environment,
        )
    };
    if let Some(title) = title {
        spec.title = title;
    }
    let secret = if secret_from_stdin {
        secret_prompt::read_optional_secret_from_stdin()?
    } else {
        secret_prompt::prompt_for_optional_secret()?
    };
    let protection = secret
        .as_ref()
        .map(|secret| auth::SessionAuthentication::create(secret.expose()))
        .transpose()?;
    let client = if let Some(target) = remote_target {
        client::Client::connect_remote_for_creation(
            remote::RemoteTarget::new(target).with_port(port),
        )?
    } else {
        client::Client::connect_with_retention(retention)?
    };
    let request = spec.request(
        client.next_shared_operation_id(),
        protection
            .as_ref()
            .map(|protection| protection.verifier().to_owned()),
    );
    let created = client.create_shared(request)?;
    if json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "session_id": created.session_id,
                "summary": created.state.summary,
            }))?
        );
    } else {
        println!("Created headless session {}.", created.session_id);
    }
    Ok(())
}

/// Client-side settings that only the caller can know.
///
/// `zmux` is deliberately free of Zetta's configuration format — a session
/// holder on a small remote host has no business parsing it — but the identity
/// a user configured once should not have to be retyped as `--identity` on every
/// command. So the binary built beside Zetta passes what it has already parsed,
/// falling back to the conventional SSH identity when needed; the standalone
/// binary applies the same fallback without reading Zetta's configuration.
#[derive(Clone, Debug, Default)]
pub struct ClientDefaults {
    /// Age identity files, as `--identity` would have given them. Used to open a
    /// session protected by the user's key, and to decrypt a disk record.
    pub identity_paths: Vec<std::path::PathBuf>,
}

/// The entry point shared by the `zmux` binary and `zetta mux`.
pub fn run(arguments: &[OsString]) -> Result<()> {
    run_with_defaults(arguments, ClientDefaults::default())
}

/// [`run`], with the client-side settings the caller has already resolved.
///
/// The error comes back already escaped for a terminal, because every caller
/// prints it and some of its text is not ours: SSH's stderr carries the remote
/// command's, and a remote daemon's refusals are relayed verbatim. Escaping it
/// here, rather than where each such text enters an error, also covers the
/// sources nobody has thought of yet.
pub fn run_with_defaults(arguments: &[OsString], defaults: ClientDefaults) -> Result<()> {
    #[cfg(target_os = "macos")]
    macos_application::host_daemon(arguments)
        .map_err(|error| anyhow::anyhow!(display_error(&error)))?;
    run_command(arguments, defaults).map_err(|error| anyhow::anyhow!(display_error(&error)))
}

/// An error as `{error:#}` prints it, with every control character escaped but
/// the line breaks, which SSH's own multi-line diagnostics need to stay
/// readable. A line feed alone cannot reach back over what is already printed.
fn display_error(error: &anyhow::Error) -> String {
    format!("{error:#}")
        .lines()
        .map(catalog::display_text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// What stopping the multiplexer turned out to involve.
#[derive(Debug, PartialEq, Eq)]
pub enum StopOutcome {
    /// There was no daemon or Windows pseudoconsole host to stop, which is not
    /// a failure: the request was that the multiplexer not be running, and it
    /// is not.
    NotRunning,
    /// Asked to stop, and it left.
    Stopped,
    /// Ended by signal, because it could not be asked to leave: it was holding
    /// sessions, or it speaks a protocol this build cannot talk to. Either way
    /// everything it was holding ended with it.
    Signalled { process_id: u32 },
}

/// How long the multiplexer is given to stop answering before this gives up.
const STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[cfg(windows)]
fn stop_pty_host(directory: &std::path::Path, force: bool) -> Result<bool> {
    pty_host::stop(directory, force)
}

#[cfg(not(windows))]
fn stop_pty_host(_directory: &std::path::Path, _force: bool) -> Result<bool> {
    Ok(false)
}

/// How long a signalled multiplexer is given to leave before it is killed.
#[cfg(unix)]
const STOP_GRACE_PERIOD: std::time::Duration = std::time::Duration::from_secs(2);

/// Stops the multiplexer holding sessions in `directory`.
///
/// The alternative is signalling a process found by name, which is what this
/// exists to replace: `pkill zmux` matches every multiplexer this user is
/// running — another test's, another checkout's — and ends the sessions of all
/// of them without saying so.
///
/// The default is therefore a refusal: the daemon owns the terminals, so
/// stopping it ends every process running in them, and it says how many rather
/// than doing that quietly. `force` is the caller saying they meant it.
///
/// A multiplexer speaking a protocol this build does not is the case with no
/// polite answer — it cannot be asked what it is holding, let alone asked to
/// leave — and it is the case a rebuild creates, so `force` covers it too. The
/// process id comes from the endpoint the daemon published for itself, and only
/// after its socket has answered, so this signals the multiplexer for *this*
/// directory and nothing else.
pub fn stop(directory: &std::path::Path, force: bool) -> Result<StopOutcome> {
    use anyhow::Context as _;

    let Ok(endpoint) = transport::Endpoint::read(&server::endpoint_path(directory)) else {
        return Ok(if stop_pty_host(directory, force)? {
            StopOutcome::Stopped
        } else {
            StopOutcome::NotRunning
        });
    };
    // The endpoint file outlives the process that wrote it, so answering is
    // what "running" means here, not the file being there.
    if transport::Stream::connect(&endpoint.socket_path).is_err() {
        return Ok(if stop_pty_host(directory, force)? {
            StopOutcome::Stopped
        } else {
            StopOutcome::NotRunning
        });
    }

    let asked = if endpoint.protocol_version == messages::PROTOCOL_VERSION {
        match client::Client::connect_existing_at(directory)? {
            Some(client) => client.shutdown(),
            None => {
                return Ok(if stop_pty_host(directory, force)? {
                    StopOutcome::Stopped
                } else {
                    StopOutcome::NotRunning
                });
            }
        }
    } else {
        Err(anyhow::anyhow!(
            "the multiplexer running as process {} speaks protocol version {}, not {}, so it \
             cannot be asked to stop",
            endpoint.process_id,
            endpoint.protocol_version,
            messages::PROTOCOL_VERSION,
        ))
    };

    match asked {
        Ok(()) => {
            wait_until_stopped(&endpoint, STOP_TIMEOUT).with_context(|| {
                format!(
                    "the multiplexer (process {}) agreed to stop and did not",
                    endpoint.process_id
                )
            })?;
            stop_pty_host(directory, false).context("stopping the Windows pseudoconsole host")?;
            Ok(StopOutcome::Stopped)
        }
        Err(refused) if !force => Err(refused.context(
            "the multiplexer was left running; rerun with --force to stop it anyway, ending \
             everything it is holding",
        )),
        Err(_) => {
            signal_until_stopped(&endpoint)?;
            stop_pty_host(directory, true).context("stopping the Windows pseudoconsole host")?;
            Ok(StopOutcome::Signalled {
                process_id: endpoint.process_id,
            })
        }
    }
}

/// Ends a multiplexer that will not, or cannot, end itself.
///
/// `SIGTERM` first: a process given the chance to leave takes its pty
/// descriptors with it, and its sessions' shells see a hangup rather than
/// nothing at all. `SIGKILL` only once it has had that chance and not taken it.
#[cfg(unix)]
fn signal_until_stopped(endpoint: &transport::Endpoint) -> Result<()> {
    use anyhow::Context as _;

    use rustix::process::{Pid, Signal, kill_process};

    let process_id = i32::try_from(endpoint.process_id)
        .ok()
        .and_then(Pid::from_raw)
        .context("the multiplexer published an unusable process id")?;
    // A signal to one process id, published by the multiplexer for itself,
    // whose socket answered a moment ago.
    let _ = kill_process(process_id, Signal::TERM);
    if wait_until_stopped(endpoint, STOP_GRACE_PERIOD).is_ok() {
        return Ok(());
    }
    let _ = kill_process(process_id, Signal::KILL);
    wait_until_stopped(endpoint, STOP_TIMEOUT).with_context(|| {
        format!(
            "the multiplexer (process {}) could not be stopped",
            endpoint.process_id
        )
    })
}

#[cfg(windows)]
fn signal_until_stopped(endpoint: &transport::Endpoint) -> Result<()> {
    use anyhow::Context as _;

    terminate_process(endpoint.process_id)
        .with_context(|| format!("terminating multiplexer process {}", endpoint.process_id))?;
    wait_until_stopped(endpoint, STOP_TIMEOUT).with_context(|| {
        format!(
            "the multiplexer (process {}) could not be stopped",
            endpoint.process_id
        )
    })
}

#[cfg(windows)]
pub(crate) fn terminate_process(process_id: u32) -> Result<()> {
    use anyhow::Context as _;
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};

    anyhow::ensure!(
        process_id > 0,
        "the process published an unusable process id"
    );
    // SAFETY: the process ID came from a private endpoint that answered just
    // before this function was called; the access right is valid.
    let opened = unsafe { OpenProcess(PROCESS_TERMINATE, false, process_id) }
        .with_context(|| format!("opening process {process_id} for termination"))?;
    // SAFETY: `OpenProcess` succeeded, so this function owns the handle.
    let process = unsafe { OwnedHandle::from_raw_handle(opened.0) };
    // SAFETY: `process` is open with `PROCESS_TERMINATE` until it drops.
    unsafe { TerminateProcess(HANDLE(process.as_raw_handle()), 1) }
        .with_context(|| format!("terminating process {process_id}"))
}

/// Waits for the multiplexer's socket to stop answering.
///
/// A reply to a shutdown says the request arrived, not that the process left;
/// only its socket going quiet says that, and reporting "stopped" without
/// checking would be a guess.
fn wait_until_stopped(endpoint: &transport::Endpoint, timeout: std::time::Duration) -> Result<()> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if transport::Stream::connect(&endpoint.socket_path).is_err() {
            return Ok(());
        }
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "it is still answering on {}",
            endpoint.socket_path.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Whether the multiplexer already has a secret for this session.
///
/// Asked so that sharing an already-protected session does not prompt for a new
/// one: the secret it has is the secret its owner chose, and replacing it
/// silently would lock out whoever knows it.
fn session_is_protected(client: &client::Client, session_id: u64) -> Result<bool> {
    Ok(client
        .list()?
        .iter()
        .find(|session| session.id == session_id)
        .is_some_and(|session| session.authentication_required))
}

trait RequiredSession {
    type Session;

    fn context_missing(self) -> Result<Self::Session>;
}

impl<T> RequiredSession for Option<T> {
    type Session = T;

    fn context_missing(self) -> Result<T> {
        self.ok_or_else(|| anyhow::anyhow!("this command requires a session ID"))
    }
}

#[cfg(test)]
#[path = "tests/lib.rs"]
mod tests;
