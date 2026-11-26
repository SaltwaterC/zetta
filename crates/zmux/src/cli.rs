//! `zmux`'s command line: reading the arguments into [`MuxArguments`],
//! refusing the combinations that make no sense, and running the command.
//!
//! An option that takes a value can be spelled `--option VALUE` or, for the
//! long names, `--option=VALUE`. Both spellings go through one
//! [`MuxArguments::set_value`] arm, so the checks on a value are written once
//! and cannot drift apart between them.

use std::{
    collections::HashMap,
    ffi::OsString,
    path::{Path, PathBuf},
};

use super::*;
use zetta_cli_help::set_once;

/// What the arguments asked for, before anything has been run.
struct MuxArguments {
    no_mux: bool,
    json: bool,
    ids_only: bool,
    daemon: bool,
    detach: bool,
    retention: retention::Retention,
    retention_bytes: Option<usize>,
    command: Option<String>,
    session: Option<SessionArgument>,
    remote: RemoteArguments,
    /// Explicit `--identity` paths append to the configured ones rather than
    /// replacing them, which is how `age -i` composes and what lets a second
    /// key be tried without having to restate the first.
    identity_paths: Vec<PathBuf>,
    configured_identity_count: usize,
    daemon_options: Option<Vec<String>>,
    handover: Handover,
    upgrade: bool,
    pty_host: bool,
    force: bool,
    relay_pane: Option<u64>,
    relay_secret_stdin: bool,
    relay_viewer_stdin: bool,
    create: CreateArguments,
}

/// Where a remote command goes, and how an `attach` reaches it.
#[derive(Default)]
struct RemoteArguments {
    target: Option<String>,
    port: Option<u16>,
    protocol: Option<String>,
    keep_alive: Option<u64>,
    forward_agent: Option<bool>,
}

/// What `create` was asked to start.
#[derive(Default)]
struct CreateArguments {
    layout: Option<PathBuf>,
    profile: Option<String>,
    title: Option<String>,
    working_directory: Option<PathBuf>,
    environment: HashMap<String, String>,
}

/// Hidden arguments a replaced daemon hands its successor.
#[derive(Default)]
struct Handover {
    #[cfg(unix)]
    resume_from: Option<i32>,
    #[cfg(windows)]
    resume_from: Option<upgrade::HandoverSource>,
    #[cfg(unix)]
    resume_listener: Option<i32>,
    #[cfg(windows)]
    resume_ready: Option<PathBuf>,
}

/// The options that take a value, by every name they can be given under, and
/// the canonical name [`MuxArguments::set_value`] knows them by.
///
/// `--keep-alive` is not here: its bare form takes no value, so `-k HOST`
/// still names a target, and an interval is only ever given as `=MS`.
const VALUE_OPTIONS: &[(&str, &[&str])] = &[
    ("--retention", &["--retention", "-r"]),
    ("--retention-bytes", &["--retention-bytes"]),
    ("--identity", &["--identity", "-i"]),
    ("--ssh-target", &["--ssh-target", "-H"]),
    ("--protocol", &["--protocol", "-P"]),
    ("--port", &["--port", "-p"]),
    ("--layout", &["--layout"]),
    ("--profile", &["--profile"]),
    ("--title", &["--title"]),
    ("--working-directory", &["--working-directory"]),
    ("--env", &["--env"]),
];

/// The long options that also take their value as `--option=VALUE`.
const EQUALS_OPTIONS: &[&str] = &[
    "--identity",
    "--ssh-target",
    "--protocol",
    "--port",
    "--layout",
    "--profile",
    "--title",
    "--working-directory",
    "--env",
];

fn value_option(argument: &str) -> Option<&'static str> {
    VALUE_OPTIONS
        .iter()
        .find(|(_, names)| names.contains(&argument))
        .map(|(canonical, _)| *canonical)
}

/// What a value option says when it is the last argument.
fn missing_value(option: &str) -> String {
    let what = match option {
        "--retention" => "a mode",
        "--identity" | "--layout" | "--working-directory" => "a path",
        "--ssh-target" => "a destination",
        "--protocol" => "ssh or zosh",
        "--profile" => "a name",
        "--title" => "text",
        "--env" => "KEY=VALUE",
        _ => "a value",
    };
    format!("{option} requires {what}")
}

/// `value`, refused when it is empty, for the options a blank value cannot mean
/// anything for.
fn non_empty<'a>(option: &str, value: &'a str, what: &str) -> Result<&'a str> {
    anyhow::ensure!(!value.trim().is_empty(), "{option} requires {what}");
    Ok(value)
}

pub(super) fn run_command(arguments: &[OsString], defaults: ClientDefaults) -> Result<()> {
    let Some(mut parsed) = MuxArguments::parse(arguments, defaults)? else {
        return Ok(());
    };
    parsed.validate()?;
    parsed.run(arguments)
}

impl MuxArguments {
    fn new(defaults: ClientDefaults) -> Self {
        let identity_paths = defaults.identity_paths;
        Self {
            no_mux: no_mux_environment(),
            json: false,
            ids_only: false,
            daemon: false,
            detach: false,
            retention: retention::Retention::default(),
            retention_bytes: None,
            command: None,
            session: None,
            remote: RemoteArguments::default(),
            configured_identity_count: identity_paths.len(),
            identity_paths,
            daemon_options: None,
            handover: Handover::default(),
            upgrade: false,
            pty_host: false,
            force: false,
            relay_pane: None,
            relay_secret_stdin: false,
            relay_viewer_stdin: false,
            create: CreateArguments::default(),
        }
    }

    /// `None` when the arguments were answered without running anything:
    /// `--help` and `--version`.
    fn parse(arguments: &[OsString], defaults: ClientDefaults) -> Result<Option<Self>> {
        let mut parsed = Self::new(defaults);
        let mut pending: Option<&'static str> = None;
        for argument in arguments {
            let argument = argument.to_string_lossy();
            if let Some(option) = pending.take() {
                parsed.set_value(option, &argument)?;
                continue;
            }
            if let Some((name, value)) = argument.split_once('=')
                && let Some(option) = EQUALS_OPTIONS.iter().find(|option| **option == name)
            {
                parsed.set_value(option, value)?;
                continue;
            }
            if let Some(option) = value_option(&argument) {
                pending = Some(option);
                continue;
            }
            if !parsed.apply(&argument, arguments)? {
                return Ok(None);
            }
        }
        if let Some(option) = pending {
            anyhow::bail!(missing_value(option));
        }
        Ok(Some(parsed))
    }

    /// The value of the option canonically named `option`, however it was
    /// spelled.
    fn set_value(&mut self, option: &'static str, value: &str) -> Result<()> {
        match option {
            "--retention" => self.retention = value.parse()?,
            "--retention-bytes" => {
                set_once(&mut self.retention_bytes, option, || {
                    value
                        .parse::<usize>()
                        .map_err(|_| anyhow::anyhow!("--retention-bytes must be a whole number"))
                })?;
            }
            "--identity" => {
                let path = non_empty(option, value, "a path")?;
                self.identity_paths.push(PathBuf::from(path));
            }
            "--ssh-target" => {
                set_once(&mut self.remote.target, option, || {
                    let target = non_empty(option, value, "a destination")?;
                    Ok(target.to_owned())
                })?;
            }
            "--protocol" => {
                set_once(&mut self.remote.protocol, option, || {
                    parse_remote_protocol(value)
                })?;
            }
            "--port" => {
                set_once(&mut self.remote.port, option, || {
                    let port = value
                        .parse::<u16>()
                        .map_err(|_| anyhow::anyhow!("--port must be a number from 1 to 65535"))?;
                    anyhow::ensure!(port != 0, "--port must be between 1 and 65535");
                    Ok(port)
                })?;
            }
            "--layout" => {
                set_once(&mut self.create.layout, option, || {
                    let path = non_empty(option, value, "a path")?;
                    Ok(PathBuf::from(path))
                })?;
            }
            "--profile" => {
                set_once(&mut self.create.profile, option, || {
                    let profile = non_empty(option, value, "a name")?;
                    Ok(profile.to_owned())
                })?;
            }
            "--title" => set_once(&mut self.create.title, option, || Ok(value.to_owned()))?,
            "--working-directory" => {
                set_once(&mut self.create.working_directory, option, || {
                    let path = non_empty(option, value, "a path")?;
                    Ok(PathBuf::from(path))
                })?;
            }
            "--env" => {
                let (name, value) = parse_environment_assignment(value)?;
                anyhow::ensure!(
                    self.create
                        .environment
                        .insert(name.clone(), value)
                        .is_none(),
                    "--env may not specify the same variable twice: {name}"
                );
            }
            other => unreachable!("{other} is not a value option"),
        }
        Ok(())
    }

    /// Applies a flag, a hidden daemon option or a positional argument. False
    /// when the argument has already been answered and nothing should run.
    fn apply(&mut self, argument: &str, arguments: &[OsString]) -> Result<bool> {
        match argument {
            "--json" | "-j" => self.json = true,
            "--ids-only" | "-I" => self.ids_only = true,
            "--secret-stdin" | "-S" => self.relay_secret_stdin = true,
            "--viewer-stdin" | "-w" => self.relay_viewer_stdin = true,
            "--force" | "-f" => self.force = true,
            // The short form takes no value, so `-k HOST` still names a
            // target; `-k=MS` and `--keep-alive=MS` are how an interval is
            // given. This is the spelling `zosh -k` already uses.
            "--keep-alive" | "-k" => {
                set_once(&mut self.remote.keep_alive, "--keep-alive", || {
                    Ok(KEEP_ALIVE_DEFAULT_MS)
                })?;
            }
            value if value.starts_with("--keep-alive=") || value.starts_with("-k=") => {
                let interval = value.split_once('=').map_or("", |(_, value)| value);
                set_once(&mut self.remote.keep_alive, "--keep-alive", || {
                    parse_keep_alive_interval(interval)
                })?;
            }
            "--forward-agent" | "--no-forward-agent" => {
                anyhow::ensure!(
                    self.remote.forward_agent.is_none(),
                    "agent-forwarding options may only be specified once"
                );
                self.remote.forward_agent = Some(argument == "--forward-agent");
            }
            "--help" | "-h" => {
                let session_id_help = if self.no_mux {
                    NO_MUX_SESSION_ID_HELP
                } else {
                    SESSION_ID_HELP
                };
                println!("{}\n\n{session_id_help}", usage(self.no_mux));
                return Ok(false);
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
                    "zmux {} (protocol {})\n{}",
                    env!("CARGO_PKG_VERSION"),
                    messages::PROTOCOL_VERSION,
                    versions::compatibility_version_text(),
                );
                return Ok(false);
            }
            value if value.starts_with('-') => self.apply_daemon_option(value, arguments)?,
            value => self.apply_positional(value)?,
        }
        Ok(true)
    }

    /// The hidden options a client, a launcher or a replaced daemon passes.
    fn apply_daemon_option(&mut self, argument: &str, arguments: &[OsString]) -> Result<()> {
        let value = || argument.split_once('=').map_or("", |(_, value)| value);
        match argument {
            // Hidden: how a client starts the daemon it could not find.
            "--daemon" => self.daemon = true,
            // Hidden: with --daemon, start the daemon so it outlives this
            // process's session, then return — how a Windows host is asked to
            // start one over SSH.
            "--detach" => self.detach = true,
            option if option.starts_with("--daemon-options=") => {
                set_once(&mut self.daemon_options, "--daemon-options", || {
                    read_daemon_options(Path::new(value()))
                })?;
            }
            // Hidden: the Windows pseudoconsole host, which outlives the
            // daemon so that replacing it does not end every session.
            "--pty-host" => self.pty_host = true,
            "--upgrade" | "-u" => self.upgrade = true,
            // Hidden: asked of a candidate replacement before this daemon
            // executes it, because `execv` cannot be undone.
            #[cfg(any(unix, windows))]
            option if option.starts_with("--resume-check") => {
                let supported = argument
                    .split_once('=')
                    .map(|(_, version)| version.to_owned())
                    .or_else(|| {
                        arguments
                            .get(1)
                            .map(|next| next.to_string_lossy().into_owned())
                    });
                let understood = supported
                    .and_then(|version| version.trim().parse::<u32>().ok())
                    .is_some_and(upgrade::accepts_handover_version);
                std::process::exit(if understood { 0 } else { 1 });
            }
            // Hidden: set by the previous image when it replaced itself.
            #[cfg(unix)]
            option if option.starts_with("--resume-from=") => {
                self.handover.resume_from = value().parse::<i32>().ok();
                anyhow::ensure!(
                    self.handover.resume_from.is_some(),
                    "unusable --resume-from descriptor"
                );
            }
            // Hidden: how a daemon from before the handover pipe still hands
            // over to this one. The file is removed once read.
            #[cfg(windows)]
            option if option.starts_with("--resume-from=") => {
                self.handover.resume_from = Some(PathBuf::from(value()))
                    .filter(|path| !path.as_os_str().is_empty())
                    .map(upgrade::HandoverSource::File);
                anyhow::ensure!(
                    self.handover.resume_from.is_some(),
                    "unusable --resume-from path"
                );
            }
            // Hidden: set by the previous image, which then sends the handover
            // over a pipe it announces on stdin.
            #[cfg(windows)]
            "--resume-from-pipe" => {
                self.handover.resume_from = Some(upgrade::HandoverSource::Pipe);
            }
            // Hidden: inherited by the replacement on platforms where the
            // listening socket's peer credentials survive an upgrade. macOS
            // omits this argument and rebinds the socket in the new image.
            #[cfg(unix)]
            option if option.starts_with("--resume-listener=") => {
                self.handover.resume_listener = value().parse::<i32>().ok();
                anyhow::ensure!(
                    self.handover.resume_listener.is_some(),
                    "unusable --resume-listener descriptor"
                );
            }
            // Hidden: the candidate publishes this marker after it has read
            // and validated the handover, before it waits for the old daemon's
            // endpoint to go away.
            #[cfg(windows)]
            option if option.starts_with("--resume-ready=") => {
                self.handover.resume_ready =
                    Some(PathBuf::from(value())).filter(|path| !path.as_os_str().is_empty());
                anyhow::ensure!(
                    self.handover.resume_ready.is_some(),
                    "unusable --resume-ready path"
                );
            }
            unknown => anyhow::bail!("unknown mux argument {unknown:?}"),
        }
        Ok(())
    }

    /// The command, and the session, pane or target it names.
    fn apply_positional(&mut self, argument: &str) -> Result<()> {
        match (self.command.as_deref(), argument) {
            (
                None,
                "list" | "profiles" | "create" | "endpoint" | "proxy-mux" | "attach" | "stop"
                | "reconnect" | "resume" | "share" | "unshare" | "kill" | "forget" | "relay-pane",
            ) => self.command = Some(argument.to_owned()),
            (Some("relay-pane"), _) if self.session.is_none() => {
                self.session = Some(SessionArgument::parse(argument)?);
            }
            (Some("relay-pane"), pane) => {
                anyhow::ensure!(self.relay_pane.is_none(), "only one pane may be relayed");
                self.relay_pane = Some(
                    pane.parse::<u64>()
                        .map_err(|_| anyhow::anyhow!("the pane ID must be a whole number"))?,
                );
            }
            (Some("attach"), target) if self.remote.target.is_none() => {
                self.remote.target = Some(target.to_owned());
            }
            (
                Some("attach" | "reconnect" | "resume" | "kill" | "forget" | "share" | "unshare"),
                _,
            ) => {
                anyhow::ensure!(self.session.is_none(), "only one session may be given");
                self.session = Some(SessionArgument::parse(argument)?);
            }
            (_, unknown) => anyhow::bail!("unknown mux argument {unknown:?}"),
        }
        Ok(())
    }

    fn command_is(&self, commands: &[&str]) -> bool {
        self.command
            .as_deref()
            .is_some_and(|command| commands.contains(&command))
    }

    /// Refuses options given to a command they mean nothing to, and settles
    /// the retention the daemon would be started with.
    fn validate(&mut self) -> Result<()> {
        const REMOTE_COMMANDS: &[&str] = &[
            "list", "profiles", "create", "attach", "kill", "share", "unshare", "forget",
        ];
        let remote = &self.remote;
        let create = &self.create;
        anyhow::ensure!(
            remote.protocol.is_none() || self.command_is(&["attach"]),
            "--protocol is only valid with attach"
        );
        anyhow::ensure!(
            remote.keep_alive.is_none() || self.command_is(&["attach"]),
            "--keep-alive is only valid with attach"
        );
        anyhow::ensure!(
            remote.keep_alive.is_none() || remote.protocol.as_deref() == Some(REMOTE_PROTOCOL_ZOSH),
            "--keep-alive holds a Zosh link open, so it needs --protocol zosh"
        );
        anyhow::ensure!(
            remote.forward_agent.is_none() || self.command_is(&["attach", "proxy-mux"]),
            "agent-forwarding options are only valid with attach and proxy-mux"
        );
        anyhow::ensure!(
            self.identity_paths.len() == self.configured_identity_count
                || self.command_is(&["resume", "reconnect"])
                || (remote.target.is_some() && self.command_is(REMOTE_COMMANDS)),
            "--identity is only valid with resume, reconnect, or remote commands"
        );
        anyhow::ensure!(
            !self.ids_only || self.command_is(&["list"]),
            "--ids-only is only valid with list"
        );
        anyhow::ensure!(
            !self.relay_secret_stdin || self.command_is(&["relay-pane", "create"]),
            "--secret-stdin is only valid with relay-pane or create"
        );
        anyhow::ensure!(
            !self.relay_viewer_stdin || self.command_is(&["relay-pane"]),
            "--viewer-stdin is only valid with relay-pane"
        );
        for (given, option) in [
            (create.layout.is_some(), "--layout"),
            (create.profile.is_some(), "--profile"),
            (create.title.is_some(), "--title"),
            (create.working_directory.is_some(), "--working-directory"),
            (!create.environment.is_empty(), "--env"),
        ] {
            anyhow::ensure!(
                !given || self.command_is(&["create"]),
                "{option} is only valid with create"
            );
        }
        anyhow::ensure!(
            create.layout.is_none()
                || (create.profile.is_none()
                    && create.working_directory.is_none()
                    && create.environment.is_empty()),
            "--layout cannot be combined with --profile, --working-directory, or --env"
        );
        anyhow::ensure!(
            !(create
                .layout
                .as_deref()
                .is_some_and(|path| path.as_os_str() == "-")
                && self.relay_secret_stdin),
            "--layout - cannot be combined with --secret-stdin"
        );
        anyhow::ensure!(
            remote.target.is_none() || self.command_is(REMOTE_COMMANDS),
            "--ssh-target is only valid with list, profiles, create, attach, share, unshare, kill, or forget"
        );
        anyhow::ensure!(
            remote.port.is_none() || remote.target.is_some(),
            "--port requires a remote SSH target"
        );
        if let Some(bytes) = self.retention_bytes {
            anyhow::ensure!(
                matches!(self.retention, retention::Retention::Memory { .. }),
                "--retention-bytes is only valid with --retention memory"
            );
            self.retention = retention::Retention::Memory { bytes };
        }
        self.retention.validate()?;
        anyhow::ensure!(
            !self.detach || self.daemon,
            "--detach is only valid with --daemon"
        );
        Ok(())
    }

    /// Runs what was asked for: one of the process roles, or a command.
    fn run(self, arguments: &[OsString]) -> Result<()> {
        if self.pty_host {
            #[cfg(windows)]
            return pty_host::run();
            #[cfg(not(windows))]
            anyhow::bail!("the pseudoconsole host exists only on Windows");
        }
        #[cfg(any(unix, windows))]
        if self.upgrade {
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
        if self.detach {
            let forwarded = arguments
                .iter()
                .filter(|argument| !matches!(argument.to_str(), Some("--daemon" | "--detach")))
                .cloned()
                .collect::<Vec<_>>();
            return client::start_detached_daemon(&forwarded);
        }
        if self.daemon {
            return self.run_daemon();
        }
        match self.command.as_deref() {
            Some("profiles") => {
                run_profiles(self.remote.target.as_deref(), self.remote.port, self.json)
            }
            Some("create") => self.run_create(),
            Some("endpoint") => self.run_endpoint(),
            Some("proxy-mux") => self.run_proxy_mux(),
            Some("attach") => self.run_attach(),
            Some("relay-pane") => self.run_relay_pane(),
            Some("reconnect") => {
                let session = self.session.context_missing()?;
                reconnect::run_reconnect_session(&session.to_string(), &self.identity_paths)
            }
            Some("stop") => run_stop(self.force),
            Some("resume") => self.run_resume(),
            Some("kill") => self.run_kill(),
            Some("share") => self.run_scope(true),
            Some("unshare") => self.run_scope(false),
            Some("forget") => self.run_forget(),
            Some("list") | None => self.run_list(),
            Some(unknown) => anyhow::bail!("unknown mux command {unknown:?}"),
        }
    }

    fn run_daemon(self) -> Result<()> {
        logging::init_daemon_log(&paths::session_catalog_dir());
        #[cfg(unix)]
        return server::run(
            self.retention,
            self.daemon_options,
            self.handover.resume_from,
            self.handover.resume_listener,
        );
        #[cfg(windows)]
        return server::run(
            self.retention,
            self.daemon_options,
            self.handover.resume_from,
            self.handover.resume_ready,
        );
        #[cfg(not(any(unix, windows)))]
        anyhow::bail!("the multiplexer daemon is not yet supported on this platform");
    }

    fn run_create(self) -> Result<()> {
        let CreateArguments {
            layout,
            profile,
            title,
            working_directory,
            environment,
        } = self.create;
        run_create(CreateOptions {
            layout_path: layout,
            profile,
            title,
            working_directory,
            environment,
            secret_from_stdin: self.relay_secret_stdin,
            remote_target: self.remote.target.as_deref(),
            port: self.remote.port,
            retention: self.retention,
            json: self.json,
        })
    }

    fn run_endpoint(&self) -> Result<()> {
        anyhow::ensure!(
            self.json,
            "endpoint requires --json so its output stays machine-readable"
        );
        anyhow::ensure!(
            self.remote.target.is_none(),
            "endpoint is a local daemon command"
        );
        anyhow::ensure!(
            self.remote.port.is_none(),
            "endpoint is a local daemon command"
        );
        let endpoint = remote::live_endpoint(&paths::session_catalog_dir())?;
        println!("{}", serde_json::to_string(&endpoint)?);
        Ok(())
    }

    fn run_proxy_mux(&self) -> Result<()> {
        anyhow::ensure!(
            self.remote.target.is_none() && self.remote.port.is_none(),
            "proxy-mux is a local daemon command"
        );
        remote::run_mux_proxy(self.remote.forward_agent == Some(true))
    }

    fn run_attach(self) -> Result<()> {
        let target = self
            .remote
            .target
            .context("attach requires an SSH target")?;
        let session = self
            .session
            .context("attach requires a numeric remote session ID")?;
        let SessionArgument::Bare(session_id) = session else {
            anyhow::bail!("remote session IDs must be numeric");
        };
        reconnect::run_remote_attach(
            &target,
            self.remote.port,
            session_id,
            &self.identity_paths,
            &reconnect::RemoteProtocolRequest {
                protocol: self.remote.protocol,
                keep_alive_ms: self.remote.keep_alive,
                forward_agent: self.remote.forward_agent,
            },
        )
    }

    fn run_relay_pane(self) -> Result<()> {
        let session = self
            .session
            .context("relay-pane requires a numeric session ID")?;
        let SessionArgument::Bare(session_id) = session else {
            anyhow::bail!("relayed session IDs must be numeric");
        };
        let pane_id = self.relay_pane.context("relay-pane requires a pane ID")?;
        relay::run(relay::RelayOptions {
            session_id,
            pane_id,
            secret_from_stdin: self.relay_secret_stdin,
            viewer_from_stdin: self.relay_viewer_stdin,
        })
    }

    #[cfg_attr(
        not(feature = "session-persistence"),
        expect(
            unused_variables,
            reason = "only a persistence build resumes a disk session"
        )
    )]
    fn run_resume(self) -> Result<()> {
        let session = self.session.context_missing()?;
        #[cfg(feature = "session-persistence")]
        {
            if reconnect::try_run_resume_disk_session(&session.to_string(), &self.identity_paths)? {
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
            client.resume(record_id, &self.identity_paths)?;
            println!(
                "Acknowledged disk session {record_id}; no Zetta window was available, so no fresh shells were started; original processes were not resumed."
            );
            Ok(())
        }
        #[cfg(not(feature = "session-persistence"))]
        {
            anyhow::bail!(
                "resume needs the session-persistence feature, which this multiplexer \
                 was built without"
            )
        }
    }

    fn run_kill(self) -> Result<()> {
        let session = self.session.context_missing()?;
        if ran_remotely(&self.remote, &self.identity_paths, "kill", &session)? {
            return Ok(());
        }
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

    /// `share` (`shared`) or `unshare`.
    fn run_scope(self, shared: bool) -> Result<()> {
        let session = self.session.context_missing()?;
        if ran_remotely(
            &self.remote,
            &self.identity_paths,
            if shared { "share" } else { "unshare" },
            &session,
        )? {
            return Ok(());
        }
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

    #[cfg_attr(
        not(any(unix, windows)),
        expect(unused_variables, reason = "no other platform runs the multiplexer")
    )]
    fn run_forget(self) -> Result<()> {
        let session = self.session.context_missing()?;
        if ran_remotely(&self.remote, &self.identity_paths, "forget", &session)? {
            return Ok(());
        }
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
                let session = resolve_session_id(&client, session, &paths::session_catalog_dir())?;
                client.forget(session)?;
                println!("Forgot session {session}.");
                Ok(())
            }
            #[cfg(not(any(unix, windows)))]
            {
                anyhow::bail!("the multiplexer is not yet supported on this platform")
            }
        }
    }

    fn run_list(&self) -> Result<()> {
        anyhow::ensure!(
            !self.json || !self.ids_only,
            "--json and --ids-only cannot be combined"
        );
        if let Some(target) = self.remote.target.as_deref() {
            let target = remote::RemoteTarget::new(target).with_port(self.remote.port);
            let client = client::Client::connect_remote(target)
                .context("connecting to the remote multiplexer")?;
            let sessions = client.list()?;
            if self.ids_only {
                for session in sessions {
                    println!("{}", session.id);
                }
            } else if self.json {
                println!("{}", serde_json::to_string(&sessions)?);
            } else {
                print_remote_sessions(&sessions);
            }
            return Ok(());
        }
        if self.ids_only {
            for catalog in catalog::read_session_catalogs(&paths::session_catalog_dir())? {
                for session in catalog.sessions {
                    println!("{}", session.id);
                }
            }
            Ok(())
        } else {
            catalog::print_session_catalogs(&paths::session_catalog_dir(), self.json)
        }
    }
}

/// For the session commands that can also act on a remote multiplexer: true
/// when `command` was sent to the remote target, so nothing is left to do here.
fn ran_remotely(
    remote: &RemoteArguments,
    identity_paths: &[PathBuf],
    command: &str,
    session: &SessionArgument,
) -> Result<bool> {
    let Some(target) = remote.target.as_deref() else {
        return Ok(false);
    };
    let SessionArgument::Bare(session_id) = *session else {
        anyhow::bail!("remote session IDs must be numeric");
    };
    reconnect::run_remote_admin(command, target, remote.port, session_id, identity_paths)?;
    Ok(true)
}

fn run_stop(force: bool) -> Result<()> {
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

/// The recipients a private daemon-options file names. The file is removed
/// once read.
#[cfg_attr(
    not(feature = "session-persistence"),
    expect(
        unused_variables,
        reason = "daemon options only carry persistence recipients"
    )
)]
fn read_daemon_options(path: &Path) -> Result<Vec<String>> {
    #[cfg(feature = "session-persistence")]
    {
        let bytes = std::fs::read(path)
            .with_context(|| format!("reading private daemon options {}", path.display()))?;
        let options: persistence::DaemonOptionsFile =
            serde_json::from_slice(&bytes).context("parsing private daemon options")?;
        let _ = std::fs::remove_file(path);
        Ok(options.recipients)
    }
    #[cfg(not(feature = "session-persistence"))]
    {
        anyhow::bail!(
            "--daemon-options needs the session-persistence feature, which this \
             multiplexer was built without"
        );
    }
}
