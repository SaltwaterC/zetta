//! Shared panes: the ones several windows watch at once, over the connection
//! that stayed open to the multiplexer.
//!
//! A shared pane is arbitrated rather than owned — its size is the smallest of
//! its viewers' and its exit is routed rather than observed — so it needs the
//! registry, the size reporting, and the grant/revoke handovers here.

use super::*;

/// How often a shared pane's task checks for arbitrated sizes. The
/// multiplexer's size events are rare and arrive with output, so this is a
/// wake-up check, not a poll that races anything.
const SHARED_SIZE_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// A reader that yields a replay prefix before the live stream, for a
/// terminal built around a shared pane whose retained output the multiplexer
/// sent with the attachment.
struct ReplayReader {
    pending: Vec<u8>,
    inner: zmux::client::SharedReader,
}

impl std::io::Read for ReplayReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if !self.pending.is_empty() {
            let count = self.pending.len().min(buffer.len());
            buffer[..count].copy_from_slice(&self.pending[..count]);
            self.pending.drain(..count);
            return Ok(count);
        }
        self.inner.read(buffer)
    }
}

/// A writer that sends a shared pane's input to the multiplexer, so every
/// shared client's input is attributed to the client that typed it.
pub(super) struct SharedPaneWriter {
    pub(super) pane: Arc<zmux::client::SharedPane>,
}

impl std::io::Write for SharedPaneWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.pane
            .send_input(buffer)
            .map(|()| buffer.len())
            .map_err(std::io::Error::other)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Builds the terminal's exit status from the multiplexer's report, the way
/// each platform spells one: Unix carries a wait status and Windows an exit
/// code, and the same number does not mean the same thing to both.
fn exit_status_from_raw(raw_status: i32) -> std::process::ExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        std::process::ExitStatus::from_raw(raw_status)
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt as _;
        std::process::ExitStatus::from_raw(raw_status as u32)
    }
}

impl Zetta {
    /// Records a shared pane and starts the task that keeps it in step with
    /// the multiplexer: applying arbitrated sizes and routing the pane's
    /// exit report to its terminal.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn register_shared_pane(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        session_id: u64,
        mux_pane_id: u64,
        pane: &Arc<zmux::client::SharedPane>,
        runtime: &MuxRuntime,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (exit_tx, exit_rx) = async_channel::unbounded();
        runtime.reporters().register_shared(mux_pane_id, exit_tx);
        self.shared_panes.insert(
            pane_id,
            SharedPaneEntry {
                pane: pane.clone(),
                mux_pane_id,
                runtime: runtime.clone(),
            },
        );
        if !runtime.is_remote() {
            // Every local route into shared mode has to come through here, so
            // every shared pane can be offered back when it turns out to be the
            // last viewer. Remote panes are deliberately stream-only.
            self.watch_for_grant(
                tab_id,
                pane_id,
                session_id,
                mux_pane_id,
                runtime,
                window,
                cx,
            );
        }
        self.spawn_shared_pane_task(tab_id, pane_id, pane.clone(), exit_rx, window, cx);
    }

    fn spawn_shared_pane_task(
        &self,
        tab_id: u64,
        pane_id: u64,
        pane: Arc<zmux::client::SharedPane>,
        exit_rx: async_channel::Receiver<zmux::client::PaneExitReport>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let executor = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            // The size the pane attached at has to be applied once the tab
            // exists; without this the grid could stay at the window's size
            // while the multiplexer's pty runs at the arbitrated one.
            let mut pending_size: Option<(u16, u16)> = None;
            loop {
                match exit_rx.try_recv() {
                    Ok(report) => {
                        this.update_in(cx, |this, _window, cx| {
                            this.route_shared_pane_exit(tab_id, pane_id, report, cx);
                        })
                        .ok();
                        return;
                    }
                    Err(async_channel::TryRecvError::Empty) => {}
                    Err(async_channel::TryRecvError::Closed) => return,
                }
                if let Some(size) = pane.take_sizes().last().copied() {
                    pending_size = Some(size);
                }
                let Some(size) = pending_size else {
                    executor.timer(SHARED_SIZE_POLL_INTERVAL).await;
                    continue;
                };
                let applied = this
                    .update_in(cx, |this, window, cx| {
                        this.apply_shared_pane_size(tab_id, pane_id, size, window, cx)
                    })
                    .unwrap_or(true);
                if applied {
                    pending_size = None;
                }
                executor.timer(SHARED_SIZE_POLL_INTERVAL).await;
            }
        })
        .detach();
    }

    /// Applies the size the multiplexer arbitrated for a shared pane, by
    /// resizing the layout to it: the grid is laid out from the pane's
    /// bounds, so the shell's wraps can only line up with the cells drawn
    /// when the two agree.
    fn apply_shared_pane_size(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        (columns, lines): (u16, u16),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let bounds = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane(pane_id))
            .and_then(TerminalPane::selected_terminal)
            .map(|terminal| terminal.read(cx).last_content().terminal_bounds);
        match shared_size_action(bounds, columns, lines) {
            SharedSizeAction::WaitForLayout => false,
            SharedSizeAction::AlreadyMatches => true,
            SharedSizeAction::Resize => {
                self.resize_pane_to(
                    tab_id,
                    pane_id,
                    Some(columns as usize),
                    Some(lines as usize),
                    window,
                    cx,
                );
                true
            }
        }
    }

    /// Routes a shared pane's exit report to its terminal.
    ///
    /// The terminal no longer reads the pty, so it has no event loop to learn
    /// the exit from; the multiplexer is the child's parent and its
    /// attribution of input replaces the terminal's own.
    fn route_shared_pane_exit(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        report: zmux::client::PaneExitReport,
        cx: &mut Context<Self>,
    ) {
        let entry = self.shared_panes.remove(&pane_id);
        let (mux_pane_id, runtime) = entry.as_ref().map_or((None, None), |entry| {
            (Some(entry.mux_pane_id), Some(entry.runtime.clone()))
        });
        if let Some(runtime) = runtime.as_ref()
            && let Some(mux_pane_id) = mux_pane_id
        {
            runtime.reporters().forget_shared(mux_pane_id);
        }
        let Some(terminal) = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane(pane_id))
            .and_then(|pane| pane.terminal.clone())
        else {
            return;
        };
        if report.disconnected {
            // The byte stream already printed the connection error into the
            // grid; the pane stays open on it until the user closes it.
            return;
        }
        terminal.update(cx, |terminal, cx| match report.raw_status {
            Some(raw_status) => {
                terminal.report_child_exit(exit_status_from_raw(raw_status), report.input_sent, cx);
            }
            // The multiplexer saw the process end but could not say how — or the
            // pane was already gone when this client asked what it had missed.
            // Reporting it anyway is what lets the pane be closed: silently
            // dropping the report left a shared terminal waiting for something
            // that had already happened, with nothing on screen to say so.
            None => terminal.report_child_exit_status_unavailable(report.input_sent, cx),
        });
    }

    /// Reports this window's size for a shared pane, so the multiplexer can
    /// arbitrate shared clients down to the smallest of them.
    fn report_shared_pane_size(
        &mut self,
        pane_id: u64,
        terminal: &Entity<Terminal>,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.shared_panes.get(&pane_id) else {
            return;
        };
        let bounds = terminal.read(cx).last_content().terminal_bounds;
        let Some((columns, lines)) = shared_size_to_report(bounds) else {
            // Not laid out yet, so this viewer has no size to arbitrate against.
            // The first layout emits `GridSizeChanged`, which reports it then.
            return;
        };
        if let Err(error) = entry.pane.send_resize(columns, lines) {
            log::debug!("could not report the shared pane's size: {error:#}");
        }
    }

    /// Drops a shared pane: its terminal is going away, so the shared
    /// connection and the exit routing that keep it in step go with it.
    ///
    /// The pane itself stays alive on the multiplexer — dropping the shared
    /// connection only removes this client from the shared set, which is
    /// exactly what closing the window's view of it means.
    pub(crate) fn drop_shared_pane(&mut self, pane_id: u64) {
        let Some(entry) = self.shared_panes.remove(&pane_id) else {
            return;
        };
        let runtime = entry.runtime;
        runtime.reporters().forget_shared(entry.mux_pane_id);
        runtime.revoke_reporters().forget(entry.mux_pane_id);
        // The grant watcher too: an offer to hand back a pane this window no
        // longer shows has nothing to hand back to, and the registration would
        // outlive every other trace of the pane.
        runtime.grant_reporters().forget(entry.mux_pane_id);
    }

    /// The multiplexer offered this pane's terminal back: this window is the only
    /// viewer left, so there is nothing left to relay for.
    ///
    /// The reverse of [`Zetta::handle_pane_revoke`]. The pane keeps its grid, its
    /// scrollback and everything on screen; only what feeds them changes, from the
    /// multiplexer's relay to the terminal itself.
    pub(crate) fn handle_pane_grant(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        session_id: u64,
        mux_pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(runtime) = self
            .shared_panes
            .get(&pane_id)
            .map(|entry| entry.runtime.clone())
            .or_else(|| self.mux_panes.runtime_for_tab(tab_id))
            .or_else(|| self.mux.clone())
        else {
            return;
        };
        if runtime.is_remote() {
            return;
        }
        // Only a pane this window is actually sharing can be taken back, and only
        // while it still has the terminal the grant was offered for.
        if !self.shared_panes.contains_key(&pane_id) {
            return;
        }
        let Some(terminal) = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane(pane_id))
            .and_then(|pane| pane.terminal.clone())
        else {
            return;
        };
        let Some(profile) = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane(pane_id))
            .map(|pane| pane.profile.clone())
        else {
            return;
        };
        let settings = TerminalSpawnSettings::current(cx);
        let options = terminal::AttachedOptions {
            shell: profile.command.clone(),
            env: HashMap::default(),
            cursor_shape: settings.cursor_shape,
            alternate_scroll: settings.alternate_scroll,
            max_scroll_history_lines: settings.max_scroll_history_lines,
            path_hyperlink_regexes: Vec::new(),
            path_hyperlink_timeout_ms: settings.path_hyperlink_timeout_ms,
            window_id: cx.entity_id().as_u64(),
        };
        let client = runtime.client().clone();
        cx.spawn_in(window, async move |this, cx| {
            let taken = cx
                .background_spawn(async move { client.take_exclusive(session_id, mux_pane_id) })
                .await;
            let attached = match taken {
                Ok(attached) => attached,
                Err(error) => {
                    // Declining is always safe: the multiplexer keeps the pane
                    // shared, and offers again the next time the viewers change.
                    log::debug!("could not take pane {mux_pane_id} back: {error:#}");
                    return;
                }
            };
            this.update_in(cx, |this, window, cx| {
                this.complete_grant_conversion(
                    tab_id,
                    pane_id,
                    session_id,
                    mux_pane_id,
                    attached,
                    terminal,
                    options,
                    &runtime,
                    window,
                    cx,
                );
            })
            .ok();
        })
        .detach();
    }

    /// Finishes taking a pane back: its terminal now reads the pty itself.
    #[allow(clippy::too_many_arguments)]
    fn complete_grant_conversion(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        session_id: u64,
        mux_pane_id: u64,
        attached: zmux::client::AttachedPane,
        terminal: Entity<Terminal>,
        options: terminal::AttachedOptions,
        runtime: &MuxRuntime,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let handover = crate::mux::attached_pane_handover_with_secret(
            attached,
            runtime.client().clone(),
            runtime.session_secret(),
        );
        let child_events = match terminal.update(cx, |terminal, cx| {
            terminal.attach_pty(handover, options, cx)
        }) {
            Ok(events) => events,
            Err(error) => {
                // The pane is left as it was — still shared as far as this window
                // is concerned — but the multiplexer has already given the
                // descriptor away, so say so rather than leaving a pane that reads
                // nothing.
                self.pane_output_error = Some(format!(
                    "Could not take this pane's terminal back from the multiplexer: {error:#}"
                ));
                cx.notify();
                return;
            }
        };
        // The shared bookkeeping goes, and with it the shared connection: this
        // window no longer reads a relay. Done after the conversion, so a failure
        // above leaves the pane as it was.
        self.drop_shared_pane(pane_id);
        // Exits now arrive through the pty's own child-event channel again, and a
        // pane holding the descriptor has to be able to answer a future revoke.
        runtime.reporters().register(mux_pane_id, child_events);
        self.watch_for_revoke(
            tab_id,
            pane_id,
            session_id,
            mux_pane_id,
            runtime,
            window,
            cx,
        );
        cx.notify();
    }

    /// The multiplexer asked this window to hand an exclusively attached pane
    /// over: another client attached, and the pane is becoming shared.
    ///
    /// The holder has to stop reading the pty — synchronously, so the
    /// daemon's drain cannot lose output to this window's own loop — snapshot
    /// the grid, and re-attach as a shared client whose terminal reads the
    /// daemon's relay instead.
    pub(crate) fn handle_pane_revoke(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        session_id: u64,
        mux_pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(runtime) = self
            .mux_panes
            .runtime_for_tab(tab_id)
            .or_else(|| self.mux.clone())
        else {
            return;
        };
        if runtime.is_remote() {
            return;
        }
        let Some(terminal) = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane(pane_id))
            .and_then(|pane| pane.terminal.clone())
        else {
            return;
        };
        // The snapshot has to be a stable picture of what the daemon will
        // retain, and the pty loop is the only other reader of the master.
        if terminal
            .update(cx, |terminal, _| terminal.stop_pty_loop())
            .is_err()
        {
            return;
        }
        let (snapshot, columns, lines) = terminal.update(cx, |terminal, _| {
            let bounds = terminal.last_content().terminal_bounds;
            (
                terminal.ansi_snapshot(SNAPSHOT_LINES),
                bounds.num_columns() as u16,
                bounds.num_lines() as u16,
            )
        });
        // Built here, on the main thread, because it reads the live tab. The
        // daemon re-reads the session's state once this handover completes, so
        // sending it below is what makes the joining window rebuild the layout as
        // it is now rather than as it was when sharing was switched on.
        let refresh = self.shared_session_refresh(tab_id, cx);
        let client = runtime.client().clone();
        let terminal_handle = terminal.clone();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = cx
                .background_spawn(async move {
                    if let Some((session_id, summary, state)) = refresh
                        && let Err(error) = client.share(session_id, summary, state, None, true)
                    {
                        // Not fatal: the handover still works, the joining
                        // window just rebuilds an older layout.
                        log::debug!("could not refresh shared session {session_id}: {error:#}");
                    }
                    client.send_snapshot(session_id, mux_pane_id, snapshot, columns, lines)?;
                    // Plain attach: it presents this process's id, so the
                    // daemon can tell the pane's holder is re-attaching. If
                    // the snapshot has not been processed yet the daemon
                    // waits for it before answering.
                    client.attach(session_id, Some(mux_pane_id), None)
                })
                .await
                .ok();
            let Some(zmux::client::AttachOutcome::SharedAttached { pane, .. }) = outcome else {
                // The handover failed; the daemon keeps the pane under revoke
                // until the next attach attempt times out.
                log::debug!("the multiplexer handover of pane {mux_pane_id} did not complete");
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.complete_revoke_conversion(
                    tab_id,
                    pane_id,
                    session_id,
                    mux_pane_id,
                    pane,
                    terminal_handle,
                    &runtime,
                    window,
                    cx,
                );
            })
            .ok();
        })
        .detach();
    }

    /// Finishes the revoke handover: the pane's terminal now reads the
    /// multiplexer's relay, and the pane is registered as shared.
    #[allow(clippy::too_many_arguments)]
    fn complete_revoke_conversion(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        session_id: u64,
        mux_pane_id: u64,
        pane: zmux::client::SharedPane,
        terminal: Entity<Terminal>,
        runtime: &MuxRuntime,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane = Arc::new(pane);
        // The daemon retained what the pane produced while this window's pty
        // loop was stopped; replay it into the grid, which holds the snapshot
        // but not those bytes.
        let reader: Box<dyn std::io::Read + Send> = Box::new(ReplayReader {
            pending: pane.replay.clone(),
            inner: pane.reader(),
        });
        let writer: Box<dyn std::io::Write + Send> =
            Box::new(SharedPaneWriter { pane: pane.clone() });
        if terminal
            .update(cx, |terminal, _| {
                terminal.attach_byte_stream(reader, writer)
            })
            .is_err()
        {
            return;
        }
        runtime.revoke_reporters().forget(mux_pane_id);
        self.register_shared_pane(
            tab_id,
            pane_id,
            session_id,
            mux_pane_id,
            &pane,
            runtime,
            window,
            cx,
        );
        // This pane's view was wired up when it was spawned, long before it
        // became shared, so its size reports have to be subscribed here rather
        // than in `connect_terminal_view`. Without this the pane joins size
        // arbitration silently: the daemon only ever knows the size it was
        // handed over at, so every other viewer is sized against a stale figure.
        self.subscribe_shared_pane_size(pane_id, &terminal, window, cx);
        cx.notify();
    }

    /// Reports this window's grid size for a shared pane whenever it changes.
    ///
    /// A shared pane's grid is laid out by this window, but the pty is the
    /// multiplexer's: the daemon has to be told this window's size so it can
    /// arbitrate every shared client down to the smallest of them. Called from
    /// both routes into shared mode — attaching into it, and being revoked into
    /// it — because a pane that never reports cannot be arbitrated for.
    pub(super) fn subscribe_shared_pane_size(
        &mut self,
        pane_id: u64,
        terminal: &Entity<Terminal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        watch_grid_size(terminal, window, cx, move |this, terminal, cx| {
            this.report_shared_pane_size(pane_id, terminal, cx);
        });
        // The size the pane is at right now, which no later event will repeat:
        // a pane that is already laid out emits `GridSizeChanged` only when it
        // next changes, so without this the daemon would arbitrate against the
        // handover size until the user happened to resize something.
        self.report_shared_pane_size(pane_id, terminal, cx);
    }
}

/// Calls `on_change` whenever `terminal`'s grid size changes.
///
/// The terminal comes from the subscription rather than from a captured handle,
/// and that is the whole point of this existing. GPUI keeps a subscription in
/// the *emitter's* list and drops it only when the emitter is released, so a
/// closure that captures its own emitter is a cycle: the terminal can never be
/// released, however thoroughly its tab is closed. A shared pane's terminal kept
/// alive that way never stops its byte stream, so its relay socket stays open
/// and the multiplexer goes on counting this window among the pane's viewers —
/// which is how unsharing came to be refused for a window that had closed the
/// tab.
pub(crate) fn watch_grid_size<V: 'static>(
    terminal: &Entity<Terminal>,
    window: &mut Window,
    cx: &mut Context<V>,
    mut on_change: impl FnMut(&mut V, &Entity<Terminal>, &mut Context<V>) + 'static,
) {
    cx.subscribe_in(
        terminal,
        window,
        move |view, terminal, event: &TerminalEvent, _window, cx| {
            if let TerminalEvent::GridSizeChanged = event {
                on_change(view, terminal, cx);
            }
        },
    )
    .detach();
}

#[cfg(test)]
#[path = "../tests/background_session_ui/shared_panes.rs"]
mod tests;
