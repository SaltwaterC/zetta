//! Creating a shared session: drafting its panes provisionally, laying them
//! out, and committing the batch only once every pane has started, so a
//! viewer never sees a session with a pane that failed.

use super::*;

pub(super) struct ProvisionalSharedPane {
    pane_id: u64,
    pty: tty::Pty,
    #[cfg(windows)]
    console_id: u64,
    #[cfg(windows)]
    child_events: tty::AttachedChildEvents,
    size: TerminalSize,
}

/// What a draft runs on *this* host, and the environment it runs in.
///
/// The name is resolved here rather than by the requester because this is the
/// machine the process starts on. An unknown name falls back to the login
/// shell, which is what the "System" profile means everywhere else, so a viewer
/// naming a profile this host has never heard of still gets a working shell
/// rather than a failed spawn.
///
/// The terminal environment is applied here for the same reason: a daemon is a
/// background process with no `TERM` of its own, and a pane that inherited that
/// came up believing its terminal had no colour.
pub(super) fn shared_draft_process(
    draft: &crate::messages::SharedPaneDraft,
    #[cfg_attr(
        windows,
        expect(
            unused_variables,
            reason = "on Windows the pane's agent pipe name comes from the listener that serves it"
        )
    )]
    pane_id: u64,
) -> (zetta_profiles::ProfileCommand, HashMap<String, String>) {
    let command = draft
        .command
        .clone()
        .or_else(|| {
            zetta_profiles::resolve(
                &crate::paths::platform_config_dir().join("config.json"),
                &draft.profile,
            )
        })
        .unwrap_or_else(|| {
            if !draft.profile.is_empty() {
                log::debug!(
                    "shared pane asked for profile {:?}, which this host does not have; \
                     starting the login shell",
                    draft.profile
                );
            }
            zetta_profiles::ProfileCommand::system()
        });
    // Local viewers already prepared their shell arguments. Remote viewers only
    // name a profile, so install the same tracker here on the executing host.
    // Its prompt synchronizes PowerShell's location with the native process CWD
    // that pane_working_directory reads; Set-Location alone leaves that stale.
    #[cfg(windows)]
    let command = {
        let mut command = command;
        if command.shell_kind() == zetta_profiles::ShellKind::PowerShell {
            zetta_profiles::install_powershell_tracking(&mut command.args);
        }
        command
    };
    let mut env = draft.env.clone();
    for name in zetta_profiles::REMOVED_TERMINAL_ENVIRONMENT {
        env.remove(*name);
    }
    for (name, value) in
        zetta_profiles::terminal_environment(zetta_profiles::TerminalEnvironmentOptions {
            version: env!("CARGO_PKG_VERSION"),
        })
    {
        env.insert(name, value);
    }
    if std::env::var_os("LANG").is_none() {
        env.entry("LANG".to_owned())
            .or_insert_with(|| zetta_profiles::FALLBACK_LANG.to_owned());
    }
    #[cfg(unix)]
    env.insert(
        "SSH_AUTH_SOCK".to_owned(),
        crate::paths::pane_forwarded_agent_socket(pane_id)
            .to_string_lossy()
            .into_owned(),
    );
    (command, env)
}

pub(super) fn start_shared_draft(
    daemon: &Arc<Daemon>,
    draft: &crate::messages::SharedPaneDraft,
) -> Result<ProvisionalSharedPane> {
    let pane_id = daemon.next_pane_id.fetch_add(1, Ordering::Relaxed);
    #[cfg(unix)]
    let agent_fallback = pane_agent_fallback(&draft.env);
    #[cfg(unix)]
    let (command, env) = shared_draft_process(draft, pane_id);
    #[cfg(windows)]
    let (command, mut env) = shared_draft_process(draft, pane_id);
    #[cfg(unix)]
    prepare_pane_agent_links(pane_id, &agent_fallback);
    // Before the shell starts, so its first `ssh` finds the pipe listening.
    #[cfg(windows)]
    {
        let agent_fallback = pane_agent_fallback(&draft.env);
        match super::agent_pipe::serve(pane_id, agent_fallback.clone()) {
            Some(pipe) => {
                env.insert(
                    "SSH_AUTH_SOCK".to_owned(),
                    pipe.to_string_lossy().into_owned(),
                );
            }
            // Without a pipe of its own the pane keeps the agent it would have
            // had, rather than a name nothing serves.
            None => match agent_fallback {
                Some(agent) => {
                    env.insert(
                        "SSH_AUTH_SOCK".to_owned(),
                        agent.to_string_lossy().into_owned(),
                    );
                }
                None => {
                    env.remove("SSH_AUTH_SOCK");
                }
            },
        }
    }
    #[cfg(unix)]
    let bootstrap_command = command.clone();
    let working_directory = pane_start_directory(draft.working_directory.clone().or_else(|| {
        draft
            .inherit_working_directory_from
            .and_then(|pane_id| pane_working_directory(daemon, pane_id))
    }));
    #[cfg(windows)]
    let (program, args) = (command.program.clone(), command.args.clone());
    #[cfg(unix)]
    let options = tty::Options {
        shell: command
            .program
            .map(|program| tty::Shell::new(program, command.args)),
        working_directory: working_directory.clone(),
        drain_on_exit: true,
        env,
        #[cfg(not(windows))]
        child_signal_mask: None,
        #[cfg(windows)]
        escape_args: false,
    };
    #[cfg(unix)]
    let pty = start_terminal(daemon, &options, draft.size)
        .context("starting a shared terminal process")?;
    // Before the pane is attachable and before anything is retained, so the
    // exchange reaches no viewer and appears in no replay.
    #[cfg(unix)]
    if draft.load_shell_integration
        && let Err(error) = load_shell_integration(&pty, &bootstrap_command)
    {
        log::debug!("could not load the shell integration for a shared pane: {error:#}");
    }
    #[cfg(unix)]
    let _child_pid = pty.child_pid();
    #[cfg(windows)]
    let (console_id, _child_pid, pty, child_events) = {
        let (console_id, child_pid, handles) =
            daemon.pty_host.open(crate::pty_host::OpenConsoleRequest {
                program,
                args,
                env,
                working_directory: working_directory.clone(),
                size: draft.size,
                palette: draft.console_palette,
                console_backend: command.console_backend,
                target_process_id: std::process::id(),
            })?;
        let mut handles = crate::transport::claim_duplicated(&handles);
        if handles.len() != 2 {
            let _ = daemon.pty_host.close(console_id);
            anyhow::bail!("the pseudoconsole host did not return two pipe handles");
        }
        let conin = handles.remove(1);
        let conout = handles.remove(0);
        match tty::attach(conout, conin, child_pid) {
            Ok((pty, child_events)) => (console_id, child_pid, pty, child_events),
            Err(error) => {
                let _ = daemon.pty_host.close(console_id);
                return Err(error).context("attaching the shared pseudoconsole to the daemon");
            }
        }
    };
    Ok(ProvisionalSharedPane {
        pane_id,
        pty,
        #[cfg(windows)]
        console_id,
        #[cfg(windows)]
        child_events,
        size: draft.size,
    })
}

#[cfg_attr(
    not(windows),
    expect(
        unused_variables,
        reason = "only Windows pty hosts need the daemon to close"
    )
)]
pub(super) fn close_provisional_shared_panes(
    daemon: &Arc<Daemon>,
    panes: Vec<ProvisionalSharedPane>,
) {
    #[cfg(windows)]
    for pane in &panes {
        let _ = daemon.pty_host.close(pane.console_id);
    }
    drop(panes);
}

pub(super) fn resolve_draft_layout(
    layout: &crate::messages::SharedDraftLayout,
    mappings: &HashMap<u64, u64>,
) -> Result<BackgroundPaneLayout> {
    use crate::messages::SharedDraftLayout;
    match layout {
        SharedDraftLayout::Existing { pane_id } => {
            Ok(BackgroundPaneLayout::Pane { pane_id: *pane_id })
        }
        SharedDraftLayout::Draft { draft_id } => Ok(BackgroundPaneLayout::Pane {
            pane_id: *mappings
                .get(draft_id)
                .with_context(|| format!("layout references unknown draft {draft_id}"))?,
        }),
        SharedDraftLayout::Split {
            axis,
            first_ratio,
            first,
            second,
        } => {
            anyhow::ensure!(
                *first_ratio > 0
                    && *first_ratio < crate::protocol::BACKGROUND_PANE_SPLIT_RATIO_SCALE,
                "invalid split ratio"
            );
            Ok(BackgroundPaneLayout::Split {
                axis: *axis,
                first_ratio: *first_ratio,
                first: Box::new(resolve_draft_layout(first, mappings)?),
                second: Box::new(resolve_draft_layout(second, mappings)?),
            })
        }
    }
}

pub(super) fn ensure_draft_layout(layout: &crate::messages::SharedDraftLayout) -> Result<()> {
    use crate::messages::SharedDraftLayout;
    match layout {
        SharedDraftLayout::Existing { .. } => {
            anyhow::bail!("a headless session layout cannot contain an existing pane")
        }
        SharedDraftLayout::Draft { .. } => Ok(()),
        SharedDraftLayout::Split { first, second, .. } => {
            ensure_draft_layout(first)?;
            ensure_draft_layout(second)
        }
    }
}

pub(super) fn layout_pane_ids(layout: &BackgroundPaneLayout, ids: &mut Vec<u64>) {
    match layout {
        BackgroundPaneLayout::Pane { pane_id } => ids.push(*pane_id),
        BackgroundPaneLayout::Split { first, second, .. } => {
            layout_pane_ids(first, ids);
            layout_pane_ids(second, ids);
        }
    }
}

pub(super) fn replace_layout_target(
    layout: &mut BackgroundPaneLayout,
    target: u64,
    replacement: BackgroundPaneLayout,
) -> bool {
    match layout {
        BackgroundPaneLayout::Pane { pane_id } if *pane_id == target => {
            *layout = replacement;
            true
        }
        BackgroundPaneLayout::Pane { .. } => false,
        BackgroundPaneLayout::Split { first, second, .. } => {
            if replace_layout_target(first, target, replacement.clone()) {
                true
            } else {
                replace_layout_target(second, target, replacement)
            }
        }
    }
}

/// Forgets the sizes viewers reported against the layout the session has just
/// moved on from. A window measured its pane in that layout, and re-reports
/// for the new one once it has laid it out.
///
/// A relay's report is kept. It is the size of a real terminal that Mosh resizes
/// whenever the window's pane changes, and the relay reports again on every
/// `SIGWINCH` — but on nothing else, because it is told nothing about revisions.
/// Forgetting it left the relay unmeasured after any edit to the session, a
/// focus change included, so the pane fell back to whatever size it was last
/// applied at: once a smaller viewer left, the window that remained stayed at
/// the departed viewer's size.
pub(super) fn clear_shared_size_reports(session: &mut Session) {
    for pane in &mut session.panes {
        if let Attachment::Shared(clients) = &mut pane.attachment {
            for client in clients
                .iter_mut()
                .filter(|client| client.relaying_for.is_none())
            {
                client.size = None;
            }
        }
    }
}

pub(super) enum SharedBatchCommit {
    Applied {
        state: crate::messages::SharedSessionState,
        removed: Vec<u64>,
        mappings: Vec<crate::messages::SharedDraftMapping>,
    },
    Retry {
        state: crate::messages::SharedSessionState,
        mappings: Vec<crate::messages::SharedDraftMapping>,
    },
    Conflict(crate::messages::SharedSessionState),
}

/// Creates a complete shared session without involving a window-owned session.
///
/// All processes are started before the new session is published. This keeps a
/// malformed layout or an unavailable profile from leaving a half-created
/// session in the catalog, which is especially important for a remote CLI that
/// cannot repair the failure through a Zetta process-control request.
pub(in crate::server) fn create_shared(
    daemon: &Arc<Daemon>,
    request: crate::messages::CreateSharedRequest,
    peer_process_id: Option<u32>,
    connection: &mut Connection,
) -> Result<()> {
    validate_create_request(&request)?;

    {
        let sessions = daemon.sessions.lock().recover_poison();
        if let Some(state) = sessions.iter().find_map(|session| {
            session
                .shared_state
                .as_ref()
                .filter(|state| state.receipt(&request.operation_id).is_some())
                .cloned()
        }) {
            return connection.send(&Response::SharedCreated {
                session_id: state.session_id,
                state,
            });
        }
    }

    let authentication = request
        .verifier
        .clone()
        .map(SessionAuthentication::from_verifier)
        .transpose()?;
    let mut provisional = Vec::with_capacity(request.panes.len());
    for draft in &request.panes {
        match start_shared_draft(daemon, draft) {
            Ok(pane) => provisional.push(pane),
            Err(error) => {
                close_provisional_shared_panes(daemon, provisional);
                return Err(error);
            }
        }
    }

    let draft_mappings = request
        .panes
        .iter()
        .zip(&provisional)
        .map(|(draft, pane)| crate::messages::SharedDraftMapping {
            draft_id: draft.draft_id,
            pane_id: pane.pane_id,
        })
        .collect::<Vec<_>>();
    let mappings = draft_mappings
        .iter()
        .map(|mapping| (mapping.draft_id, mapping.pane_id))
        .collect::<HashMap<_, _>>();
    let replacement = match resolve_draft_layout(&request.replacement, &mappings) {
        Ok(layout) => layout,
        Err(error) => {
            close_provisional_shared_panes(daemon, provisional);
            return Err(error);
        }
    };
    let active_pane = match request.active_pane {
        Some(crate::messages::SharedPaneRef::Draft { draft_id }) => *mappings
            .get(&draft_id)
            .with_context(|| format!("active pane references unknown draft {draft_id}"))?,
        Some(crate::messages::SharedPaneRef::Existing { .. }) => {
            close_provisional_shared_panes(daemon, provisional);
            anyhow::bail!("a headless session cannot activate an existing pane")
        }
        None => provisional
            .first()
            .map(|pane| pane.pane_id)
            .context("a headless session must contain a pane")?,
    };

    let mut sessions = daemon.sessions.lock().recover_poison();
    if let Some(state) = sessions.iter().find_map(|session| {
        session
            .shared_state
            .as_ref()
            .filter(|state| state.receipt(&request.operation_id).is_some())
            .cloned()
    }) {
        drop(sessions);
        close_provisional_shared_panes(daemon, provisional);
        return connection.send(&Response::SharedCreated {
            session_id: state.session_id,
            state,
        });
    }

    let session_id = daemon.next_session_id.fetch_add(1, Ordering::Relaxed);
    let mut summary = crate::protocol::BackgroundSessionSummary {
        id: session_id,
        title: request.title,
        authentication_required: authentication.is_some(),
        active_pane,
        layout: replacement,
        panes: Vec::with_capacity(request.panes.len()),
        held: false,
        scoped_to: None,
        key_envelope: None,
    };
    let retention = *daemon.retention.lock().recover_poison();
    let mut panes = Vec::with_capacity(provisional.len());
    for (draft, pane) in request.panes.iter().zip(provisional.drain(..)) {
        let mut metadata = draft.metadata.clone();
        metadata.id = pane.pane_id;
        metadata.state = crate::protocol::BackgroundPaneState::Running;
        metadata.exit = None;
        summary.panes.push(metadata);
        panes.push(Pane {
            id: pane.pane_id,
            pty: pane.pty,
            #[cfg(windows)]
            console_id: pane.console_id,
            #[cfg(windows)]
            child_events: pane.child_events,
            attachment: Attachment::Shared(Vec::new()),
            attachment_client_id: None,
            size: pane.size,
            retained: retention.new_retained(pane.size.columns, pane.size.lines),
            handed_over: None,
            handover_waiters: 0,
            exited: false,
            exit_status: None,
            pending_input: Vec::new(),
            clipboard_scanner: zclip::protocol::Scanner::default(),
        });
    }

    let mut state = crate::messages::SharedSessionState::new(
        session_id,
        summary.clone(),
        serde_json::Value::Null,
    );
    state.last_operation_id = Some(request.operation_id.clone());
    state.record_receipt(crate::messages::SharedOperationReceipt {
        operation_id: request.operation_id,
        draft_mappings,
    });
    summary = state.summary.clone();
    let mut new_session = Session {
        id: session_id,
        summary,
        state: serde_json::Value::Null,
        shared_state: Some(state.clone()),
        authentication,
        key_envelope: None,
        failed_authentications: 0,
        refuse_until: None,
        panes,
        // A headless create has no window whose lifetime could keep it alive;
        // it must outlive the CLI or remote picker that requested it.
        keep: true,
        offered: true,
        owner: None,
        sealed_to: None,
    };
    pin_protected(daemon, &mut new_session, peer_process_id);
    #[cfg(feature = "session-persistence")]
    if daemon.persistence.enabled() {
        persist_live_session(daemon, &new_session)?;
    }
    sessions.push(new_session);
    drop(sessions);
    publish(daemon);
    wake_drain(daemon);
    connection.send(&Response::SharedCreated { session_id, state })
}

pub(super) fn validate_create_request(
    request: &crate::messages::CreateSharedRequest,
) -> Result<()> {
    anyhow::ensure!(
        !request.panes.is_empty(),
        "a headless session must contain a pane"
    );
    ensure_draft_layout(&request.replacement)?;
    let mut draft_ids = request
        .panes
        .iter()
        .map(|pane| pane.draft_id)
        .collect::<Vec<_>>();
    draft_ids.sort_unstable();
    anyhow::ensure!(
        draft_ids.windows(2).all(|pair| pair[0] != pair[1]),
        "headless draft pane IDs must be unique"
    );
    let draft_mapping = request
        .panes
        .iter()
        .enumerate()
        .map(|(index, pane)| (pane.draft_id, index as u64 + 1))
        .collect::<HashMap<_, _>>();
    let layout = resolve_draft_layout(&request.replacement, &draft_mapping)?;
    let mut layout_ids = Vec::new();
    layout_pane_ids(&layout, &mut layout_ids);
    layout_ids.sort_unstable();
    let expected = (1..=request.panes.len() as u64).collect::<Vec<_>>();
    anyhow::ensure!(
        layout_ids == expected,
        "headless layout must contain every pane exactly once"
    );
    if let Some(active_pane) = request.active_pane {
        match active_pane {
            crate::messages::SharedPaneRef::Draft { draft_id } => anyhow::ensure!(
                request.panes.iter().any(|pane| pane.draft_id == draft_id),
                "active pane references unknown draft {draft_id}"
            ),
            crate::messages::SharedPaneRef::Existing { .. } => {
                anyhow::bail!("a headless session cannot reference an existing pane")
            }
        }
    }
    let config_path = crate::paths::platform_config_dir().join("config.json");
    for pane in &request.panes {
        if let Some(command) = &pane.command {
            anyhow::ensure!(
                command
                    .program
                    .as_deref()
                    .is_some_and(|program| !program.trim().is_empty()),
                "headless command program must not be empty"
            );
        } else if !pane.profile.is_empty() {
            anyhow::ensure!(
                zetta_profiles::resolve(&config_path, &pane.profile).is_some(),
                "profile {:?} is not available on the host running zmux",
                pane.profile
            );
        }
        anyhow::ensure!(
            pane.size.columns > 0 && pane.size.lines > 0,
            "headless pane size must have positive columns and lines"
        );
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub(super) struct SharedBatchCommitContext<'a> {
    request: &'a crate::messages::SharedSpawnBatchRequest,
    mappings: &'a [crate::messages::SharedDraftMapping],
    mapping_map: &'a HashMap<u64, u64>,
    replacement: &'a BackgroundPaneLayout,
    peer_process_id: Option<u32>,
    proof: &'a SecretProof,
}

pub(super) fn commit_shared_batch(
    daemon: &Arc<Daemon>,
    provisional: &mut Vec<ProvisionalSharedPane>,
    context: SharedBatchCommitContext<'_>,
) -> Result<SharedBatchCommit> {
    use crate::messages::{SharedOperationReceipt, SharedPaneRef};
    let request = context.request;
    let mut sessions = daemon.sessions.lock().recover_poison();
    let session = sessions
        .iter_mut()
        .find(|session| session.id == request.session_id)
        .with_context(|| format!("session {} does not exist", request.session_id))?;
    anyhow::ensure!(
        session_control_authorized(session, context.peer_process_id, context.proof),
        "shared session {} is not authorized for this client",
        request.session_id
    );
    let mut state = normalize_shared_state(daemon, session)?;
    if let Some(receipt) = state.receipt(&request.operation_id) {
        return Ok(SharedBatchCommit::Retry {
            state: state.clone(),
            mappings: receipt.draft_mappings.clone(),
        });
    }
    if request.target_pane_id.is_none() && state.revision != request.base_revision {
        return Ok(SharedBatchCommit::Conflict(state));
    }

    let old_layout = state.presentation.layout.clone();
    let mut next_layout = old_layout.clone();
    if let Some(target) = request.target_pane_id {
        if !replace_layout_target(&mut next_layout, target, context.replacement.clone()) {
            return Ok(SharedBatchCommit::Conflict(state));
        }
    } else {
        next_layout = context.replacement.clone();
    }
    let mut next_ids = Vec::new();
    layout_pane_ids(&next_layout, &mut next_ids);
    let mut unique = next_ids.clone();
    unique.sort_unstable();
    unique.dedup();
    anyhow::ensure!(
        unique.len() == next_ids.len(),
        "shared layout contains a pane more than once"
    );
    anyhow::ensure!(
        context
            .mappings
            .iter()
            .all(|mapping| next_ids.contains(&mapping.pane_id)),
        "every spawned draft must appear exactly once in the replacement layout"
    );
    anyhow::ensure!(
        next_ids.iter().all(|pane_id| {
            session.panes.iter().any(|pane| pane.id == *pane_id)
                || context
                    .mappings
                    .iter()
                    .any(|mapping| mapping.pane_id == *pane_id)
        }),
        "shared layout references a pane the daemon does not hold"
    );

    let mut old_ids = Vec::new();
    layout_pane_ids(&old_layout, &mut old_ids);
    let removed = old_ids
        .into_iter()
        .filter(|pane_id| !next_ids.contains(pane_id))
        .collect::<Vec<_>>();
    let next_active_pane = match request.active_pane {
        Some(SharedPaneRef::Existing { pane_id }) => pane_id,
        Some(SharedPaneRef::Draft { draft_id }) => *context
            .mapping_map
            .get(&draft_id)
            .with_context(|| format!("active pane references unknown draft {draft_id}"))?,
        None => state.presentation.active_pane,
    };
    anyhow::ensure!(
        next_ids.contains(&next_active_pane),
        "active shared pane is not in the layout"
    );
    let retention = *daemon.retention.lock().recover_poison();
    for (draft, pane) in request.panes.iter().zip(provisional.drain(..)) {
        let mut metadata = draft.metadata.clone();
        metadata.id = pane.pane_id;
        metadata.state = crate::protocol::BackgroundPaneState::Running;
        metadata.exit = None;
        state.summary.panes.push(metadata);
        session.panes.push(Pane {
            id: pane.pane_id,
            pty: pane.pty,
            #[cfg(windows)]
            console_id: pane.console_id,
            #[cfg(windows)]
            child_events: pane.child_events,
            attachment: Attachment::Shared(Vec::new()),
            attachment_client_id: None,
            size: pane.size,
            retained: retention.new_retained(pane.size.columns, pane.size.lines),
            handed_over: None,
            handover_waiters: 0,
            exited: false,
            exit_status: None,
            pending_input: Vec::new(),
            clipboard_scanner: zclip::protocol::Scanner::default(),
        });
    }
    state
        .summary
        .panes
        .retain(|pane| !removed.contains(&pane.id));
    session.panes.retain(|pane| !removed.contains(&pane.id));
    for pane_id in &removed {
        remove_pane_agent_links(*pane_id);
    }
    state.presentation.layout = next_layout;
    state.presentation.active_pane = next_active_pane;
    state
        .presentation
        .minimized_panes
        .retain(|pane_id| !removed.contains(pane_id));
    if state
        .presentation
        .maximized_pane
        .is_some_and(|pane_id| removed.contains(&pane_id))
    {
        state.presentation.maximized_pane = None;
    }
    state.summary.layout = state.presentation.layout.clone();
    state.summary.active_pane = state.presentation.active_pane;
    state.revision = state.revision.next();
    state.last_operation_id = Some(request.operation_id.clone());
    state.record_receipt(SharedOperationReceipt {
        operation_id: request.operation_id.clone(),
        draft_mappings: context.mappings.to_vec(),
    });
    clear_shared_size_reports(session);
    session.summary = state.summary.clone();
    session.state = state.state.clone();
    session.shared_state = Some(state.clone());
    Ok(SharedBatchCommit::Applied {
        state,
        removed,
        mappings: context.mappings.to_vec(),
    })
}

/// Starts a complete set of draft processes before committing any pane or
/// layout state. A target-based replacement is rebased against the latest
/// tree; a whole-layout replacement remains exact-revision by design.
pub(in crate::server) fn spawn_shared_batch(
    daemon: &Arc<Daemon>,
    request: crate::messages::SharedSpawnBatchRequest,
    caller: Caller<'_>,
    connection: &mut Connection,
) -> Result<()> {
    let Caller {
        peer_process_id,
        session_secret,
        ..
    } = caller;
    let client_id = caller.client_id.clone();
    use crate::messages::SharedDraftMapping;

    anyhow::ensure!(
        !request.panes.is_empty(),
        "a shared spawn batch must contain a pane"
    );
    let mut draft_ids = request
        .panes
        .iter()
        .map(|pane| pane.draft_id)
        .collect::<Vec<_>>();
    draft_ids.sort_unstable();
    anyhow::ensure!(
        draft_ids.windows(2).all(|pair| pair[0] != pair[1]),
        "shared draft pane IDs must be unique"
    );

    // Proved once, before the lock, and honoured by both checks below: the one
    // here and the one at commit, which runs after the panes have spawned.
    let proof = prove_session_secret(daemon, peer_process_id, session_secret, |session| {
        session.id == request.session_id
    });
    {
        let mut sessions = daemon.sessions.lock().recover_poison();
        let session = sessions
            .iter_mut()
            .find(|session| session.id == request.session_id)
            .with_context(|| format!("session {} does not exist", request.session_id))?;
        anyhow::ensure!(
            session.offered,
            "session {} is not offered for shared collaboration",
            request.session_id
        );
        anyhow::ensure!(
            session_control_authorized(session, peer_process_id, &proof),
            "shared session {} is not authorized for this client",
            request.session_id
        );
        let state = normalize_shared_state(daemon, session)?;
        if let Some(receipt) = state.receipt(&request.operation_id) {
            return connection.send(&Response::SharedBatchSpawned {
                mappings: receipt.draft_mappings.clone(),
                state,
            });
        }
        if request.target_pane_id.is_none() && state.revision != request.base_revision {
            return connection.send(&Response::SharedConflict { state });
        }
    }

    let mut provisional = Vec::with_capacity(request.panes.len());
    for draft in &request.panes {
        match start_shared_draft(daemon, draft) {
            Ok(pane) => provisional.push(pane),
            Err(error) => {
                close_provisional_shared_panes(daemon, provisional);
                return Err(error);
            }
        }
    }
    let mappings = request
        .panes
        .iter()
        .zip(&provisional)
        .map(|(draft, pane)| SharedDraftMapping {
            draft_id: draft.draft_id,
            pane_id: pane.pane_id,
        })
        .collect::<Vec<_>>();
    let mapping_map = mappings
        .iter()
        .map(|mapping| (mapping.draft_id, mapping.pane_id))
        .collect::<HashMap<_, _>>();
    let replacement = match resolve_draft_layout(&request.replacement, &mapping_map) {
        Ok(layout) => layout,
        Err(error) => {
            close_provisional_shared_panes(daemon, provisional);
            return Err(error);
        }
    };

    let commit = commit_shared_batch(
        daemon,
        &mut provisional,
        SharedBatchCommitContext {
            request: &request,
            mappings: &mappings,
            mapping_map: &mapping_map,
            replacement: &replacement,
            peer_process_id,
            proof: &proof,
        },
    );

    let commit = match commit {
        Ok(committed) => committed,
        Err(error) => {
            close_provisional_shared_panes(daemon, provisional);
            return Err(error);
        }
    };
    let (state, removed, response_mappings) = match commit {
        SharedBatchCommit::Applied {
            state,
            removed,
            mappings,
        } => (state, removed, mappings),
        SharedBatchCommit::Retry { state, mappings } => {
            close_provisional_shared_panes(daemon, provisional);
            return connection.send(&Response::SharedBatchSpawned { mappings, state });
        }
        SharedBatchCommit::Conflict(state) => {
            close_provisional_shared_panes(daemon, provisional);
            return connection.send(&Response::SharedConflict { state });
        }
    };
    let added = response_mappings
        .iter()
        .map(|mapping| mapping.pane_id)
        .collect();
    broadcast_except(
        daemon,
        &Event::SharedPanesChanged {
            session_id: request.session_id,
            added,
            removed,
            state: state.clone(),
        },
        &client_id,
    );
    publish(daemon);
    wake_drain(daemon);
    connection.send(&Response::SharedBatchSpawned {
        mappings: response_mappings,
        state,
    })
}
