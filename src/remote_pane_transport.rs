//! How a remote pane's bytes get here.
//!
//! A remote session is two conversations. One is control: listing, attaching,
//! spawning and closing panes, layout revisions, exit reports, image paste.
//! That is framed JSON, it is what the multiplexer speaks, and it travels over
//! the OpenSSH stream-local forward in `zmux`'s `remote.rs` — always, whatever
//! is chosen here.
//!
//! The other is the pane itself, which is a terminal. A terminal is what Mosh
//! carries well and what TCP over SSH carries badly: a suspended laptop or a
//! changed network stalls or kills an SSH forward, and every keystroke waits a
//! round trip. So a pane's bytes can be moved onto a Mosh link of their own
//! while control stays where it is:
//!
//! ```text
//! control  ssh -T -N -L … zmux.sock ─────────────▶ the remote multiplexer
//! pane     ssh TARGET 'zosh-server new -s -- zmux relay-pane S P'
//!          then UDP/SSP ───────────────────────────▶ that pane's bytes
//! ```
//!
//! Three things follow, and they are why this is a module rather than a flag:
//!
//! - **One link per pane.** Mosh carries one terminal, so each attached pane
//!   gets its own `zosh-server` and its own relay. They are bootstrapped
//!   concurrently, because each one is an SSH round trip.
//! - **A pane is on one transport or the other, decided before its terminal is
//!   built.** Nothing switches mid-stream: a cut-over would either lose output
//!   or write it into the scrollback twice. A pane added to a session already
//!   open takes the transport that session was opened with — half a tab on a
//!   transport the other half is not on would come apart the moment either one
//!   was interrupted — which is why the choice is kept on `MuxRuntime` rather
//!   than at the attach that made it.
//! - **A Mosh pane is not a shared-stream pane.** Zetta does not keep the
//!   multiplexer's byte stream for it — paying for the same output twice would
//!   defeat the point, and a pane that no longer needs the SSH forward should
//!   not die with it. The relay is the multiplexer's viewer for that pane, and
//!   the size it reports is the size this window asked for through Mosh. So
//!   this window is in no pane's shared set, and a control request that is
//!   authorized by watching a pane — image paste — has nothing to show. The
//!   relay names this window to the daemon as the viewer it is relaying to,
//!   which is what makes those requests recognizable; it travels inside the
//!   Mosh link, like a protected session's secret and for the same reason.
//!
//! The one thing that costs: arbitrated sizes reach the relay rather than this
//! window, so when a second viewer with a smaller window joins, the remote
//! program is resized for it but this pane's grid is not. The columns beyond
//! what the program draws are simply unused.

use std::collections::HashMap;

use zmux::auth::SessionSecret;

#[cfg(feature = "zosh-client")]
mod zosh_stream;
#[cfg(not(feature = "zosh-client"))]
#[path = "remote_pane_transport/zosh_stream_disabled.rs"]
mod zosh_stream;

pub(crate) use zosh_stream::{
    ZoshPaneHandle, ZoshPaneStream, ZoshTerminalParts, parse_keep_alive_interval,
};

/// A pane whose bytes travel over Mosh, as this window keeps it.
///
/// The session is held here rather than in the terminal because closing the
/// pane has to end the Mosh session, and because the pane's identity in the
/// multiplexer outlives the terminal that displays it.
pub(crate) struct ZoshPaneEntry {
    /// Held for the pane's lifetime and explicitly stopped when its process
    /// exits, before the terminal waits for its reader to finish.
    pub(crate) session: std::sync::Arc<ZoshPaneHandle>,
    pub(crate) mux_pane_id: u64,
    /// The runtime the pane's control traffic goes through, so closing the
    /// pane can forget what was registered for it.
    pub(crate) runtime: crate::mux::MuxRuntime,
}

impl ZoshPaneEntry {
    /// Stops the Mosh loop even though the terminal still holds its reader,
    /// writer and resize control. This makes the reader reach EOF before the
    /// terminal waits for its byte-stream worker to finish.
    pub(crate) fn shutdown(&self) {
        zosh_stream::shutdown(&self.session);
    }
}

/// How a remote session's panes travel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RemotePaneTransport {
    /// The multiplexer's own byte stream, over the SSH forward.
    #[default]
    Ssh,
    /// A Mosh session per pane, in front of a relay on the remote host.
    Zosh {
        keep_alive_ms: Option<u64>,
        forward_agent: bool,
    },
}

impl RemotePaneTransport {
    /// The name this transport is written as in configuration, on the command
    /// line, and in the picker.
    pub(crate) const SSH: &'static str = "ssh";
    pub(crate) const ZOSH: &'static str = "zosh";

    pub(crate) fn parse(
        value: &str,
        keep_alive_ms: Option<u64>,
        forward_agent: bool,
    ) -> anyhow::Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            Self::SSH => Ok(Self::Ssh),
            Self::ZOSH => Ok(Self::Zosh {
                keep_alive_ms,
                forward_agent,
            }),
            other => anyhow::bail!(
                "unknown remote session protocol {other:?}; expected {:?} or {:?}",
                Self::SSH,
                Self::ZOSH
            ),
        }
    }

    /// What a newly opened picker starts on.
    pub(crate) fn from_config(remote: &crate::config::RemoteSessionConfig) -> Self {
        match remote.protocol {
            crate::config::RemoteSessionProtocol::Ssh => Self::Ssh,
            crate::config::RemoteSessionProtocol::Zosh => Self::Zosh {
                keep_alive_ms: remote.keep_alive_ms,
                forward_agent: remote.forward_agent,
            },
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Ssh => Self::SSH,
            Self::Zosh { .. } => Self::ZOSH,
        }
    }

    pub(crate) fn is_zosh(self) -> bool {
        matches!(self, Self::Zosh { .. })
    }

    /// Only the tests ask: everything else either carries the transport whole
    /// or reads the interval out of the `Zosh` variant it just matched.
    #[cfg(test)]
    pub(crate) fn keep_alive_ms(self) -> Option<u64> {
        match self {
            Self::Ssh => None,
            Self::Zosh { keep_alive_ms, .. } => keep_alive_ms,
        }
    }
}

/// The panes a bootstrap put on Mosh, and what happened to the ones it could
/// not.
#[derive(Default)]
pub(crate) struct RemotePaneStreams {
    streams: HashMap<u64, ZoshPaneStream>,
    fallbacks: Vec<String>,
}

impl RemotePaneStreams {
    /// The streams for a single pane, for one that arrived on its own rather
    /// than with the session it belongs to.
    pub(crate) fn one(mux_pane_id: u64, stream: Option<ZoshPaneStream>) -> Self {
        Self {
            streams: stream
                .map(|stream| HashMap::from([(mux_pane_id, stream)]))
                .unwrap_or_default(),
            fallbacks: Vec::new(),
        }
    }

    /// Takes the Mosh stream for a multiplexer pane, or `None` when that pane
    /// is on the multiplexer's byte stream — which is what every pane of an
    /// SSH session answers.
    pub(crate) fn take(&mut self, mux_pane_id: u64) -> Option<ZoshPaneStream> {
        self.streams.remove(&mux_pane_id)
    }

    /// Why panes fell back, one sentence each, in the order they were
    /// bootstrapped.
    pub(crate) fn fallbacks(&self) -> &[String] {
        &self.fallbacks
    }
}

/// Brings up one Mosh session per pane, concurrently.
///
/// Blocking, and deliberately: every step is SSH or UDP I/O, so this belongs
/// on the background executor beside the rest of a remote attach.
///
/// A pane that cannot be brought up is not an error. It is left out of the
/// result with its reason recorded, and the caller attaches it over the
/// multiplexer's byte stream the way it always did.
pub(crate) fn bootstrap_remote_pane_streams(
    client: &zmux::client::Client,
    transport: RemotePaneTransport,
    session_id: u64,
    secret: Option<&SessionSecret>,
    mux_pane_ids: &[u64],
) -> RemotePaneStreams {
    let RemotePaneTransport::Zosh {
        keep_alive_ms,
        forward_agent,
    } = transport
    else {
        return RemotePaneStreams::default();
    };
    if mux_pane_ids.is_empty() {
        return RemotePaneStreams::default();
    }
    let (streams, fallbacks) = zosh_stream::bootstrap(
        client,
        keep_alive_ms,
        forward_agent,
        session_id,
        secret,
        mux_pane_ids,
    );
    RemotePaneStreams { streams, fallbacks }
}

/// Brings up one pane that was added to a session already open.
///
/// The session's transport is not chosen again here — a session whose panes
/// travel over Mosh has to carry the ones added later the same way, or closing
/// the SSH forward would take half a tab with it. `None` means this pane stays
/// on the multiplexer's byte stream, either because that is what the session
/// uses or because its bootstrap failed; the reason is logged, since a pane
/// arriving is not the moment to interrupt anybody.
///
/// Blocking, like the bootstrap it delegates to.
pub(crate) fn bootstrap_spawned_pane(
    runtime: &crate::mux::MuxRuntime,
    session_id: u64,
    mux_pane_id: u64,
) -> Option<ZoshPaneStream> {
    let transport = runtime.pane_transport();
    if !transport.is_zosh() {
        return None;
    }
    let secret = runtime.session_secret();
    let mut streams = bootstrap_remote_pane_streams(
        runtime.client(),
        transport,
        session_id,
        secret.as_ref(),
        &[mux_pane_id],
    );
    for reason in streams.fallbacks() {
        log::warn!("a pane added to a Zosh session stayed on SSH: {reason}");
    }
    streams.take(mux_pane_id)
}

#[cfg(test)]
#[path = "tests/remote_pane_transport.rs"]
mod tests;
