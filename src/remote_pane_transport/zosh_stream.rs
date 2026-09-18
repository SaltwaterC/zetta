//! One remote pane, carried by the bundled Zosh client.
//!
//! Each pane is a `zosh-server` on the remote host running `zmux relay-pane`,
//! and a headless Mosh session here rendering it into the byte stream a
//! terminal is built from. See the parent module for why the pane travels this
//! way while everything else about the session stays on SSH.

use std::{
    collections::HashMap,
    io::{Read, Write},
    path::PathBuf,
    sync::Arc,
};

use terminal::{ConsolePalette, PtyControl};
use zmux::auth::SessionSecret;

/// The size a pane's Mosh session starts at.
///
/// Nothing has been laid out when a session is bootstrapped, so this is the
/// conventional terminal a program expects to start in; the pane's first
/// layout resizes it before anything is shown.
const INITIAL_PANE_SIZE: (u16, u16) = (80, 24);

/// What a pane holds on to for as long as it is shown. Dropping it ends the
/// Mosh session, which is what closing the pane should do.
pub(crate) type ZoshPaneHandle = zosh::PaneSession;

/// Configuration is parsed in builds that have no bundled Zosh client, so the
/// bounds it enforces are written in `config.rs`. These are the same numbers
/// seen from the other side: if the protocol's ever move, this stops the build
/// rather than letting a file be accepted that the client would then refuse.
const _: () = {
    assert!(crate::config::REMOTE_KEEP_ALIVE_MIN_MS == zosh::KEEP_ALIVE_MIN_MS);
    assert!(crate::config::REMOTE_KEEP_ALIVE_MAX_MS == zosh::KEEP_ALIVE_MAX_MS);
    assert!(crate::config::REMOTE_KEEP_ALIVE_DEFAULT_MS == zosh::KEEP_ALIVE_DEFAULT_MS);
};

/// Reads a keep-alive interval in milliseconds, with the bounds the protocol
/// itself sets. One implementation, so the picker, the configuration file and
/// the command line cannot disagree about what is accepted.
pub(crate) fn parse_keep_alive_interval(value: &str) -> anyhow::Result<u64> {
    zosh::parse_keep_alive_interval(value)
}

/// What a terminal is built from, once the session has been split up for it.
pub(crate) struct ZoshTerminalParts {
    pub(crate) reader: Box<dyn Read + Send>,
    pub(crate) writer: Box<dyn Write + Send>,
    pub(crate) control: Arc<dyn PtyControl>,
    /// Kept by the pane: the session ends when the last handle to it goes.
    pub(crate) session: Arc<ZoshPaneHandle>,
}

/// One pane's Mosh session, before its terminal has been built from it.
pub(crate) struct ZoshPaneStream {
    session: Arc<zosh::PaneSession>,
    reader: zosh::PaneReader,
}

impl ZoshPaneStream {
    /// Splits the session into what a terminal is built from: the display
    /// bytes, somewhere to put keystrokes, and the control it resizes through.
    ///
    /// The session handle comes back too and has to be kept for as long as the
    /// pane is shown: dropping it ends the Mosh session.
    pub(crate) fn into_terminal_parts(self) -> ZoshTerminalParts {
        let writer = self.session.writer();
        ZoshTerminalParts {
            control: Arc::new(ZoshPtyControl {
                session: Arc::clone(&self.session),
            }),
            reader: Box::new(self.reader),
            writer: Box::new(writer),
            session: self.session,
        }
    }
}

/// A terminal resizes through its pty control. For a Mosh pane that is the
/// Mosh session: the size travels to the remote `zosh-server`'s pty, the relay
/// is signalled by it, and the multiplexer learns this viewer's size from the
/// relay. Reporting it over the control connection as well would make one
/// viewer look like two.
struct ZoshPtyControl {
    session: Arc<zosh::PaneSession>,
}

impl PtyControl for ZoshPtyControl {
    fn resize(&self, columns: u16, lines: u16) {
        self.session.resize(columns, lines);
    }

    fn set_console_palette(&self, _palette: ConsolePalette) {
        // A console palette belongs to a Windows pseudoconsole, and what is at
        // the far end of a Mosh link is a POSIX pty. The remote emulator is
        // told its colours the ordinary way, in band.
    }
}

/// Brings up every pane concurrently, returning the ones that came up and a
/// sentence for each one that did not.
pub(super) fn bootstrap(
    client: &zmux::client::Client,
    keep_alive_ms: Option<u64>,
    forward_agent: bool,
    session_id: u64,
    secret: Option<&SessionSecret>,
    mux_pane_ids: &[u64],
) -> (HashMap<u64, ZoshPaneStream>, Vec<String>) {
    if forward_agent {
        return (
            HashMap::new(),
            vec![
                "SSH-agent forwarding is unavailable for existing zmux relay panes; their shells already exist, so those panes stayed on SSH."
                    .to_owned(),
            ],
        );
    }
    let Some(target) = client.remote_target().cloned() else {
        return (
            HashMap::new(),
            vec!["A local session's panes are already local, so Zosh has nothing to carry.".into()],
        );
    };
    // One round trip for the whole session rather than one per pane: the relay
    // is the same executable for every one of them.
    let program = match client.resolve_remote_program() {
        Ok(program) => program,
        Err(error) => {
            return (
                HashMap::new(),
                vec![format!(
                    "Could not find zmux on {}, so its panes stayed on SSH: {error:#}",
                    target.destination()
                )],
            );
        }
    };
    let request = PaneRequest {
        target,
        program,
        session_id,
        keep_alive_ms,
        forward_agent,
        secret: secret.map(|secret| secret.expose().to_owned()),
    };

    let mut streams = HashMap::new();
    let mut fallbacks = Vec::new();
    std::thread::scope(|scope| {
        let started = mux_pane_ids
            .iter()
            .map(|mux_pane_id| {
                let request = &request;
                let mux_pane_id = *mux_pane_id;
                (
                    mux_pane_id,
                    scope.spawn(move || bootstrap_one(request, mux_pane_id)),
                )
            })
            .collect::<Vec<_>>();
        for (mux_pane_id, handle) in started {
            match handle.join() {
                Ok(Ok(stream)) => {
                    streams.insert(mux_pane_id, stream);
                }
                Ok(Err(reason)) => fallbacks.push(reason),
                Err(_) => fallbacks.push(format!(
                    "Bootstrapping pane {mux_pane_id} over Zosh panicked, so it stayed on SSH."
                )),
            }
        }
    });
    (streams, fallbacks)
}

/// Everything a pane's bootstrap needs that is the same for every pane in the
/// session.
struct PaneRequest {
    target: zmux::remote::RemoteTarget,
    program: PathBuf,
    session_id: u64,
    keep_alive_ms: Option<u64>,
    forward_agent: bool,
    /// Held as plain text only while the bootstrap runs, and sent to the relay
    /// inside the established Mosh link rather than on a remote command line
    /// every account on that host can read.
    secret: Option<String>,
}

/// Brings up one pane, or says why it could not be.
///
/// The failure is a sentence rather than an error type because every failure
/// here has the same consequence — this pane stays on the multiplexer's byte
/// stream — so the caller's job is to report the reason, not to tell the kinds
/// apart.
fn bootstrap_one(request: &PaneRequest, mux_pane_id: u64) -> Result<ZoshPaneStream, String> {
    let endpoint = bootstrap_endpoint(request, mux_pane_id)?;
    let key = zosh::Base64Key::from_printable(&endpoint.key).map_err(|error| {
        format!("The Zosh endpoint for pane {mux_pane_id} carried an unusable key: {error}")
    })?;
    let mut session = zosh::PaneSession::connect(
        &endpoint.host,
        endpoint.port,
        &key,
        INITIAL_PANE_SIZE.0,
        INITIAL_PANE_SIZE.1,
        zosh::PaneSessionSettings {
            keep_alive: request.keep_alive_ms,
            forward_agent: request.forward_agent,
            agent_binding: endpoint.agent_binding,
            agent_path: endpoint.agent_path,
            // A pane is scrolled, so the history a Mosh state cannot describe
            // is exactly what it is for. The remote relay's own replay comes
            // through the same screen, so without this a reattached pane would
            // show one screenful of what it had been showing all along.
            scrollback_kib: zosh::SCROLLBACK_DEFAULT_KIB,
            ..zosh::PaneSessionSettings::default()
        },
    )
    .map_err(|error| {
        format!(
            "Could not reach the Zosh session for pane {mux_pane_id}, which stayed on SSH: \
             {error:#}"
        )
    })?;
    let reader = session
        .take_reader()
        .expect("a session hands out its reader once, and this is that once");
    // The relay is waiting on its first line before it attaches anything, and
    // this is the only path a secret travels: inside the Mosh link, never in
    // the remote command line.
    if let Some(secret) = &request.secret {
        let mut writer = session.writer();
        writeln!(writer, "{secret}").map_err(|error| {
            format!("Could not authenticate pane {mux_pane_id} over Zosh: {error}")
        })?;
    }
    Ok(ZoshPaneStream {
        session: Arc::new(session),
        reader,
    })
}

fn bootstrap_endpoint(
    request: &PaneRequest,
    mux_pane_id: u64,
) -> Result<zosh::PaneEndpoint, String> {
    let bootstrap = zosh::bootstrap_pane_endpoint(&zosh::PaneBootstrapRequest {
        target: request.target.destination().to_owned(),
        ssh_port: request.target.port(),
        remote_command: relay_command(request, mux_pane_id),
        keep_alive: request.keep_alive_ms,
        forward_agent: request.forward_agent,
        proxy_program: bundled_zosh_program(),
    });
    match bootstrap {
        Ok(zosh::PaneBootstrapOutcome::Endpoint(endpoint)) => {
            for diagnostic in &endpoint.diagnostics {
                log::debug!("zosh bootstrap for pane {mux_pane_id}: {diagnostic}");
            }
            Ok(endpoint)
        }
        Ok(zosh::PaneBootstrapOutcome::UnsupportedServer { output, .. }) => Err(format!(
            "{} has no usable Mosh server, so pane {mux_pane_id} stayed on SSH: {}",
            request.target.destination(),
            first_line(&output)
        )),
        Err(error) => Err(format!(
            "Could not start a Zosh session for pane {mux_pane_id}, which stayed on SSH: {error:#}"
        )),
    }
}

/// What `zosh-server` runs instead of a login shell.
///
/// The multiplexer is named by the absolute path the remote host's own shell
/// resolved, because a command started inside a Mosh server does not have the
/// `PATH` that an SSH command would have had.
fn relay_command(request: &PaneRequest, mux_pane_id: u64) -> Vec<String> {
    let mut command = vec![
        request.program.to_string_lossy().into_owned(),
        "relay-pane".to_owned(),
        request.session_id.to_string(),
        mux_pane_id.to_string(),
    ];
    if request.secret.is_some() {
        command.push("--secret-stdin".to_owned());
    }
    command
}

/// The bundled `zosh`, which OpenSSH runs to discover the address the Mosh
/// server is reachable at. It sits beside this executable; when it does not,
/// the bootstrap asks the remote SSH server which address it saw the
/// connection arrive on instead.
fn bundled_zosh_program() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let sibling = executable
        .parent()?
        .join(if cfg!(windows) { "zosh.exe" } else { "zosh" });
    sibling.is_file().then_some(sibling)
}

fn first_line(output: &str) -> String {
    output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("the remote host said nothing")
        .to_owned()
}

#[cfg(test)]
#[path = "../tests/remote_pane_transport/zosh_stream.rs"]
mod tests;
