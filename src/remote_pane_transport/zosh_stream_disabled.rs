//! What a build without the bundled Zosh client can carry a pane over.
//!
//! Nothing, so [`ZoshPaneStream`] has no values at all: every pane takes the
//! multiplexer's byte stream, and the caller needs no feature predicate of its
//! own to know it. Choosing Zosh still parses — configuration and the command
//! line are not rewritten by a build option — and falls back with a reason
//! that says which build this is.

use std::{
    collections::HashMap,
    io::{Read, Write},
    sync::Arc,
};

use terminal::PtyControl;
use zmux::auth::SessionSecret;

pub(crate) enum ZoshPaneStream {}

/// The same shape the enabled half returns, for call sites that are written
/// once for both.
pub(crate) struct ZoshTerminalParts {
    pub(crate) reader: Box<dyn Read + Send>,
    pub(crate) writer: Box<dyn Write + Send>,
    pub(crate) control: Arc<dyn PtyControl>,
    pub(crate) session: Arc<ZoshPaneHandle>,
}

/// The same name the enabled half exports, so a pane's field does not need a
/// feature predicate to be written down.
pub(crate) type ZoshPaneHandle = ZoshPaneStream;

pub(super) fn shutdown(session: &ZoshPaneHandle) {
    match *session {}
}

pub(crate) fn parse_keep_alive_interval(_value: &str) -> anyhow::Result<u64> {
    anyhow::bail!("this build has no bundled Zosh client, so it holds no link open")
}

impl ZoshPaneStream {
    pub(crate) fn into_terminal_parts(self) -> ZoshTerminalParts {
        match self {}
    }
}

pub(super) fn bootstrap(
    _client: &zmux::client::Client,
    _keep_alive_ms: Option<u64>,
    _forward_agent: bool,
    _session_id: u64,
    _secret: Option<&SessionSecret>,
    _mux_pane_ids: &[u64],
) -> (HashMap<u64, ZoshPaneStream>, Vec<String>) {
    (
        HashMap::new(),
        vec!["This build has no bundled Zosh client, so the panes stayed on SSH.".to_owned()],
    )
}
