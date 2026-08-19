//! The application's connection to the session multiplexer.
//!
//! Every pane's process belongs to `zmux`, not to Zetta: that is what lets a
//! session outlive the window it was started in, and eventually be attached
//! from another machine. While a pane is attached, Zetta holds the PTY
//! descriptor itself and reads it directly, so an attached terminal costs
//! exactly what a locally spawned one costs.
//!
//! Two things follow from the multiplexer owning the process, and both are
//! handled here. Zetta cannot reap the child, so exit statuses arrive over the
//! event subscription and are routed to the terminal that is showing the pane.
//! And a tab is a session: panes spawned into the same tab have to join the
//! same session, which is what [`MuxSession`] tracks.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use anyhow::{Context as _, Result};
use gpui::AppContext as _;
use terminal::{PtyHandover, PtyProvider, PtySpawnRequest};
use zmux::{
    client::{Client, ExitReporters, PaneSignals},
    messages::{SpawnRequest, TerminalSize},
};

/// The connection shared by every pane in this process.
#[derive(Clone)]
pub(crate) struct MuxRuntime {
    client: Arc<Client>,
    reporters: Arc<ExitReporters>,
    revoke_reporters: Arc<PaneSignals>,
    grant_reporters: Arc<PaneSignals>,
}

impl MuxRuntime {
    /// Connects to the multiplexer, starting one if there is none.
    pub(crate) fn connect() -> Result<Self> {
        let client = Arc::new(Client::connect().context("connecting to the multiplexer")?);
        let subscription = client
            .subscribe()
            .context("subscribing to multiplexer events")?;
        Ok(Self {
            client,
            reporters: subscription.exits,
            revoke_reporters: subscription.revokes,
            grant_reporters: subscription.grants,
        })
    }

    pub(crate) fn client(&self) -> &Arc<Client> {
        &self.client
    }

    pub(crate) fn reporters(&self) -> &Arc<ExitReporters> {
        &self.reporters
    }

    pub(crate) fn revoke_reporters(&self) -> &Arc<PaneSignals> {
        &self.revoke_reporters
    }

    /// Where the multiplexer's offers to hand a pane back are delivered.
    pub(crate) fn grant_reporters(&self) -> &Arc<PaneSignals> {
        &self.grant_reporters
    }

    /// A provider for one pane of `session`.
    ///
    /// Per pane rather than per tab because the caller needs to know which
    /// terminal the multiplexer created, and one provider serving several
    /// panes could not say which answer belonged to which.
    pub(crate) fn provider(&self, session: MuxSession) -> Arc<MuxPtyProvider> {
        Arc::new(MuxPtyProvider {
            runtime: self.clone(),
            session,
            opened: Mutex::new(None),
        })
    }
}

/// The multiplexer session a tab's panes belong to.
///
/// Shared between a tab's panes: the first to spawn creates the session and
/// records its identifier, and the rest join it. Held behind a mutex because
/// panes spawn asynchronously and two may reach the multiplexer at once.
#[derive(Clone, Default)]
pub(crate) struct MuxSession(Arc<Mutex<Option<u64>>>);

impl MuxSession {
    pub(crate) fn id(&self) -> Option<u64> {
        *self.0.lock().unwrap()
    }

    pub(crate) fn set_id(&self, session_id: u64) {
        *self.0.lock().unwrap() = Some(session_id);
    }

    pub(crate) fn from_id(session_id: u64) -> Self {
        Self(Arc::new(Mutex::new(Some(session_id))))
    }
}

/// Opens one pane's PTY through the multiplexer.
pub(crate) struct MuxPtyProvider {
    runtime: MuxRuntime,
    session: MuxSession,
    opened: Mutex<Option<OpenedPane>>,
}

#[derive(Clone, Copy)]
pub(crate) struct OpenedPane {
    pub(crate) session_id: u64,
    pub(crate) pane_id: u64,
}

impl MuxPtyProvider {
    /// What the multiplexer created, once [`PtyProvider::open`] has returned.
    pub(crate) fn opened(&self) -> Option<OpenedPane> {
        *self.opened.lock().unwrap()
    }

    pub(crate) fn runtime(&self) -> &MuxRuntime {
        &self.runtime
    }
}

impl PtyProvider for MuxPtyProvider {
    fn open(&self, request: PtySpawnRequest) -> Result<PtyHandover> {
        let pane = self.runtime.client.spawn(SpawnRequest {
            session_id: self.session.id(),
            // Named so a platform that cannot attach a terminal to a message
            // can duplicate one into this process instead.
            client_process_id: std::process::id(),
            program: request.program,
            args: request.args,
            env: request.env.into_iter().collect(),
            working_directory: request.working_directory,
            // The real size is applied by the first resize, which happens as
            // soon as the pane is laid out. Starting from a conventional
            // terminal size rather than zero keeps a program that inspects it
            // before then from seeing something impossible.
            size: TerminalSize {
                columns: 80,
                lines: 24,
                cell_width: 0,
                cell_height: 0,
            },
        })?;
        self.session.set_id(pane.session_id);
        *self.opened.lock().unwrap() = Some(OpenedPane {
            session_id: pane.session_id,
            pane_id: pane.pane_id,
        });
        Ok(attached_pane_handover(pane))
    }

    fn resize(&self, columns: u16, lines: u16) {
        // Only the multiplexer can resize a console it created. On Unix the
        // resize already happened through the descriptor, so this is a no-op
        // there rather than a second, redundant round trip.
        if cfg!(windows)
            && let Some(opened) = self.opened()
            && let Err(error) =
                self.runtime
                    .client
                    .resize(opened.session_id, opened.pane_id, columns, lines)
        {
            log::debug!("could not resize the multiplexer's terminal: {error:#}");
        }
    }
}

/// Turns what the multiplexer handed over into what a terminal is built from.
pub(crate) fn attached_pane_handover(pane: zmux::client::AttachedPane) -> PtyHandover {
    PtyHandover {
        #[cfg(unix)]
        descriptor: pane.descriptor,
        #[cfg(windows)]
        conout: pane.conout,
        #[cfg(windows)]
        conin: pane.conin,
        child_pid: pane.child_pid,
        replay: pane.replay,
    }
}

/// Where each visible pane's terminal lives in the multiplexer.
///
/// Kept beside the tabs rather than inside them because a pane's identity in
/// the multiplexer outlives the `TerminalPane` that displays it: the same
/// terminal is shown by a new pane after a session is attached in another
/// window.
#[derive(Default)]
pub(crate) struct MuxPanes {
    panes: HashMap<u64, u64>,
    sessions: HashMap<u64, MuxSession>,
}

/// A pane this window is showing in shared mode.
///
/// The pane's terminal reads what the multiplexer relays instead of the pty
/// itself, and input goes back through [`zmux::client::SharedPane::send_input`]
/// so the multiplexer can attribute it. The pane is kept here because the
/// shared connection outlives a render: sizes arrive on it between frames,
/// and the terminal's byte-stream worker owns only a clone of the reader.
pub(crate) struct SharedPaneEntry {
    pub(crate) pane: Arc<zmux::client::SharedPane>,
    pub(crate) mux_pane_id: u64,
}

impl MuxPanes {
    /// The session a tab's panes belong to, creating one if this is the tab's
    /// first pane.
    pub(crate) fn session_for_tab(&mut self, tab_id: u64) -> MuxSession {
        self.sessions.entry(tab_id).or_default().clone()
    }

    pub(crate) fn adopt_session(&mut self, tab_id: u64, session_id: u64) {
        self.sessions
            .insert(tab_id, MuxSession::from_id(session_id));
    }

    pub(crate) fn session_id(&self, tab_id: u64) -> Option<u64> {
        self.sessions.get(&tab_id)?.id()
    }

    /// Whether a tab in this window is already showing this session.
    ///
    /// The reconnect picker needs this: attaching a session the same *process*
    /// already holds is not a join, because the multiplexer recognises its own
    /// holder and hands the terminal straight back — so it would open a second
    /// tab reading the same pty as the first. Two readers of one terminal split
    /// its output between them arbitrarily.
    pub(crate) fn holds_session(&self, session_id: u64) -> bool {
        self.sessions
            .values()
            .any(|session| session.id() == Some(session_id))
    }

    pub(crate) fn record(&mut self, pane_id: u64, mux_pane_id: u64) {
        self.panes.insert(pane_id, mux_pane_id);
    }

    pub(crate) fn mux_pane_id(&self, pane_id: u64) -> Option<u64> {
        self.panes.get(&pane_id).copied()
    }

    pub(crate) fn ids(&self) -> &HashMap<u64, u64> {
        &self.panes
    }

    pub(crate) fn forget_pane(&mut self, pane_id: u64) {
        self.panes.remove(&pane_id);
    }

    pub(crate) fn forget_tab(&mut self, tab_id: u64) {
        self.sessions.remove(&tab_id);
    }
}

#[cfg(test)]
#[path = "tests/mux.rs"]
mod tests;

impl crate::Zetta {
    /// A provider that spawns into `tab_id`'s multiplexer session.
    ///
    /// Returns `None` when the multiplexer cannot be reached, so the pane opens
    /// on a local process instead. A session that cannot be backgrounded is a
    /// smaller failure than a terminal that will not open at all, and the
    /// reason is surfaced rather than swallowed.
    pub(crate) fn mux_provider_for_tab(
        &mut self,
        tab_id: u64,
        cx: &mut gpui::Context<Self>,
    ) -> Option<Arc<MuxPtyProvider>> {
        if self.mux.is_none() {
            match MuxRuntime::connect() {
                Ok(runtime) => self.mux = Some(runtime),
                Err(error) => {
                    self.configuration_error = Some(format!(
                        "Could not reach the session multiplexer, so this terminal cannot be \
                         backgrounded: {error:#}"
                    ));
                    cx.notify();
                    return None;
                }
            }
        }
        let session = self.mux_panes.session_for_tab(tab_id);
        Some(self.mux.as_ref()?.provider(session))
    }

    /// Records what the multiplexer created for a pane, and routes that pane's
    /// exit reports to its terminal.
    ///
    /// Without the second part the terminal would never learn that its process
    /// ended: it reads the real PTY, but only the multiplexer is the child's
    /// parent, and the event loop deliberately retries a failing read rather
    /// than treating it as a hangup.
    pub(crate) fn adopt_mux_pane(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        provider: Option<&MuxPtyProvider>,
        builder: &mut terminal::TerminalBuilder,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(provider) = provider else {
            return;
        };
        if let Some(error) = builder.multiplexer_error() {
            // The terminal opened locally instead, so it works but cannot be
            // backgrounded. Say so once rather than letting the difference
            // surface later as a session that mysteriously will not detach.
            self.pane_output_error = Some(format!(
                "This terminal is running outside the session multiplexer, so it cannot be \
                 backgrounded: {error}"
            ));
        }
        let Some(opened) = provider.opened() else {
            return;
        };
        self.mux_panes.adopt_session(tab_id, opened.session_id);
        self.mux_panes.record(pane_id, opened.pane_id);
        if let Some(events) = builder.take_child_events() {
            provider
                .runtime()
                .reporters()
                .register(opened.pane_id, events);
        }
        self.watch_for_revoke(
            tab_id,
            pane_id,
            opened.session_id,
            opened.pane_id,
            provider.runtime(),
            window,
            cx,
        );
    }

    /// Hands a closed pane back to the multiplexer.
    ///
    /// Dropping the terminal is not enough to tell the multiplexer anything: it
    /// holds its own descriptor for the pane, so the pane stays marked as taken
    /// by this process, nothing drains it, and the program blocks as soon as the
    /// terminal's buffer fills — until this window exits and the multiplexer's
    /// liveness sweep notices. Saying so explicitly is what stops a closed pane
    /// from leaving a wedged shell behind.
    pub(crate) fn release_mux_pane(&mut self, tab_id: u64, pane_id: u64, cx: &mut gpui::App) {
        let Some(mux_pane_id) = self.mux_panes.mux_pane_id(pane_id) else {
            return;
        };
        self.mux_panes.forget_pane(pane_id);
        let Some(runtime) = self.mux.clone() else {
            return;
        };
        // Locally first, and synchronously: a reporter left registered for a
        // pane nobody is showing would deliver that pane's exit into a terminal
        // that has already gone.
        runtime.reporters().forget(mux_pane_id);
        runtime.revoke_reporters().forget(mux_pane_id);
        let Some(session_id) = self.mux_panes.session_id(tab_id) else {
            return;
        };
        // Off the main thread: closing a pane must not wait on the multiplexer's
        // sessions lock, and nothing here depends on the answer.
        cx.background_spawn(async move {
            if let Err(error) = runtime.client().close_pane(session_id, mux_pane_id) {
                log::debug!("could not release pane {mux_pane_id} to the multiplexer: {error:#}");
            }
        })
        .detach();
    }

    /// Registers this pane's grant delivery and takes the terminal back when it
    /// comes.
    ///
    /// The reverse of [`Zetta::watch_for_revoke`], and registered wherever a pane
    /// becomes shared. The multiplexer offers a pane back once its viewers come
    /// down to one, because relaying to a single client is the daemon reading a
    /// terminal the client could read itself — at about a quarter more cost for
    /// sustained output, and with the daemon's own flow control in the way.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn watch_for_grant(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        session_id: u64,
        mux_pane_id: u64,
        runtime: &MuxRuntime,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<crate::Zetta>,
    ) {
        let (grant_tx, grant_rx) = async_channel::unbounded();
        runtime.grant_reporters().register(mux_pane_id, grant_tx);
        cx.spawn_in(window, async move |this, cx| {
            let _ = grant_rx.recv().await;
            this.update_in(cx, |this, window, cx| {
                this.handle_pane_grant(tab_id, pane_id, session_id, mux_pane_id, window, cx);
            })
        })
        .detach();
    }

    /// Registers this pane's revoke delivery and answers it when it comes.
    ///
    /// Another client attaching to the pane while this one holds it makes the
    /// multiplexer ask the holder to hand the terminal over: stop reading the
    /// pty, snapshot the screen, and re-attach as a shared client. The asking
    /// is asynchronous, so this wakes the main thread with a channel rather
    /// than running the handover on the subscription thread.
    ///
    /// Every route to holding a pane exclusively has to come through here —
    /// spawning one *and* reattaching one. A pane that holds the descriptor
    /// without watching for a revoke cannot answer one, so a third window's
    /// attach waits out the multiplexer's whole handover timeout and then fails.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn watch_for_revoke(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        session_id: u64,
        mux_pane_id: u64,
        runtime: &MuxRuntime,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let (revoke_tx, revoke_rx) = async_channel::unbounded();
        runtime.revoke_reporters().register(mux_pane_id, revoke_tx);
        cx.spawn_in(window, async move |this, cx| {
            // The daemon waits only so long for the answer, so the handover
            // has to start promptly. Nothing is ever sent here; the channel
            // is just the arrival of the revoke.
            let _ = revoke_rx.recv().await;
            this.update_in(cx, |this, window, cx| {
                this.handle_pane_revoke(tab_id, pane_id, session_id, mux_pane_id, window, cx);
            })
        })
        .detach();
    }
}
