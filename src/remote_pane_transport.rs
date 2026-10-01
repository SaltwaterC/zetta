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
//! login    ssh -M -S ctl TARGET            (one per remote host)
//! control  ssh -S ctl -O forward -L … zmux.sock ──▶ the remote multiplexer
//! pane     ssh -S ctl TARGET 'zosh-server new -s -- zmux relay-pane S P'
//!          then UDP/SSP ───────────────────────────▶ that pane's bytes
//! ```
//!
//! On Windows, whose OpenSSH shares no connections, control is instead one
//! `ssh TARGET zmux proxy-mux` carrying every connection (see `zmux`'s
//! `mux_bridge.rs`), and each pane's bootstrap is a login of its own.
//!
//! Four things follow, and they are why this is a module rather than a flag:
//!
//! - **One link per pane.** Mosh carries one terminal, so each attached pane
//!   gets its own `zosh-server` and its own relay. They are bootstrapped
//!   concurrently, as sessions on the SSH login the control forward already
//!   holds where the platform allows it (not Windows), so a pane costs a
//!   session rather than a login.
//! - **A pane on Mosh is never attached over SSH as well.** Every pane but
//!   the first is bootstrapped before it is attached, and only the ones
//!   that fall back are attached over the forward — which would otherwise
//!   carry each pane's whole replay just to throw it away.
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
///
/// A reason is kept against the pane it is about, because a pane that fell
/// back is marked as being on SSH for as long as it is shown: a one-off notice
/// is gone before anyone wonders why one pane of a Zosh session stalls with
/// the SSH forward while its neighbours roam.
#[derive(Default)]
pub(crate) struct RemotePaneStreams {
    streams: HashMap<u64, ZoshPaneStream>,
    fallbacks: Vec<(u64, String)>,
}

impl RemotePaneStreams {
    /// Why this multiplexer pane stayed on SSH, when a Zosh bootstrap was
    /// tried for it and failed. `None` for a pane on Mosh, and for every pane
    /// of a session that chose SSH: nothing fell back there.
    pub(crate) fn fallback(&self, mux_pane_id: u64) -> Option<&str> {
        self.fallbacks
            .iter()
            .find(|(pane, _)| *pane == mux_pane_id)
            .map(|(_, reason)| reason.as_str())
    }

    /// Whether the bootstrap put this pane on Mosh.
    pub(crate) fn contains(&self, mux_pane_id: u64) -> bool {
        self.streams.contains_key(&mux_pane_id)
    }

    /// Takes the Mosh stream for a multiplexer pane, or `None` when that pane
    /// is on the multiplexer's byte stream — which is what every pane of an
    /// SSH session answers.
    pub(crate) fn take(&mut self, mux_pane_id: u64) -> Option<ZoshPaneStream> {
        self.streams.remove(&mux_pane_id)
    }

    /// Why panes fell back, one sentence each, in the order they were
    /// bootstrapped.
    pub(crate) fn fallbacks(&self) -> impl Iterator<Item = &str> {
        self.fallbacks.iter().map(|(_, reason)| reason.as_str())
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
/// the SSH forward would take half a tab with it. A pane with no stream in the
/// result stays on the multiplexer's byte stream, either because that is what
/// the session uses or because its bootstrap failed, which the result's
/// fallback says.
///
/// Blocking, like the bootstrap it delegates to.
pub(crate) fn bootstrap_spawned_pane(
    runtime: &crate::mux::MuxRuntime,
    session_id: u64,
    mux_pane_id: u64,
) -> RemotePaneStreams {
    let secret = runtime.session_secret();
    let streams = bootstrap_remote_pane_streams(
        runtime.client(),
        runtime.pane_transport(),
        session_id,
        secret.as_ref(),
        &[mux_pane_id],
    );
    for reason in streams.fallbacks() {
        log::warn!("a pane added to a Zosh session stayed on SSH: {reason}");
    }
    streams
}

#[cfg(test)]
#[path = "tests/remote_pane_transport.rs"]
mod tests;
