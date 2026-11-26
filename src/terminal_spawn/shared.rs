//! Spawning a pane of a shared session: drafting it on the multiplexer, laying
//! the draft out beside the panes already there, and attaching each viewer's
//! terminal once the daemon has committed it.

use super::*;

#[derive(Clone)]
pub(crate) struct SharedTerminalLaunch {
    pub(super) tab_id: u64,
    pub(super) pane_id: u64,
    pub(super) attention_id: u64,
    pub(super) pane_routing_id: u64,
    pub(super) terminal_theme: Option<Arc<Theme>>,
    pub(super) provider: Arc<crate::mux::MuxPtyProvider>,
    pub(super) shell: Shell,
    pub(super) environment: HashMap<String, String>,
    /// The nonce this pane's shell is given to authenticate its lifecycle
    /// markers; see `shared_draft_process`. Drawn by the window, because the
    /// window's terminal has to expect it and the daemon that starts the
    /// shell cannot tell it. Kept across the one resynchronised retry, which
    /// is safe because a refused draft started no shell.
    pub(super) marker_nonce: Option<String>,
    pub(super) working_directory: Option<PathBuf>,
    pub(super) title: String,
    pub(super) profile_name: String,
    pub(super) cursor_shape: terminal::terminal_settings::CursorShape,
    pub(super) alternate_scroll: terminal::terminal_settings::AlternateScroll,
    pub(super) max_scroll_history_lines: Option<usize>,
    pub(super) shell_integration_startup_command: Option<Vec<u8>>,
    pub(super) tracked_multi_command_launch: bool,
    pub(super) base_revision: zmux::messages::SessionRevision,
    pub(super) console_palette: terminal::ConsolePalette,
    /// Whether this launch has already been resynchronised once. A session
    /// whose daemon was replaced leaves every window proposing pane ids that no
    /// longer exist; the first refusal brings the window up to date and the
    /// launch is made again, but only once, so a session that refuses for some
    /// other reason cannot put the window in a loop.
    pub(super) resynchronized: bool,
}

pub(super) enum SharedTerminalBuild {
    Ready(Box<TerminalBuilder>, Box<SpawnedTerminal>),
    Conflict {
        tab_id: u64,
        pane_id: u64,
        state: Box<zmux::messages::SharedSessionState>,
    },
    /// The session refused the proposed geometry, and a fresh snapshot came
    /// back with it. This window's picture of the session was wrong, so the
    /// snapshot is applied before the draft is given up — otherwise the same
    /// wrong picture produces the same refusal for every later split.
    Desynchronized {
        tab_id: u64,
        pane_id: u64,
        state: Box<zmux::messages::SharedSessionState>,
        reason: String,
    },
}

pub(super) enum SharedTerminalBatchBuild {
    Ready {
        session_id: u64,
        state: zmux::messages::SharedSessionState,
        terminals: Vec<(TerminalBuilder, SpawnedTerminal)>,
    },
    Conflict {
        state: zmux::messages::SharedSessionState,
        panes: Vec<(u64, u64)>,
    },
    /// As [`SharedTerminalBuild::Desynchronized`]: the panes are the session's
    /// now, whatever went wrong here, so the window converges on the committed
    /// state rather than dropping them.
    Desynchronized {
        state: zmux::messages::SharedSessionState,
        panes: Vec<(u64, u64)>,
        reason: String,
    },
}

/// What a shared pane's terminal is built from, whichever way its bytes travel.
///
/// A bundle because the two spawn paths — one pane, and a batch of them — build
/// the same thing from values they hold differently.
pub(super) struct SharedPaneTerminal<'a> {
    pub(super) pane: &'a Arc<zmux::client::SharedPane>,
    pub(super) runtime: &'a crate::mux::MuxRuntime,
    pub(super) title: String,
    pub(super) cursor_shape: terminal::terminal_settings::CursorShape,
    pub(super) alternate_scroll: terminal::terminal_settings::AlternateScroll,
    pub(super) max_scroll_history_lines: Option<usize>,
    pub(super) working_directory: Option<PathBuf>,
    /// What this pane's draft gave its shell; `None` for one that carried no
    /// nonce, whose markers then authenticate nothing.
    pub(super) marker_nonce: Option<String>,
    pub(super) executor: &'a gpui::BackgroundExecutor,
}

/// Builds the byte stream a shared pane's terminal reads.
///
/// Over the multiplexer's own stream unless the session's panes travel over
/// Mosh, in which case this brings up that pane's link and hands back the
/// session to keep: the multiplexer's stream for it is then released with the
/// `SharedPane` the caller drops. A Mosh link that could not be brought up
/// leaves the pane on the multiplexer's stream, and hands back why.
pub(super) fn build_shared_pane_terminal(
    build: SharedPaneTerminal<'_>,
) -> (
    TerminalBuilder,
    Option<Arc<crate::remote_pane_transport::ZoshPaneHandle>>,
    Option<String>,
) {
    let SharedPaneTerminal {
        pane,
        runtime,
        title,
        cursor_shape,
        alternate_scroll,
        max_scroll_history_lines,
        working_directory,
        marker_nonce,
        executor,
    } = build;
    let session_id = pane.session_id();
    let mux_pane_id = pane.pane_id();
    // A pane added to a session whose panes travel over Mosh travels the same
    // way: half a tab on a transport the other half is not on would come apart
    // the moment either one was interrupted.
    let mut streams =
        crate::remote_pane_transport::bootstrap_spawned_pane(runtime, session_id, mux_pane_id);
    if let Some(stream) = streams.take(mux_pane_id) {
        let (built, session) = crate::background_session_ui::zosh_panes::build_zosh_pane(
            crate::background_session_ui::zosh_panes::ZoshPaneBuild {
                title,
                cursor_shape,
                alternate_scroll,
                max_scroll_history_lines,
                window_id: 0,
                working_directory,
                runtime,
                session_id,
                mux_pane_id,
                executor,
            },
            stream,
        );
        // The relay reports this window's Mosh size to the multiplexer. Its
        // attachment viewport is only a bootstrap value and, unlike an SSH
        // shared stream, no later viewport frames arrive here to replace it.
        return (
            built.with_command_marker_nonce(marker_nonce),
            Some(session),
            None,
        );
    }
    let fallback = streams.fallback(mux_pane_id).map(str::to_owned);
    let initial_viewport = pane.take_initial_viewport();
    let built = TerminalBuilder::new_byte_stream(
        Box::new(pane.reader()),
        Box::new(
            crate::background_session_ui::shared_panes::SharedPaneWriter { pane: pane.clone() },
        ),
        title,
        terminal::DisplayOnlySettings {
            cursor_shape,
            alternate_scroll,
            max_scroll_history_lines,
            window_id: 0,
            background_executor: executor,
            path_style: PathStyle::local(),
        },
    )
    .with_raw_output()
    .with_shared_viewport(initial_viewport)
    .with_working_directory(working_directory)
    .with_replay(pane.replay.clone())
    .with_pty_control(crate::mux::mux_pty_control_with_secret(
        runtime.client().clone(),
        session_id,
        mux_pane_id,
        runtime.session_secret(),
    ))
    .with_command_marker_nonce(marker_nonce);
    (built, None, fallback)
}

pub(super) fn build_shared_terminal_batch(
    launches: Vec<SharedTerminalLaunch>,
    runtime: crate::mux::MuxRuntime,
    request: zmux::messages::SharedSpawnBatchRequest,
    terminal_executor: &gpui::BackgroundExecutor,
) -> Result<SharedTerminalBatchBuild> {
    let session_id = request.session_id;
    let client = runtime.client().clone();
    // Taken before the launches are consumed below: every recovery path has to
    // name the drafts, including the ones that run after the loop has started.
    let panes = launches
        .iter()
        .map(|launch| (launch.tab_id, launch.pane_id))
        .collect::<Vec<_>>();
    let result = match client.spawn_shared_batch(request) {
        Ok(result) => result,
        Err(error) => {
            // The session would not take this geometry. Bring back what it
            // actually holds so the window stops proposing the same thing.
            let state = client.shared_snapshot(session_id)?;
            return Ok(SharedTerminalBatchBuild::Desynchronized {
                state,
                panes,
                reason: format!("{error:#}"),
            });
        }
    };
    let mut committed = match result {
        zmux::client::SharedBatchResult::Applied(committed) => committed,
        zmux::client::SharedBatchResult::Conflict(state) => {
            return Ok(SharedTerminalBatchBuild::Conflict { state, panes });
        }
    };
    let mut terminals = Vec::with_capacity(launches.len());
    for launch in launches {
        let mapping = committed
            .mappings
            .iter()
            .find(|mapping| mapping.draft_id == launch.pane_id)
            .with_context(|| format!("shared batch omitted draft pane {}", launch.pane_id))?;
        launch
            .provider
            .record_shared_opened(session_id, mapping.pane_id);
        // Committed already: the panes belong to the session whatever happens
        // here, so a failed attachment converges the window on them instead of
        // dropping panes that are running for every other viewer.
        let pane = match attach_committed_shared_pane(
            &client,
            &runtime,
            session_id,
            mapping.pane_id,
            &mut committed.state,
        ) {
            Ok(pane) => pane,
            Err(error) => {
                return Ok(SharedTerminalBatchBuild::Desynchronized {
                    state: committed.state,
                    panes,
                    reason: format!("{error:#}"),
                });
            }
        };
        let pane = Arc::new(pane);
        let image_paste_handler = crate::background_session_ui::image_paste::handler_for_pane(
            &runtime,
            session_id,
            mapping.pane_id,
            crate::background_session_ui::image_paste::LocalPasteTarget {
                shell: launch.shell.clone(),
                environment: launch.environment.clone(),
                working_directory: launch.working_directory.clone(),
            },
        );
        let (builder, zosh_session, transport_fallback) =
            build_shared_pane_terminal(SharedPaneTerminal {
                pane: &pane,
                runtime: &runtime,
                title: launch.title.clone(),
                cursor_shape: launch.cursor_shape,
                alternate_scroll: launch.alternate_scroll,
                max_scroll_history_lines: launch.max_scroll_history_lines,
                working_directory: launch.working_directory.clone(),
                marker_nonce: launch.marker_nonce.clone(),
                executor: terminal_executor,
            });
        let builder = builder
            .with_image_paste_handler(image_paste_handler)
            .with_init_command_startup_shell(launch.shell);
        terminals.push((
            builder,
            SpawnedTerminal {
                tab_id: launch.tab_id,
                pane_id: launch.pane_id,
                attention_id: launch.attention_id,
                pane_routing_id: launch.pane_routing_id,
                terminal_theme: launch.terminal_theme,
                restore_options: None,
                restored_working_directory: None,
                mux_provider: None,
                shared: Some(SharedSpawn {
                    pane,
                    runtime: runtime.clone(),
                    zosh_session,
                    transport_fallback,
                    state: None,
                }),
                shell_integration_startup_command: launch.shell_integration_startup_command,
                tracked_multi_command_launch: launch.tracked_multi_command_launch,
                image_paste_handler: None,
            },
        ));
    }
    Ok(SharedTerminalBatchBuild::Ready {
        session_id,
        state: committed.state,
        terminals,
    })
}

pub(super) fn attach_committed_shared_pane(
    client: &Arc<zmux::client::Client>,
    runtime: &crate::mux::MuxRuntime,
    session_id: u64,
    pane_id: u64,
    state: &mut zmux::messages::SharedSessionState,
) -> Result<zmux::client::SharedPane> {
    let mut last_error = None;
    for attempt in 0..3 {
        if attempt > 0 {
            *state = client.shared_snapshot(session_id)?;
            anyhow::ensure!(
                state.contains_pane(pane_id),
                "committed shared pane {pane_id} was removed before it could attach"
            );
        }
        match client.attach_shared_with_secret(
            session_id,
            pane_id,
            runtime.session_secret().as_ref(),
        ) {
            Ok(zmux::client::AttachOutcome::SharedAttached { pane, .. }) => return Ok(pane),
            Ok(_) => {
                last_error = Some(anyhow::anyhow!(
                    "committed shared pane did not attach as a shared stream"
                ));
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.expect("three shared attach attempts produce an outcome"))
        .with_context(|| format!("attaching committed shared pane {pane_id}"))
}

/// What a shared pane draft may say about the process it asks for.
///
/// A pane in a shared session is started by the daemon, on the daemon's host.
/// When that host is this machine, this window resolved the profile against the
/// very configuration the daemon would read, and its resolution carries more
/// than a name can — the working-directory tracking wrappers a WSL, MSYS2 or
/// Cygwin profile needs — so it is sent and used as-is.
///
/// When the host is another machine, none of that holds. The program path, the
/// `PATH` around it and the tracking files all name this machine, and sending
/// them is exactly how a Linux client came to start macOS's `/bin/bash` 3.2
/// with a Linux `PATH` in front of it. Only the profile name and the pane's
/// routing identity cross; the daemon resolves the rest.
///
/// Either way the draft also carries `marker_nonce`, the secret the pane's
/// shell integration signs its lifecycle markers with (see `run_command.rs`'s
/// "What may satisfy a dependency"). The daemon starts the shell, but it is
/// this window's terminal that has to recognise the markers, so the window
/// draws the nonce and tells both. It rides only where nothing prints it: the
/// zmux request, over a same-user private socket locally and over `ssh -T`'s
/// encrypted channel to a remote daemon, which puts it in the shell's
/// environment and retains it nowhere. It is never typed into the pane — the
/// integration loader the daemon types is constant text — so no viewer's
/// replay can contain it.
///
/// A remote pane carries it too, deliberately past the `ZETTA_`-only filter:
/// it names nothing about this machine, and without it a `zetta pane wait` on
/// a remote shared pane whose own startup files load Zetta's integration could
/// never succeed. Anyone on that host who could read it from the shell's
/// environment could also type into the pane, so it widens nothing.
///
/// A pane this window did not create — one it joined, or reattached after a
/// restart — was given a nonce it never knew, so its terminal expects none and
/// its markers fail every dependent closed. That is the intended limit, not an
/// oversight: the alternative is trusting what a pane prints.
pub(super) fn shared_draft_process(
    remote: bool,
    shell: &Shell,
    environment: &HashMap<String, String>,
    marker_nonce: Option<&str>,
) -> (
    Option<zetta_profiles::ProfileCommand>,
    HashMap<String, String>,
) {
    let (command, mut environment) = if remote {
        (
            None,
            zmux::messages::shared_draft_environment(environment.clone()),
        )
    } else {
        (
            Some(crate::config::profile_command(shell)),
            environment.clone(),
        )
    };
    // Whatever the window's own environment held is not this pane's.
    environment.remove(terminal::COMMAND_MARKER_NONCE_ENV);
    if let Some(nonce) = marker_nonce {
        if remote {
            // Not `give_command_marker_nonce`: its `WSLENV` entry is for a WSL
            // distribution on this machine, and the shell runs on another.
            environment.insert(
                terminal::COMMAND_MARKER_NONCE_ENV.to_owned(),
                nonce.to_owned(),
            );
        } else {
            terminal::give_command_marker_nonce(shell, &mut environment, nonce);
        }
    }
    (command, environment)
}

/// The size a pane's pty starts at, before any viewer has laid the pane out and
/// reported a real one.
///
/// The daemon arbitrates a shared pane down to the smallest of its viewers, and
/// falls back to this while none of them has measured. A fixed 80x24 meant a
/// new pane's first screens were drawn — and wrapped — at a width no window was
/// showing, so the source pane's own size is used instead: a split leaves the
/// two halves near enough that the reflow, when the real sizes arrive, is
/// small.
pub(super) fn shared_spawn_stand_in_size(
    tab: Option<&Tab>,
    source_pane_id: u64,
    cx: &App,
) -> zmux::messages::TerminalSize {
    let measured = tab
        .and_then(|tab| tab.pane(source_pane_id))
        .and_then(TerminalPane::selected_terminal)
        .map(|terminal| terminal.read(cx).last_content().terminal_bounds)
        .filter(|bounds| bounds.num_columns() > 0 && bounds.num_lines() > 0);
    zmux::messages::TerminalSize {
        columns: measured.map_or(80, |bounds| bounds.num_columns() as u16),
        lines: measured.map_or(24, |bounds| bounds.num_lines() as u16),
        cell_width: 0,
        cell_height: 0,
    }
}

/// Every pane a proposed layout names as already existing.
///
/// These are the ids the session is asked to place, translated from this
/// window's own map. The daemon refuses the whole request if one of them names
/// a pane it does not hold, which is why they are worth checking first.
pub(super) fn shared_draft_layout_existing_ids(
    layout: &zmux::messages::SharedDraftLayout,
    ids: &mut Vec<u64>,
) {
    match layout {
        zmux::messages::SharedDraftLayout::Draft { .. } => {}
        zmux::messages::SharedDraftLayout::Existing { pane_id } => ids.push(*pane_id),
        zmux::messages::SharedDraftLayout::Split { first, second, .. } => {
            shared_draft_layout_existing_ids(first, ids);
            shared_draft_layout_existing_ids(second, ids);
        }
    }
}

pub(super) fn shared_draft_layout(
    layout: &PaneLayout,
    draft_pane_id: u64,
    mux_ids: &HashMap<u64, u64>,
) -> Result<zmux::messages::SharedDraftLayout> {
    Ok(match layout {
        PaneLayout::Pane(pane_id) if *pane_id == draft_pane_id => {
            zmux::messages::SharedDraftLayout::Draft {
                draft_id: draft_pane_id,
            }
        }
        PaneLayout::Pane(pane_id) => zmux::messages::SharedDraftLayout::Existing {
            pane_id: *mux_ids
                .get(pane_id)
                .with_context(|| format!("shared pane {pane_id} has no multiplexer id"))?,
        },
        PaneLayout::Split {
            axis,
            first_ratio,
            first,
            second,
        } => zmux::messages::SharedDraftLayout::Split {
            axis: *axis,
            first_ratio: *first_ratio,
            first: Box::new(shared_draft_layout(first, draft_pane_id, mux_ids)?),
            second: Box::new(shared_draft_layout(second, draft_pane_id, mux_ids)?),
        },
    })
}

pub(super) fn shared_complete_draft_layout(
    layout: &PaneLayout,
    draft_pane_ids: &HashSet<u64>,
    mux_ids: &HashMap<u64, u64>,
) -> Result<zmux::messages::SharedDraftLayout> {
    Ok(match layout {
        PaneLayout::Pane(pane_id) if draft_pane_ids.contains(pane_id) => {
            zmux::messages::SharedDraftLayout::Draft { draft_id: *pane_id }
        }
        PaneLayout::Pane(pane_id) => zmux::messages::SharedDraftLayout::Existing {
            pane_id: *mux_ids
                .get(pane_id)
                .with_context(|| format!("shared pane {pane_id} has no multiplexer id"))?,
        },
        PaneLayout::Split {
            axis,
            first_ratio,
            first,
            second,
        } => zmux::messages::SharedDraftLayout::Split {
            axis: *axis,
            first_ratio: *first_ratio,
            first: Box::new(shared_complete_draft_layout(
                first,
                draft_pane_ids,
                mux_ids,
            )?),
            second: Box::new(shared_complete_draft_layout(
                second,
                draft_pane_ids,
                mux_ids,
            )?),
        },
    })
}

pub(super) fn shared_spawn_replacement(
    layout: &PaneLayout,
    draft_pane_id: u64,
    mux_ids: &HashMap<u64, u64>,
) -> Result<(Option<u64>, zmux::messages::SharedDraftLayout)> {
    match layout {
        PaneLayout::Split { first, second, .. }
            if matches!(first.as_ref(), PaneLayout::Pane(id) if *id == draft_pane_id)
                && matches!(second.as_ref(), PaneLayout::Pane(_)) =>
        {
            let PaneLayout::Pane(target) = second.as_ref() else {
                unreachable!()
            };
            Ok((
                Some(
                    *mux_ids
                        .get(target)
                        .context("shared split target has no mux id")?,
                ),
                shared_draft_layout(layout, draft_pane_id, mux_ids)?,
            ))
        }
        PaneLayout::Split { first, second, .. }
            if matches!(second.as_ref(), PaneLayout::Pane(id) if *id == draft_pane_id)
                && matches!(first.as_ref(), PaneLayout::Pane(_)) =>
        {
            let PaneLayout::Pane(target) = first.as_ref() else {
                unreachable!()
            };
            Ok((
                Some(
                    *mux_ids
                        .get(target)
                        .context("shared split target has no mux id")?,
                ),
                shared_draft_layout(layout, draft_pane_id, mux_ids)?,
            ))
        }
        PaneLayout::Split { first, second, .. } => {
            if first.contains_pane(draft_pane_id) {
                shared_spawn_replacement(first, draft_pane_id, mux_ids)
            } else if second.contains_pane(draft_pane_id) {
                shared_spawn_replacement(second, draft_pane_id, mux_ids)
            } else {
                anyhow::bail!("shared draft pane is not in the tab layout")
            }
        }
        PaneLayout::Pane(_) => Ok((None, shared_draft_layout(layout, draft_pane_id, mux_ids)?)),
    }
}

/// The shared-session half of a [`SpawnedTerminal`].
pub(super) struct SharedSpawn {
    pub(super) pane: Arc<zmux::client::SharedPane>,
    pub(super) runtime: crate::mux::MuxRuntime,
    /// Set when this pane's bytes travel over Mosh rather than over the
    /// multiplexer's own stream. `pane` is still carried, for the identities
    /// every shared session is bookkept by; what changes is what feeds the
    /// terminal, and what is registered for it.
    pub(super) zosh_session: Option<Arc<crate::remote_pane_transport::ZoshPaneHandle>>,
    /// Why this pane of a Zosh session stayed on SSH, when it did.
    pub(super) transport_fallback: Option<String>,
    /// The committed session state to apply once the terminal is installed,
    /// when the spawn itself added the pane.
    pub(super) state: Option<zmux::messages::SharedSessionState>,
}

/// What a spawned pane is registered from: the stream the multiplexer relays it
/// on, the Mosh session carrying it when it has one instead, and the terminal
/// showing it.
pub(super) struct SpawnedSharedPane<'a> {
    pub(super) shared_pane: &'a Arc<zmux::client::SharedPane>,
    pub(super) zosh_session: Option<Arc<crate::remote_pane_transport::ZoshPaneHandle>>,
    pub(super) transport_fallback: Option<String>,
    pub(super) runtime: &'a crate::mux::MuxRuntime,
    pub(super) terminal: &'a Entity<Terminal>,
}

impl Zetta {
    pub(super) fn spawn_shared_terminal(
        &mut self,
        launch: SharedTerminalLaunch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab_id = launch.tab_id;
        self.pending_shared_terminal_launches
            .entry(tab_id)
            .or_default()
            .push(launch);
        if !self.shared_spawn_batch_scheduled.insert(tab_id) {
            return;
        }
        let executor = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            executor.timer(Duration::ZERO).await;
            this.update_in(cx, |this, window, cx| {
                this.shared_spawn_batch_scheduled.remove(&tab_id);
                let launches = this
                    .pending_shared_terminal_launches
                    .remove(&tab_id)
                    .unwrap_or_default();
                if launches.len() == 1 {
                    this.spawn_shared_terminal_now(
                        launches.into_iter().next().expect("one launch was queued"),
                        window,
                        cx,
                    );
                } else if !launches.is_empty() {
                    this.spawn_shared_terminal_batch(launches, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn spawn_shared_terminal_now(
        &mut self,
        launch: SharedTerminalLaunch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Kept so a refusal that turns out to be this window being behind can
        // be answered by making the same launch again, once, against the
        // session as it actually is.
        let retry = (!launch.resynchronized).then(|| SharedTerminalLaunch {
            resynchronized: true,
            ..launch.clone()
        });
        let Some(geometry) = self.shared_spawn_geometry(&launch, cx) else {
            return;
        };
        let key = launch.provider.shared_key();
        let draft = SharedSpawnDraft {
            tab_id: launch.tab_id,
            pane_id: launch.pane_id,
            tracked_multi_command_launch: launch.tracked_multi_command_launch,
        };
        let executor = cx.background_executor().clone();
        let job = SharedSpawnJob {
            launch,
            geometry,
            executor: executor.clone(),
        };
        let build = executor.spawn(async move { job.run() });
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let result = build.await;
                this.update_in(cx, |this, window, cx| {
                    this.finish_shared_spawn(result, draft, key.as_ref(), retry, window, cx);
                })
                .ok();
            })
            .detach();
    }

    /// Where a new shared pane goes, proposed from this window's picture of
    /// the session. `None` once the draft has been given up and reported.
    fn shared_spawn_geometry(
        &mut self,
        launch: &SharedTerminalLaunch,
        cx: &mut Context<Self>,
    ) -> Option<SharedSpawnGeometry> {
        let SharedTerminalLaunch {
            tab_id, pane_id, ..
        } = *launch;
        // Before the geometry is read out of the tab, not after the session has
        // refused it: the proposal is built from this window's translation of
        // its panes, so a translation the session has outlived has to go first.
        self.reconcile_shared_tab(tab_id, cx);
        let spawn_geometry = self
            .tabs
            .by_id(tab_id)
            .with_context(|| format!("shared tab {tab_id} disappeared"))
            .and_then(|tab| {
                shared_spawn_replacement(&tab.layout, pane_id, self.mux_panes.ids()).map(
                    |(target_pane_id, replacement)| {
                        let active_pane = if tab.active_pane == pane_id {
                            Some(zmux::messages::SharedPaneRef::Draft { draft_id: pane_id })
                        } else {
                            self.mux_panes
                                .mux_pane_id(tab.active_pane)
                                .map(|pane_id| zmux::messages::SharedPaneRef::Existing { pane_id })
                        };
                        (target_pane_id, replacement, active_pane)
                    },
                )
            });
        let Ok((target_pane_id, replacement, active_pane)) = spawn_geometry else {
            self.report_pane_spawn_error(
                tab_id,
                pane_id,
                format!(
                    "Could not prepare the shared pane layout: {:#}",
                    spawn_geometry.unwrap_err()
                ),
                cx,
            );
            return None;
        };
        // The proposal is checked against what this window believes the session
        // holds before it is sent, and the window is reconciled with that same
        // belief on the way. A stale translation would otherwise be refused by
        // the daemon for the whole request, and refused again for every split
        // after it, because nothing here would have learned anything.
        let mut named = target_pane_id.into_iter().collect::<Vec<_>>();
        shared_draft_layout_existing_ids(&replacement, &mut named);
        if let Some(zmux::messages::SharedPaneRef::Existing { pane_id }) = active_pane {
            named.push(pane_id);
        }
        let key = launch.provider.shared_key();
        if !key
            .as_ref()
            .is_some_and(|key| self.shared_geometry_is_current(key, &named))
        {
            self.discard_uncommitted_shared_pane(
                tab_id,
                pane_id,
                "The shared session changed while this pane was being created. Try again."
                    .to_owned(),
                cx,
            );
            return None;
        }
        let spawn_tab = self.tabs.by_id(tab_id);
        let stand_in_size = shared_spawn_stand_in_size(
            spawn_tab,
            target_pane_id
                .and_then(|mux_pane_id| {
                    self.mux_panes.local_pane_id_among(
                        spawn_tab
                            .into_iter()
                            .flat_map(|tab| tab.panes.iter().map(|pane| pane.id)),
                        mux_pane_id,
                    )
                })
                .unwrap_or(pane_id),
            cx,
        );
        Some(SharedSpawnGeometry {
            target_pane_id,
            replacement,
            active_pane,
            stand_in_size,
        })
    }

    /// Settles a shared spawn: attaches the committed pane, or brings this
    /// window up to date with the session and retries once, or gives the
    /// draft up.
    fn finish_shared_spawn(
        &mut self,
        result: Result<SharedTerminalBuild>,
        draft: SharedSpawnDraft,
        key: Option<&SharedSessionKey>,
        retry: Option<SharedTerminalLaunch>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(SharedTerminalBuild::Ready(builder, spawned)) => {
                self.finish_terminal_spawn(*builder, *spawned, window, cx);
            }
            Ok(SharedTerminalBuild::Desynchronized {
                tab_id,
                pane_id,
                state,
                reason,
            }) => {
                {
                    log::warn!(
                        "shared session {} refused this window's pane layout: {reason}",
                        state.session_id
                    );
                    // The snapshot first: it is the session as it is, and
                    // applying it is what makes the retry below propose
                    // something the session can actually accept.
                    if let Some(key) = key {
                        self.apply_shared_snapshot(key, *state, window, cx);
                    }
                    let Some(mut retry) = retry else {
                        self.discard_uncommitted_shared_pane(
                            tab_id,
                            pane_id,
                            format!("Could not create the shared pane: {reason}"),
                            cx,
                        );
                        return;
                    };
                    // Rebased on what the session is at now. The revision
                    // the launch was built with belongs to the picture that
                    // has just been replaced.
                    if let Some(revision) = retry.provider.shared_key().and_then(|key| {
                        self.shared_collaboration
                            .state(&key)
                            .map(|state| state.revision)
                    }) {
                        retry.base_revision = revision;
                    }
                    self.spawn_shared_terminal_now(retry, window, cx);
                }
            }
            Ok(SharedTerminalBuild::Conflict {
                tab_id,
                pane_id,
                state,
            }) => {
                if let Some(key) = key {
                    self.apply_shared_snapshot(key, *state, window, cx);
                }
                self.discard_uncommitted_shared_pane(
                    tab_id,
                    pane_id,
                    "The shared layout changed before this pane could be created.".to_owned(),
                    cx,
                );
            }
            Err(error) => {
                self.discard_uncommitted_shared_pane(
                    draft.tab_id,
                    draft.pane_id,
                    format!("Could not create the shared pane: {error:#}"),
                    cx,
                );
                if draft.tracked_multi_command_launch {
                    self.finish_multi_command_launch(window, cx);
                }
            }
        }
    }

    pub(super) fn spawn_shared_terminal_batch(
        &mut self,
        launches: Vec<SharedTerminalLaunch>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab_id = launches[0].tab_id;
        self.reconcile_shared_tab(tab_id, cx);
        let draft_ids = launches
            .iter()
            .map(|launch| launch.pane_id)
            .collect::<HashSet<_>>();
        let layout = self
            .tabs
            .by_id(tab_id)
            .with_context(|| format!("shared tab {tab_id} disappeared"))
            .and_then(|tab| {
                shared_complete_draft_layout(&tab.layout, &draft_ids, self.mux_panes.ids()).map(
                    |layout| {
                        let active = if draft_ids.contains(&tab.active_pane) {
                            zmux::messages::SharedPaneRef::Draft {
                                draft_id: tab.active_pane,
                            }
                        } else {
                            zmux::messages::SharedPaneRef::Existing {
                                pane_id: self
                                    .mux_panes
                                    .mux_pane_id(tab.active_pane)
                                    .expect("non-draft shared pane has a mux id"),
                            }
                        };
                        (layout, active)
                    },
                )
            });
        let Ok((replacement, active_pane)) = layout else {
            let error = layout.unwrap_err();
            for launch in launches {
                self.report_pane_spawn_error(
                    tab_id,
                    launch.pane_id,
                    format!("Could not prepare the shared pane batch: {error:#}"),
                    cx,
                );
            }
            return;
        };
        // A batch replaces the session's whole layout rather than one node of
        // it, so every pane the tab holds is named. That makes it the request
        // most exposed to a translation this window kept for a pane the session
        // dropped — one such id and the session refuses all of it.
        let mut named = Vec::new();
        shared_draft_layout_existing_ids(&replacement, &mut named);
        if let zmux::messages::SharedPaneRef::Existing { pane_id } = active_pane {
            named.push(pane_id);
        }
        let current = launches[0]
            .provider
            .shared_key()
            .is_some_and(|key| self.shared_geometry_is_current(&key, &named));
        if !current {
            for launch in &launches {
                self.discard_uncommitted_shared_pane(
                    tab_id,
                    launch.pane_id,
                    "The shared session changed while these panes were being created. Try again."
                        .to_owned(),
                    cx,
                );
            }
            return;
        }
        let runtime = launches[0].provider.runtime().clone();
        let session_id = launches[0]
            .provider
            .session_id()
            .expect("a shared launch has a session id");
        let base_revision = launches[0].base_revision;
        let remote = runtime.is_remote();
        let client = runtime.client().clone();
        let operation_id = client.next_shared_operation_id();
        let stand_in_size =
            shared_spawn_stand_in_size(self.tabs.by_id(tab_id), launches[0].pane_id, cx);
        // A batch replaces the whole layout rather than splitting one pane, so
        // the directory to fall back to is the tab's active pane's.
        let inherit_directory_from = self
            .tabs
            .by_id(tab_id)
            .and_then(|tab| self.mux_panes.mux_pane_id(tab.active_pane));
        let panes = shared_batch_drafts(&launches, remote, inherit_directory_from, stand_in_size);
        let executor = cx.background_executor().clone();
        let terminal_executor = executor.clone();
        let request = zmux::messages::SharedSpawnBatchRequest {
            session_id,
            base_revision,
            operation_id,
            target_pane_id: None,
            replacement,
            panes,
            active_pane: Some(active_pane),
        };
        let daemon = runtime.daemon();
        let build = executor.spawn(async move {
            build_shared_terminal_batch(launches, runtime, request, &terminal_executor)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = build.await;
            this.update_in(cx, |this, window, cx| {
                this.finish_shared_batch_spawn(result, &daemon, tab_id, draft_ids, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Settles a shared batch: attaches every committed pane, or brings this
    /// window up to date with the session and gives the drafts up.
    fn finish_shared_batch_spawn(
        &mut self,
        result: Result<SharedTerminalBatchBuild>,
        daemon: &crate::mux::MuxDaemon,
        tab_id: u64,
        draft_ids: HashSet<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(SharedTerminalBatchBuild::Ready {
                session_id,
                state,
                mut terminals,
            }) => {
                let key = SharedSessionKey {
                    daemon: daemon.clone(),
                    session_id,
                };
                for (_, spawned) in &terminals {
                    let pane = &spawned.shared.as_ref().expect("batch pane is shared").pane;
                    self.mux_panes.record(spawned.pane_id, pane.pane_id());
                    self.shared_collaboration
                        .record_pane(&key, pane.pane_id(), spawned.pane_id);
                }
                self.apply_shared_snapshot(&key, state, window, cx);
                for (builder, spawned) in terminals.drain(..) {
                    self.finish_terminal_spawn(builder, spawned, window, cx);
                }
            }
            Ok(SharedTerminalBatchBuild::Desynchronized {
                state,
                panes,
                reason,
            }) => {
                log::warn!(
                    "shared session {} refused this window's pane batch: {reason}",
                    state.session_id
                );
                let key = SharedSessionKey {
                    daemon: daemon.clone(),
                    session_id: state.session_id,
                };
                self.apply_shared_snapshot(&key, state, window, cx);
                for (tab_id, pane_id) in panes {
                    self.discard_uncommitted_shared_pane(
                        tab_id,
                        pane_id,
                        "This window's view of the shared session was out of date. It has \
                             been brought up to date — try again."
                            .to_owned(),
                        cx,
                    );
                }
            }
            Ok(SharedTerminalBatchBuild::Conflict { state, panes }) => {
                let key = SharedSessionKey {
                    daemon: daemon.clone(),
                    session_id: state.session_id,
                };
                self.apply_shared_snapshot(&key, state, window, cx);
                for (tab_id, pane_id) in panes {
                    self.discard_uncommitted_shared_pane(
                        tab_id,
                        pane_id,
                        "The shared layout changed before this pane batch committed.".to_owned(),
                        cx,
                    );
                }
            }
            Err(error) => {
                for pane_id in draft_ids {
                    self.discard_uncommitted_shared_pane(
                        tab_id,
                        pane_id,
                        format!("Could not create the shared panes: {error:#}"),
                        cx,
                    );
                }
            }
        }
    }

    /// Registers a pane the multiplexer relays, once its terminal exists.
    ///
    /// Which registry it belongs in follows from how it is fed: a pane carried
    /// over Mosh holds no stream from the multiplexer, and the one it was
    /// attached with is released when the caller drops it — reading the remote
    /// host's output twice would pay for it twice.
    pub(super) fn register_spawned_shared_pane(
        &mut self,
        ids: MuxPaneIds,
        pane: SpawnedSharedPane<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SpawnedSharedPane {
            shared_pane,
            zosh_session,
            transport_fallback,
            runtime,
            terminal,
        } = pane;
        let tab_id = ids.tab_id;
        let pane_id = ids.pane_id;
        match zosh_session {
            Some(session) => self.register_zosh_pane(ids, session, runtime, window, cx),
            None => {
                if let Some(reason) = transport_fallback {
                    self.note_transport_fallback(tab_id, pane_id, &reason, cx);
                }
                self.register_shared_pane(ids, shared_pane, runtime, window, cx);
                // A pane spawned into a shared session is wired up here rather
                // than in `connect_terminal_view`, which is what subscribes the
                // panes an *attach* builds — so without this a pane you split
                // never tells the daemon how big it is. The pty then keeps the
                // stand-in size the draft was created with for the rest of its
                // life: the pane draws at 24 lines in a full-height window, and
                // maximizing it changes nothing, because the size this viewer
                // is showing was never reported.
                self.subscribe_shared_pane_size(pane_id, terminal, window, cx);
            }
        }
        // Geometry and pane metadata were committed in the batch. Once the
        // terminal exists, publish the remaining opaque PaneState that needs
        // its stable local-to-mux mapping.
        self.sync_shared_tab_state(tab_id, cx);
    }
}

/// The draft a shared spawn is for, which a failure has to give up.
#[derive(Clone, Copy)]
pub(super) struct SharedSpawnDraft {
    tab_id: u64,
    pane_id: u64,
    tracked_multi_command_launch: bool,
}

/// Where a new shared pane goes in the session's layout, decided on the
/// window's thread before the request leaves it.
pub(super) struct SharedSpawnGeometry {
    target_pane_id: Option<u64>,
    replacement: zmux::messages::SharedDraftLayout,
    active_pane: Option<zmux::messages::SharedPaneRef>,
    stand_in_size: zmux::messages::TerminalSize,
}

/// What a shared spawn carries off the window's thread: the request to the
/// daemon and the attach that follows it both block.
pub(super) struct SharedSpawnJob {
    launch: SharedTerminalLaunch,
    geometry: SharedSpawnGeometry,
    executor: gpui::BackgroundExecutor,
}

impl SharedSpawnJob {
    fn run(self) -> Result<SharedTerminalBuild> {
        let SharedSpawnJob {
            launch:
                SharedTerminalLaunch {
                    tab_id,
                    pane_id,
                    attention_id,
                    pane_routing_id,
                    terminal_theme,
                    provider,
                    shell,
                    environment,
                    marker_nonce,
                    working_directory,
                    title,
                    profile_name,
                    cursor_shape,
                    alternate_scroll,
                    max_scroll_history_lines,
                    shell_integration_startup_command,
                    tracked_multi_command_launch,
                    base_revision,
                    console_palette,
                    resynchronized: _,
                },
            geometry:
                SharedSpawnGeometry {
                    target_pane_id,
                    replacement,
                    active_pane,
                    stand_in_size,
                },
            executor: terminal_executor,
        } = self;
        let remote = provider.runtime().is_remote();
        let (draft_command, draft_environment) =
            shared_draft_process(remote, &shell, &environment, marker_nonce.as_deref());
        // Loaded by the daemon, before the pane has an attachment or anything
        // retained. The wrapper that loads it is delivered as if typed, so the
        // terminal echoes it to every viewer of a shared pane — and a viewer
        // that did not write it has no way to know what it is looking at.
        let daemon_loads_shell_integration = shell_integration_startup_command.is_some();
        let shell_integration_startup_command = None;
        let runtime = provider.runtime().clone();
        let startup_shell = shell.clone();
        let (program, _args) = shell.program_and_args();
        let client = runtime.client().clone();
        let operation_id = client.next_shared_operation_id();
        let session_id = provider
            .session_id()
            .context("shared pane has no multiplexer session")?;
        let request = zmux::messages::SharedSpawnBatchRequest {
            session_id,
            base_revision,
            operation_id,
            target_pane_id,
            replacement,
            panes: vec![zmux::messages::SharedPaneDraft {
                draft_id: pane_id,
                profile: profile_name.clone(),
                command: draft_command.clone(),
                env: draft_environment.clone(),
                working_directory: working_directory.clone(),
                // The pane being split. Only used when this window had no
                // directory of its own to send, which is the case for a
                // pane on another machine: it reports its directory to the
                // window that runs it, not to this one.
                inherit_working_directory_from: target_pane_id,
                load_shell_integration: daemon_loads_shell_integration,
                size: stand_in_size,
                console_palette,
                metadata: BackgroundPaneSummary {
                    id: 0,
                    label: title.clone(),
                    profile: profile_name,
                    configured_command: String::new(),
                    application: program,
                    foreground_command: None,
                    terminal_title: None,
                    working_directory: working_directory.clone(),
                    state: BackgroundPaneState::Running,
                    exit: None,
                },
            }],
            active_pane,
        };
        let spawned = match client.spawn_shared_batch(request) {
            Ok(spawned) => spawned,
            Err(error) => {
                // The session would not take the geometry. Whatever this
                // window believed about the session was wrong, so bring
                // back what is actually there rather than reporting and
                // leaving the same belief in place to fail again.
                let Ok(state) = client.shared_snapshot(session_id) else {
                    return Err(error);
                };
                return Ok(SharedTerminalBuild::Desynchronized {
                    tab_id,
                    pane_id,
                    state: Box::new(state),
                    reason: format!("{error:#}"),
                });
            }
        };
        let spawned = match spawned {
            zmux::client::SharedBatchResult::Applied(spawned) => spawned,
            zmux::client::SharedBatchResult::Conflict(state) => {
                return Ok::<_, anyhow::Error>(SharedTerminalBuild::Conflict {
                    tab_id,
                    pane_id,
                    state: Box::new(state),
                });
            }
        };
        let mapping = spawned
            .mappings
            .iter()
            .find(|mapping| mapping.draft_id == pane_id)
            .context("shared batch did not commit its requested pane")?;
        provider.record_shared_opened(session_id, mapping.pane_id);
        let mut committed_state = spawned.state;
        // Past this point the pane exists for every viewer, so a failure
        // here is this window's alone. Handing back the committed state
        // rather than an error is what lets the pane arrive the ordinary
        // way — as one the session holds and this window has yet to
        // attach — instead of being dropped locally and left running.
        let pane = match attach_committed_shared_pane(
            &client,
            &runtime,
            session_id,
            mapping.pane_id,
            &mut committed_state,
        ) {
            Ok(pane) => pane,
            Err(error) => {
                return Ok(SharedTerminalBuild::Desynchronized {
                    tab_id,
                    pane_id,
                    state: Box::new(committed_state),
                    reason: format!("{error:#}"),
                });
            }
        };
        let pane = Arc::new(pane);
        let image_paste_handler = crate::background_session_ui::image_paste::handler_for_pane(
            &runtime,
            pane.session_id(),
            pane.pane_id(),
            crate::background_session_ui::image_paste::LocalPasteTarget {
                shell,
                environment,
                working_directory: working_directory.clone(),
            },
        );
        let (builder, zosh_session, transport_fallback) =
            build_shared_pane_terminal(SharedPaneTerminal {
                pane: &pane,
                runtime: &runtime,
                title,
                cursor_shape,
                alternate_scroll,
                max_scroll_history_lines,
                working_directory,
                marker_nonce,
                executor: &terminal_executor,
            });
        let builder = builder
            .with_image_paste_handler(image_paste_handler)
            .with_init_command_startup_shell(startup_shell);
        Ok::<_, anyhow::Error>(SharedTerminalBuild::Ready(
            Box::new(builder),
            Box::new(SpawnedTerminal {
                tab_id,
                pane_id,
                attention_id,
                pane_routing_id,
                terminal_theme,
                restore_options: None,
                restored_working_directory: None,
                mux_provider: None,
                shared: Some(SharedSpawn {
                    pane,
                    runtime,
                    zosh_session,
                    transport_fallback,
                    state: Some(committed_state),
                }),
                shell_integration_startup_command,
                tracked_multi_command_launch,
                image_paste_handler: None,
            }),
        ))
    }
}

/// The pane drafts a shared batch proposes, one per launch, all started at
/// the same stand-in size until each viewer reports its own.
fn shared_batch_drafts(
    launches: &[SharedTerminalLaunch],
    remote: bool,
    inherit_directory_from: Option<u64>,
    stand_in_size: zmux::messages::TerminalSize,
) -> Vec<zmux::messages::SharedPaneDraft> {
    launches
        .iter()
        .map(|launch| {
            let (program, _args) = launch.shell.program_and_args();
            let (command, env) = shared_draft_process(
                remote,
                &launch.shell,
                &launch.environment,
                launch.marker_nonce.as_deref(),
            );
            zmux::messages::SharedPaneDraft {
                draft_id: launch.pane_id,
                profile: launch.profile_name.clone(),
                command,
                env,
                working_directory: launch.working_directory.clone(),
                inherit_working_directory_from: inherit_directory_from,
                load_shell_integration: launch.shell_integration_startup_command.is_some(),
                size: stand_in_size,
                console_palette: launch.console_palette,
                metadata: BackgroundPaneSummary {
                    id: 0,
                    label: launch.title.clone(),
                    profile: launch.profile_name.clone(),
                    configured_command: String::new(),
                    application: program,
                    foreground_command: None,
                    terminal_title: None,
                    working_directory: launch.working_directory.clone(),
                    state: BackgroundPaneState::Running,
                    exit: None,
                },
            }
        })
        .collect()
}
