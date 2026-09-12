//! Panes whose bytes arrive over Mosh.
//!
//! The counterpart to `shared_panes.rs`, for a remote session the user chose
//! the Zosh protocol for. What is the same: the pane is one the multiplexer
//! holds, so its exit is reported over the control connection rather than
//! learned from a pty, and the routing is the shared one.
//!
//! What differs is everything the multiplexer's byte stream would have carried.
//! This window does not hold that stream — see `remote_pane_transport` for why
//! — so:
//!
//! - input and output travel through the Mosh session, not the connection;
//! - a resize goes to the Mosh session, which is what sizes the remote
//!   `zosh-server`'s pty and, through the relay's `SIGWINCH`, the pane itself;
//! - there is no descriptor for the multiplexer to ask this window to hand
//!   over, so no revoke watch;
//! - the size the multiplexer arbitrates between viewers reaches the relay
//!   rather than this window, so nothing here applies one.
//!
//! The session handle has to outlive the terminal it feeds: dropping it ends
//! the Mosh session. That is what makes closing the pane close the link.

use super::*;

use crate::remote_pane_transport::{
    ZoshPaneEntry, ZoshPaneHandle, ZoshPaneStream, ZoshTerminalParts,
};

/// What building a Mosh-carried pane needs that is not the stream itself.
///
/// A bundle because two paths build one — a session being attached, and a pane
/// added to a session already open — and because the terminal settings travel
/// together with the pane they describe.
pub(crate) struct ZoshPaneBuild<'a> {
    pub(crate) title: String,
    pub(crate) cursor_shape: terminal::terminal_settings::CursorShape,
    pub(crate) alternate_scroll: terminal::terminal_settings::AlternateScroll,
    pub(crate) max_scroll_history_lines: Option<usize>,
    pub(crate) window_id: u64,
    pub(crate) working_directory: Option<PathBuf>,
    pub(crate) runtime: &'a MuxRuntime,
    pub(crate) session_id: u64,
    pub(crate) mux_pane_id: u64,
    pub(crate) executor: &'a gpui::BackgroundExecutor,
}

/// Turns one Mosh session into the terminal that shows it.
///
/// No replay: the relay wrote the pane's retained output into the remote
/// emulator before anything was synchronized, so the first frame that arrives
/// already *is* the screen. Passing the replay here as well would paint it
/// twice.
pub(crate) fn build_zosh_pane(
    build: ZoshPaneBuild<'_>,
    stream: ZoshPaneStream,
) -> (TerminalBuilder, Arc<ZoshPaneHandle>) {
    let ZoshPaneBuild {
        title,
        cursor_shape,
        alternate_scroll,
        max_scroll_history_lines,
        window_id,
        working_directory,
        runtime,
        session_id,
        mux_pane_id,
        executor,
    } = build;
    let ZoshTerminalParts {
        reader,
        writer,
        control,
        session,
    } = stream.into_terminal_parts();
    let built = TerminalBuilder::new_byte_stream(
        reader,
        writer,
        title,
        cursor_shape,
        alternate_scroll,
        max_scroll_history_lines,
        window_id,
        executor,
        PathStyle::local(),
    )
    .with_working_directory(working_directory)
    .with_pty_control(control)
    // Pasting an image is a control operation: it stores the bytes with the
    // multiplexer and writes a path into the pane. It goes over the control
    // connection, which a Mosh pane still has. The remote handler is named
    // directly rather than chosen by `handler_for_pane`, because a pane that
    // travels over Mosh is a remote pane by construction and there is no local
    // target to fall back to.
    .with_image_paste_handler(Arc::new(
        crate::background_session_ui::image_paste::RemoteImagePasteHandler::new(
            runtime,
            session_id,
            mux_pane_id,
        ),
    ));
    (built, session)
}

impl Zetta {
    /// Records a Mosh-carried pane and routes its exit report to its terminal.
    ///
    /// The exit is the one thing about such a pane that still arrives over the
    /// control connection: the multiplexer is the process's parent, and the
    /// Mosh session in front of it only ever carried a screen.
    pub(crate) fn register_zosh_pane(
        &mut self,
        ids: MuxPaneIds,
        session: Arc<ZoshPaneHandle>,
        runtime: &MuxRuntime,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (exit_tx, exit_rx) = async_channel::unbounded();
        runtime
            .reporters()
            .register_shared(ids.mux_pane_id, exit_tx);
        self.zosh_panes.insert(
            ids.pane_id,
            ZoshPaneEntry {
                session,
                mux_pane_id: ids.mux_pane_id,
                runtime: runtime.clone(),
            },
        );
        let MuxPaneIds {
            tab_id, pane_id, ..
        } = ids;
        cx.spawn_in(window, async move |this, cx| {
            let Ok(report) = exit_rx.recv().await else {
                return;
            };
            this.update_in(cx, |this, _window, cx| {
                this.route_shared_pane_exit(tab_id, pane_id, report, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Ends a Mosh-carried pane's session, if this pane was one.
    ///
    /// Dropping the handle is what closes the link: this side shuts the Mosh
    /// session down, the remote `zosh-server` goes with it, and the relay that
    /// was the multiplexer's viewer for the pane ends — leaving the pane to
    /// whichever viewer is left, exactly as closing a shared stream does.
    pub(crate) fn drop_zosh_pane(&mut self, pane_id: u64) {
        let Some(entry) = self.zosh_panes.remove(&pane_id) else {
            return;
        };
        entry.runtime.reporters().forget_shared(entry.mux_pane_id);
    }

    /// Whether this pane reads a stream the multiplexer relays rather than a
    /// pty descriptor of its own — over the shared connection or over Mosh.
    ///
    /// The two are the same answer everywhere the question is "does this
    /// window hold a descriptor for it", which is what decides whether a pane
    /// can be checkpointed, handed over, or offered back.
    pub(crate) fn pane_is_relayed(&self, pane_id: u64) -> bool {
        self.shared_panes.contains_key(&pane_id) || self.zosh_panes.contains_key(&pane_id)
    }
}
