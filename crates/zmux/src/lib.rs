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
pub mod paths;
#[cfg(feature = "session-persistence")]
pub mod persistence;
pub mod protocol;
pub mod reconnect;
pub mod retention;

pub mod client;
pub mod messages;
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

use std::ffi::OsString;

#[cfg(feature = "session-persistence")]
use anyhow::Context as _;
use anyhow::Result;

/// Set to `1` in shells launched by a Zetta process that keeps sessions local
/// instead of using the multiplexer daemon.
pub const NO_MUX_ENVIRONMENT_VARIABLE: &str = "ZETTA_NO_MUX";

fn format_help_table<'a>(rows: impl AsRef<[(&'a str, &'a str)]>) -> String {
    let rows = rows
        .as_ref()
        .iter()
        .map(|&(label, description)| (label.trim_end(), description))
        .collect::<Vec<_>>();
    let label_width = rows
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(0);
    rows.into_iter()
        .map(|(label, description)| {
            let mut lines = description.split('\n');
            let first_line = lines.next().unwrap_or("").trim_end();
            let mut formatted = String::new();
            formatted.push_str("  ");
            formatted.push_str(label);
            formatted.push_str(&" ".repeat(label_width - label.chars().count()));
            if !first_line.is_empty() {
                formatted.push_str("  ");
                formatted.push_str(first_line);
            }
            for line in lines {
                formatted.push('\n');
                let line = line.trim_end();
                if !line.is_empty() {
                    formatted.push_str("  ");
                    formatted.push_str(&" ".repeat(label_width));
                    formatted.push_str("  ");
                    formatted.push_str(line);
                }
            }
            formatted
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn usage(no_mux: bool) -> String {
    let commands = if no_mux {
        format_help_table([
            ("list", "List the available background session catalogs"),
            (
                "reconnect SESSION_ID",
                "Open a backgrounded session in a Zetta window",
            ),
        ])
    } else {
        format_help_table([
            ("list", "List the sessions this multiplexer is holding"),
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

/// Client-side settings that only the caller can know.
///
/// `zmux` is deliberately free of Zetta's configuration format — a session
/// holder on a small remote host has no business parsing it — but the identity
/// a user configured once should not have to be retyped as `--identity` on every
/// command. So the binaries built beside Zetta pass what they have already
/// parsed, and the standalone one passes nothing.
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
pub fn run_with_defaults(arguments: &[OsString], defaults: ClientDefaults) -> Result<()> {
    let no_mux = no_mux_environment();
    let mut json = false;
    let mut daemon = false;
    let mut retention = retention::Retention::default();
    let mut command: Option<String> = None;
    let mut session: Option<SessionArgument> = None;
    let mut expect_retention = false;
    let mut expect_retention_bytes = false;
    let mut retention_bytes = None;
    #[cfg(unix)]
    let mut resume_from: Option<i32> = None;
    #[cfg(windows)]
    let mut resume_from: Option<std::path::PathBuf> = None;
    #[cfg(feature = "session-persistence")]
    let mut daemon_options = None;
    // Explicit paths append to the configured ones rather than replacing them,
    // which is how `age -i` composes and what lets a second key be tried without
    // having to restate the first.
    let mut identity_paths = defaults.identity_paths;
    let configured_identity_count = identity_paths.len();
    let mut expect_identity = false;
    #[cfg(not(feature = "session-persistence"))]
    let daemon_options = None;
    #[cfg(unix)]
    let mut resume_listener = None;
    #[cfg(windows)]
    let mut resume_ready = None;
    let mut upgrade = false;
    let mut pty_host = false;
    let mut force = false;

    for argument in arguments {
        let argument = argument.to_string_lossy();
        if expect_retention {
            retention = retention::Retention::parse(&argument)?;
            expect_retention = false;
            continue;
        }
        if expect_retention_bytes {
            anyhow::ensure!(
                retention_bytes.is_none(),
                "--retention-bytes may only be specified once"
            );
            retention_bytes = Some(
                argument
                    .parse::<usize>()
                    .map_err(|_| anyhow::anyhow!("--retention-bytes must be a whole number"))?,
            );
            expect_retention_bytes = false;
            continue;
        }
        if expect_identity {
            identity_paths.push(std::path::PathBuf::from(argument.as_ref()));
            expect_identity = false;
            continue;
        }
        match argument.as_ref() {
            "--json" | "-j" => json = true,
            "--force" | "-f" => force = true,
            "--retention" | "-r" => expect_retention = true,
            "--identity" | "-i" => expect_identity = true,
            value if value.starts_with("--identity=") => {
                let path = value
                    .split_once('=')
                    .map(|(_, path)| path)
                    .unwrap_or_default();
                anyhow::ensure!(!path.is_empty(), "--identity requires a path");
                identity_paths.push(std::path::PathBuf::from(path));
            }
            // Hidden: an independent daemon launcher may pass a byte budget
            // alongside its explicit startup retention mode.
            "--retention-bytes" => expect_retention_bytes = true,
            // Hidden: how a client starts the daemon it could not find.
            "--daemon" => daemon = true,
            value if value.starts_with("--daemon-options=") => {
                anyhow::ensure!(
                    daemon_options.is_none(),
                    "--daemon-options may only be specified once"
                );
                let path = std::path::PathBuf::from(
                    value
                        .split_once('=')
                        .map(|(_, path)| path)
                        .unwrap_or_default(),
                );
                #[cfg(feature = "session-persistence")]
                {
                    let bytes = std::fs::read(&path).with_context(|| {
                        format!("reading private daemon options {}", path.display())
                    })?;
                    let options: persistence::DaemonOptionsFile =
                        serde_json::from_slice(&bytes).context("parsing private daemon options")?;
                    let _ = std::fs::remove_file(&path);
                    daemon_options = Some(options.recipients);
                }
                #[cfg(not(feature = "session-persistence"))]
                {
                    let _ = path;
                    anyhow::bail!(
                        "--daemon-options needs the session-persistence feature, which this \
                         multiplexer was built without"
                    );
                }
            }
            // Hidden: the Windows pseudoconsole host, which outlives the
            // daemon so that replacing it does not end every session.
            "--pty-host" => pty_host = true,
            "--upgrade" | "-u" => upgrade = true,
            // Hidden: asked of a candidate replacement before this daemon
            // executes it, because `execv` cannot be undone.
            #[cfg(any(unix, windows))]
            value if value.starts_with("--resume-check") => {
                let supported = value
                    .split_once('=')
                    .map(|(_, version)| version.to_owned())
                    .or_else(|| {
                        arguments
                            .get(1)
                            .map(|next| next.to_string_lossy().into_owned())
                    });
                let understood = supported
                    .and_then(|version| version.trim().parse::<u32>().ok())
                    .is_some_and(|version| version == upgrade::HANDOVER_VERSION);
                std::process::exit(if understood { 0 } else { 1 });
            }
            // Hidden: set by the previous image when it replaced itself.
            #[cfg(unix)]
            value if value.starts_with("--resume-from=") => {
                resume_from = value
                    .split_once('=')
                    .and_then(|(_, descriptor)| descriptor.parse::<i32>().ok());
                anyhow::ensure!(resume_from.is_some(), "unusable --resume-from descriptor");
            }
            #[cfg(windows)]
            value if value.starts_with("--resume-from=") => {
                resume_from = value
                    .split_once('=')
                    .map(|(_, path)| std::path::PathBuf::from(path))
                    .filter(|path| !path.as_os_str().is_empty());
                anyhow::ensure!(resume_from.is_some(), "unusable --resume-from path");
            }
            // Hidden: inherited by the replacement on platforms where the
            // listening socket's peer credentials survive an upgrade. macOS
            // omits this argument and rebinds the socket in the new image.
            #[cfg(unix)]
            value if value.starts_with("--resume-listener=") => {
                resume_listener = value
                    .split_once('=')
                    .and_then(|(_, descriptor)| descriptor.parse::<i32>().ok());
                anyhow::ensure!(
                    resume_listener.is_some(),
                    "unusable --resume-listener descriptor"
                );
            }
            // Hidden: the candidate publishes this marker after it has read
            // and validated the handover, before it waits for the old daemon's
            // endpoint to go away.
            #[cfg(windows)]
            value if value.starts_with("--resume-ready=") => {
                resume_ready = value
                    .split_once('=')
                    .map(|(_, path)| std::path::PathBuf::from(path))
                    .filter(|path| !path.as_os_str().is_empty());
                anyhow::ensure!(resume_ready.is_some(), "unusable --resume-ready path");
            }
            "--help" | "-h" => {
                let session_id_help = if no_mux {
                    NO_MUX_SESSION_ID_HELP
                } else {
                    SESSION_ID_HELP
                };
                println!("{}\n\n{session_id_help}", usage(no_mux));
                return Ok(());
            }
            // `-V` as well as `-v`, which is what `zetta --version` takes and
            // therefore what anyone typing this expects. The uppercase spelling
            // predates that and still works rather than becoming an error for
            // whatever has already been scripted against it.
            "--version" | "-v" | "-V" => {
                // The protocol as well as the package version: which build this
                // is says nothing about whether it can talk to the multiplexer
                // that happens to be running, and that is the question anybody
                // reads a multiplexer's version for.
                println!(
                    "zmux {} (protocol {})",
                    env!("CARGO_PKG_VERSION"),
                    messages::PROTOCOL_VERSION
                );
                return Ok(());
            }
            value @ ("list" | "stop" | "reconnect" | "resume" | "share" | "unshare" | "kill"
            | "forget")
                if command.is_none() =>
            {
                command = Some(value.to_owned());
            }
            value
                if !value.starts_with('-')
                    && matches!(
                        command.as_deref(),
                        Some("reconnect" | "resume" | "kill" | "forget" | "share" | "unshare")
                    ) =>
            {
                anyhow::ensure!(session.is_none(), "only one session may be given");
                session = Some(SessionArgument::parse(&argument)?);
            }
            unknown => anyhow::bail!("unknown mux argument {unknown:?}"),
        }
    }
    anyhow::ensure!(!expect_retention, "--retention requires a mode");
    anyhow::ensure!(!expect_identity, "--identity requires a path");
    anyhow::ensure!(
        identity_paths.len() == configured_identity_count
            || matches!(command.as_deref(), Some("resume") | Some("reconnect")),
        "--identity is only valid with the resume and reconnect commands"
    );
    anyhow::ensure!(
        !expect_retention_bytes,
        "--retention-bytes requires a value"
    );
    if let Some(bytes) = retention_bytes {
        anyhow::ensure!(
            matches!(retention, retention::Retention::Memory { .. }),
            "--retention-bytes is only valid with --retention memory"
        );
        retention = retention::Retention::Memory { bytes };
    }
    retention.validate()?;

    if pty_host {
        #[cfg(windows)]
        return pty_host::run();
        #[cfg(not(windows))]
        anyhow::bail!("the pseudoconsole host exists only on Windows");
    }

    if upgrade {
        #[cfg(unix)]
        {
            // Tolerant of the version it answers with: replacing a multiplexer
            // from an earlier build is the whole point, and insisting on a
            // version match here reported the mismatch instead of resolving it.
            let Some(client) = client::Client::connect_for_upgrade()? else {
                anyhow::bail!("no multiplexer is running");
            };
            client.upgrade()?;
            println!("Replaced the multiplexer; its sessions were kept.");
            return Ok(());
        }
        #[cfg(windows)]
        {
            let Some(client) = client::Client::connect_for_upgrade()? else {
                anyhow::bail!("no multiplexer is running");
            };
            client.upgrade()?;
            println!("Replaced the multiplexer; its sessions were kept.");
            return Ok(());
        }
    }

    if daemon {
        #[cfg(unix)]
        return server::run(retention, daemon_options, resume_from, resume_listener);
        #[cfg(windows)]
        return server::run(retention, daemon_options, resume_from, resume_ready);
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (retention, daemon_options, resume_from);
            anyhow::bail!("the multiplexer daemon is not yet supported on this platform");
        }
    }
    #[cfg(unix)]
    let _ = (retention, daemon_options, resume_from, resume_listener);
    #[cfg(windows)]
    let _ = (retention, daemon_options, resume_from, resume_ready);

    match command.as_deref() {
        Some("reconnect") => {
            let session = session.context_missing()?;
            reconnect::run_reconnect_session(&session.to_string(), &identity_paths)
        }
        Some("stop") => {
            match stop(&paths::session_catalog_dir(), force)? {
                StopOutcome::NotRunning => println!("No multiplexer is running."),
                StopOutcome::Stopped => println!("Stopped the multiplexer."),
                StopOutcome::Signalled { process_id } => println!(
                    "Stopped the multiplexer (process {process_id}), ending what it was \
                     holding."
                ),
            }
            Ok(())
        }
        Some("resume") => {
            let session = session.context_missing()?;
            #[cfg(feature = "session-persistence")]
            {
                if reconnect::try_run_resume_disk_session(&session.to_string(), &identity_paths)? {
                    println!(
                        "Restored disk session {}; fresh shells were started by a Zetta window; original processes were not resumed.",
                        session
                    );
                    return Ok(());
                }
                let client = client::Client::connect_at_with_retention_for_resume(
                    &paths::session_catalog_dir(),
                    retention::Retention::Disk,
                )?;
                let record_id = resolve_restorable_id(&client, session)?;
                client.resume(record_id, &identity_paths)?;
                println!(
                    "Acknowledged disk session {record_id}; no Zetta window was available, so no fresh shells were started; original processes were not resumed."
                );
                Ok(())
            }
            #[cfg(not(feature = "session-persistence"))]
            {
                let _ = (session, identity_paths);
                anyhow::bail!(
                    "resume needs the session-persistence feature, which this multiplexer \
                     was built without"
                )
            }
        }
        Some("kill") => {
            let session = session.context_missing()?;
            let directory = paths::session_catalog_dir();
            if let Some(client) = client::Client::connect_existing()? {
                let session = resolve_session_id(&client, session, &directory)?;
                match client.kill(session) {
                    Ok(()) => println!("Ended session {session}."),
                    #[cfg(feature = "session-persistence")]
                    Err(error) if is_missing_session_error(&error, session) => {
                        anyhow::ensure!(
                            persistence::forget_opaque_record(&directory, session)?,
                            "session {session} does not exist"
                        );
                        println!("Forgot stale disk session {session}.");
                    }
                    Err(error) => return Err(error),
                }
            } else {
                #[cfg(feature = "session-persistence")]
                {
                    let session = resolve_forget_id(session, &directory)?;
                    anyhow::ensure!(
                        persistence::forget_opaque_record(&directory, session)?,
                        "session {session} does not exist"
                    );
                    println!("Forgot stale disk session {session}.");
                }
                #[cfg(not(feature = "session-persistence"))]
                anyhow::bail!("no multiplexer is running");
            }
            Ok(())
        }
        command @ (Some("share") | Some("unshare")) => {
            let shared = command == Some("share");
            let session = session.context_missing()?;
            let Some(client) = client::Client::connect_existing()? else {
                anyhow::bail!("no multiplexer is running");
            };
            let session = resolve_session_id(&client, session, &paths::session_catalog_dir())?;
            // Sharing is what makes a session joinable from another process,
            // so it is where a secret is offered: joining a session is being
            // handed whatever its terminals can already do, and a shell that
            // has answered `sudo` can do a great deal. Offered rather than
            // required, as the dialog in a window offers it — and not offered
            // at all for a session that already has one, because a second
            // secret would replace the one whoever knows it is expecting.
            let protection = match (shared, session_is_protected(&client, session)?) {
                (true, false) => secret_prompt::prompt_for_optional_secret()?
                    .map(|secret| auth::SessionAuthentication::create(secret.expose()))
                    .transpose()?,
                _ => None,
            };
            client.set_session_scope(session, shared, protection.as_ref())?;
            if shared {
                println!("Shared session {session} with every Zetta process.");
            } else {
                println!("Scoped session {session} back to the window that held it.");
            }
            Ok(())
        }
        Some("forget") => {
            let session = session.context_missing()?;
            #[cfg(feature = "session-persistence")]
            {
                let directory = paths::session_catalog_dir();
                let session_id = resolve_forget_id(session, &directory)?;
                if let Some(client) = client::Client::connect_existing()? {
                    match client.forget(session_id) {
                        Ok(()) => {}
                        Err(error) if is_missing_session_error(&error, session_id) => {
                            anyhow::ensure!(
                                persistence::forget_opaque_record(&directory, session_id)?,
                                "session {session_id} does not exist"
                            );
                        }
                        Err(error) => return Err(error),
                    }
                } else {
                    anyhow::ensure!(
                        persistence::forget_opaque_record(&directory, session_id)?,
                        "session {session_id} does not exist"
                    );
                }
                println!("Forgot session {session_id}.");
                Ok(())
            }
            #[cfg(not(feature = "session-persistence"))]
            {
                #[cfg(any(unix, windows))]
                {
                    let Some(client) = client::Client::connect_existing()? else {
                        anyhow::bail!("no multiplexer is running");
                    };
                    let session =
                        resolve_session_id(&client, session, &paths::session_catalog_dir())?;
                    client.forget(session)?;
                    println!("Forgot session {session}.");
                    Ok(())
                }
                #[cfg(not(any(unix, windows)))]
                {
                    let _ = session;
                    anyhow::bail!("the multiplexer is not yet supported on this platform")
                }
            }
        }
        Some("list") | None => catalog::print_session_catalogs(&paths::session_catalog_dir(), json),
        Some(unknown) => anyhow::bail!("unknown mux command {unknown:?}"),
    }
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

    let process_id = endpoint.process_id as libc::pid_t;
    anyhow::ensure!(
        process_id > 0,
        "the multiplexer published an unusable process id"
    );
    // SAFETY: a signal to one process id, published by the multiplexer for
    // itself, whose socket answered a moment ago.
    unsafe { libc::kill(process_id, libc::SIGTERM) };
    if wait_until_stopped(endpoint, STOP_GRACE_PERIOD).is_ok() {
        return Ok(());
    }
    unsafe { libc::kill(process_id, libc::SIGKILL) };
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
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};

    anyhow::ensure!(
        process_id > 0,
        "the process published an unusable process id"
    );
    // SAFETY: the process ID came from a private endpoint that answered just
    // before this function was called. The returned handle is closed below.
    let process = unsafe { OpenProcess(PROCESS_TERMINATE, false, process_id) }
        .with_context(|| format!("opening process {process_id} for termination"))?;
    let result = unsafe { TerminateProcess(process, 1) }
        .with_context(|| format!("terminating process {process_id}"));
    // SAFETY: `process` is the handle returned by `OpenProcess` above.
    unsafe {
        let _ = CloseHandle(process);
    }
    result
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
