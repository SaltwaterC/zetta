//! Reconciling a shared session's published layout with the panes the daemon
//! actually holds: pruning panes that exited, and normalizing a summary so it
//! names exactly the live panes.

use super::*;

/// Removes panes whose process has ended and no client is reading, and ends
/// sessions that no longer hold anything to attach to.
///
/// A pane that exits while detached leaves nothing a reattach could show: the
/// process is gone, so handing its terminal to a new client produces an empty,
/// blocked tab. A session whose panes have all gone is dead and must not be
/// offered. A pane a client is still reading is kept until that client lets
/// go — the terminal it holds shows the exit — at which point the reclaim and
/// detach paths prune it here.
///
/// Returns whether anything was pruned, so the caller republishes only then.
pub(super) fn prune_exited_panes(daemon: &Arc<Daemon>) -> bool {
    let mut sessions = daemon.sessions.lock().recover_poison();
    let mut changed = false;
    let mut removed_session_ids = Vec::new();
    let mut shared_removals = Vec::new();
    #[cfg(windows)]
    let mut closed_consoles = Vec::new();
    for session in sessions.iter_mut() {
        let removed = session
            .panes
            .iter()
            .filter(|pane| exited_pane_is_unread(pane))
            .map(|pane| pane.id)
            .collect::<Vec<_>>();
        let before = session.panes.len();
        session.panes.retain(|pane| {
            let remove = exited_pane_is_unread(pane);
            #[cfg(windows)]
            if remove {
                closed_consoles.push(pane.console_id);
            }
            !remove
        });
        changed |= session.panes.len() != before;
        for pane_id in &removed {
            lifecycle::remove_pane_agent_links(*pane_id);
        }
        if !removed.is_empty()
            && !session.panes.is_empty()
            && let Some(state) = remove_pruned_shared_panes(daemon, session, &removed)
        {
            shared_removals.push((session.id, removed, state));
        }
        if session.panes.is_empty() {
            removed_session_ids.push(session.id);
        }
    }
    let before = sessions.len();
    sessions.retain(|session| !session.panes.is_empty());
    changed |= sessions.len() != before;
    drop(sessions);
    for session_id in &removed_session_ids {
        remove_image_session(daemon, *session_id);
    }
    #[cfg(feature = "session-persistence")]
    {
        let mut persistence = daemon.persistence.lock();
        if let Some(persistence) = persistence.as_mut() {
            for session_id in removed_session_ids {
                persistence.unseal(session_id);
                if let Err(error) = persistence.forget(session_id) {
                    log::warn!("could not remove persisted session {session_id}: {error:#}");
                }
            }
        }
    }
    #[cfg(windows)]
    for console_id in closed_consoles {
        close_host_console(daemon, console_id);
    }
    for (session_id, removed, state) in shared_removals {
        broadcast(
            daemon,
            &Event::SharedPanesChanged {
                session_id,
                added: Vec::new(),
                removed,
                state,
            },
        );
    }
    changed
}

/// A batch-created shared pane has no data-plane client until the requester
/// attaches one. If its program exits first, it is just as unobservable as an
/// unheld pane and must not keep a dead entry in the session forever.
pub(super) fn exited_pane_is_unread(pane: &Pane) -> bool {
    pane.exited
        && match &pane.attachment {
            Attachment::None => true,
            Attachment::Shared(clients) => clients.is_empty(),
            Attachment::Exclusive(_)
            | Attachment::Revoking { .. }
            | Attachment::Granting { .. } => false,
        }
}

/// Keeps an offered session's canonical layout in lockstep with a pane that
/// the reaper has removed.  A shared split replaces one node of the existing
/// canonical tree, so leaving an exited node in that tree makes every later
/// split name a pane the daemon no longer owns.
pub(super) fn remove_pruned_shared_panes(
    daemon: &Arc<Daemon>,
    session: &mut Session,
    removed: &[u64],
) -> Option<crate::messages::SharedSessionState> {
    if session.shared_state.is_some()
        && let Err(error) = normalize_shared_state(daemon, session)
    {
        log::warn!(
            "could not normalize shared session {} before pruning panes: {error:#}",
            session.id
        );
        return None;
    }
    let state = session.shared_state.as_mut()?;
    let mut changed = false;
    for pane_id in removed {
        if !state.contains_pane(*pane_id) {
            continue;
        }
        // The final pane takes the session with it, so there is no surviving
        // shared state or viewer to update.
        if state.summary.panes.len() == 1 {
            break;
        }
        let operation_id = SharedOperationId::new(
            ClientId::new("daemon-prune"),
            state.revision.0.saturating_add(1),
        );
        state
            .apply_operation(
                operation_id,
                &SharedSessionOperation::ClosePane { pane_id: *pane_id },
            )
            .expect("the daemon pane set and canonical shared state agree");
        changed = true;
    }
    if !changed {
        return None;
    }
    session.summary = state.summary.clone();
    session.summary.id = session.id;
    session.state = state.state.clone();
    Some(state.clone())
}

/// Ensures an offered session's collaboration snapshot describes the panes the
/// daemon can actually attach.
///
/// `Session::panes` owns the PTYs and is therefore the final authority: the
/// catalog summary is only a client-supplied description of them. Rebuilding
/// that description before normalizing prevents `list` from advertising a
/// pane that the attachment path cannot open. If an older handoff left those
/// two out of step, the repair gets a new revision, is persisted, and is sent
/// to subscribers before the caller continues with its request.
pub(super) fn normalize_shared_state(
    daemon: &Arc<Daemon>,
    session: &mut Session,
) -> Result<crate::messages::SharedSessionState> {
    let had_shared_state = session.shared_state.is_some();
    let mut state = session.shared_state.clone().unwrap_or_else(|| {
        crate::messages::SharedSessionState::new(
            session.id,
            session.summary.clone(),
            session.state.clone(),
        )
    });
    state = state.migrate()?;

    let live_summary = summary_from_daemon_panes(session, &state)?;
    let catalog_changed = session.summary != live_summary;
    session.summary = live_summary.clone();
    let summary_changed = state.summary != live_summary;
    state.summary = live_summary;
    let presentation_changed = state.normalize()?;
    let repaired = had_shared_state && (catalog_changed || summary_changed || presentation_changed);
    if repaired {
        state.revision = state.revision.next();
    }
    session.shared_state = Some(state.clone());

    if repaired {
        #[cfg(feature = "session-persistence")]
        persist_live_session(daemon, session)?;
        broadcast(
            daemon,
            &Event::SharedSessionUpdated {
                state: state.clone(),
            },
        );
    }

    Ok(state)
}

/// Reconciles catalog metadata and geometry with the daemon-owned live PTYs.
///
/// Metadata for a surviving pane remains intact. A pane which was never
/// published is given a deliberately minimal entry so that it remains visible
/// and attachable; the next ordinary publication will fill in its richer
/// client-owned metadata. An exited pane can remain daemon-owned while its last
/// viewer is still unwinding, but it is no longer attachable and must not be in
/// the summary a new viewer receives. Geometry preserves the layout that
/// accounts for the most live panes, collapses dead branches, and appends
/// unpublished panes.
pub(super) fn summary_from_daemon_panes(
    session: &Session,
    previous_state: &crate::messages::SharedSessionState,
) -> Result<BackgroundSessionSummary> {
    let live_pane_ids = session
        .panes
        .iter()
        .filter(|pane| !pane.exited)
        .map(|pane| pane.id)
        .collect::<HashSet<_>>();
    anyhow::ensure!(
        !live_pane_ids.is_empty(),
        "session {} has no live daemon-owned panes",
        session.id
    );

    let mut included = HashSet::new();
    let mut panes = Vec::new();
    for summary in [&session.summary, &previous_state.summary] {
        panes.extend(
            summary
                .panes
                .iter()
                .filter(|pane| live_pane_ids.contains(&pane.id) && included.insert(pane.id))
                .cloned(),
        );
    }
    for pane in session.panes.iter().filter(|pane| !pane.exited) {
        if included.insert(pane.id) {
            panes.push(unpublished_pane_summary(pane));
        }
    }

    let summary_coverage = live_layout_coverage(&session.summary.layout, &live_pane_ids);
    let presentation_coverage =
        live_layout_coverage(&previous_state.presentation.layout, &live_pane_ids);
    let source_layout = if presentation_coverage > summary_coverage {
        &previous_state.presentation.layout
    } else {
        &session.summary.layout
    };
    let mut laid_out = HashSet::new();
    let mut layout = retain_daemon_panes_in_layout(source_layout, &live_pane_ids, &mut laid_out);
    for pane in &panes {
        if laid_out.insert(pane.id) {
            layout = Some(match layout {
                Some(layout) => BackgroundPaneLayout::Split {
                    axis: "horizontal".to_owned(),
                    first_ratio: crate::protocol::DEFAULT_BACKGROUND_PANE_SPLIT_RATIO,
                    first: Box::new(layout),
                    second: Box::new(BackgroundPaneLayout::Pane { pane_id: pane.id }),
                },
                None => BackgroundPaneLayout::Pane { pane_id: pane.id },
            });
        }
    }
    let layout = layout.expect("a live daemon-owned pane is always placed in the layout");
    let active_pane = if live_pane_ids.contains(&session.summary.active_pane) {
        session.summary.active_pane
    } else if live_pane_ids.contains(&previous_state.presentation.active_pane) {
        previous_state.presentation.active_pane
    } else {
        first_layout_pane_id(&layout)
    };

    Ok(BackgroundSessionSummary {
        id: session.id,
        title: session.summary.title.clone(),
        authentication_required: session.summary.authentication_required,
        active_pane,
        layout,
        panes,
        held: session.summary.held,
        scoped_to: session.summary.scoped_to,
        key_envelope: session.summary.key_envelope.clone(),
    })
}

pub(super) fn unpublished_pane_summary(pane: &Pane) -> crate::protocol::BackgroundPaneSummary {
    crate::protocol::BackgroundPaneSummary {
        id: pane.id,
        label: format!("pane-{}", pane.id),
        profile: String::new(),
        configured_command: String::new(),
        application: String::new(),
        foreground_command: None,
        terminal_title: None,
        working_directory: None,
        state: if pane.exited {
            crate::protocol::BackgroundPaneState::Exited
        } else {
            crate::protocol::BackgroundPaneState::Running
        },
        exit: None,
    }
}

/// Retains each live leaf once, collapsing a split whose other child no
/// longer exists. `seen` also repairs a malformed layout that repeated a pane.
pub(super) fn retain_daemon_panes_in_layout(
    layout: &BackgroundPaneLayout,
    live_pane_ids: &HashSet<u64>,
    seen: &mut HashSet<u64>,
) -> Option<BackgroundPaneLayout> {
    match layout {
        BackgroundPaneLayout::Pane { pane_id }
            if live_pane_ids.contains(pane_id) && seen.insert(*pane_id) =>
        {
            Some(BackgroundPaneLayout::Pane { pane_id: *pane_id })
        }
        BackgroundPaneLayout::Pane { .. } => None,
        BackgroundPaneLayout::Split {
            axis,
            first_ratio,
            first,
            second,
        } => match (
            retain_daemon_panes_in_layout(first, live_pane_ids, seen),
            retain_daemon_panes_in_layout(second, live_pane_ids, seen),
        ) {
            (Some(first), Some(second)) => Some(BackgroundPaneLayout::Split {
                axis: axis.clone(),
                first_ratio: *first_ratio,
                first: Box::new(first),
                second: Box::new(second),
            }),
            (Some(layout), None) | (None, Some(layout)) => Some(layout),
            (None, None) => None,
        },
    }
}

/// Counts the distinct daemon-owned leaves a layout can preserve.
pub(super) fn live_layout_coverage(
    layout: &BackgroundPaneLayout,
    live_pane_ids: &HashSet<u64>,
) -> usize {
    let mut seen = HashSet::new();
    count_live_layout_panes(layout, live_pane_ids, &mut seen);
    seen.len()
}

pub(super) fn count_live_layout_panes(
    layout: &BackgroundPaneLayout,
    live_pane_ids: &HashSet<u64>,
    seen: &mut HashSet<u64>,
) {
    match layout {
        BackgroundPaneLayout::Pane { pane_id } => {
            if live_pane_ids.contains(pane_id) {
                seen.insert(*pane_id);
            }
        }
        BackgroundPaneLayout::Split { first, second, .. } => {
            count_live_layout_panes(first, live_pane_ids, seen);
            count_live_layout_panes(second, live_pane_ids, seen);
        }
    }
}

pub(super) fn first_layout_pane_id(layout: &BackgroundPaneLayout) -> u64 {
    match layout {
        BackgroundPaneLayout::Pane { pane_id } => *pane_id,
        BackgroundPaneLayout::Split { first, .. } => first_layout_pane_id(first),
    }
}

pub(super) fn summary_matches_daemon_panes(
    summary: &BackgroundSessionSummary,
    session: &Session,
) -> bool {
    let mut summary_pane_ids = summary.panes.iter().map(|pane| pane.id).collect::<Vec<_>>();
    summary_pane_ids.sort_unstable();
    let mut daemon_pane_ids = session
        .panes
        .iter()
        .filter(|pane| !pane.exited)
        .map(|pane| pane.id)
        .collect::<Vec<_>>();
    daemon_pane_ids.sort_unstable();
    summary_pane_ids == daemon_pane_ids
}
