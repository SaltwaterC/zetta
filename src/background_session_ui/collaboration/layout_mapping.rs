//! Translating a shared session's layout and pane ids into this window's
//! namespace and back, and deriving the operation a local layout change
//! amounts to.

use super::*;

/// Puts back a pane the session's layout does not place.
///
/// A split puts its pane in the tab before the session has accepted it, so any
/// snapshot arriving in between describes a tab without that pane. Installing
/// one verbatim drops it out of the layout, and the split — whose whole
/// proposal is "put this pane next to that one" — then has nothing to describe.
///
/// It goes back beside whatever it was split from, read out of the layout being
/// replaced, which is where the session is about to put it too. Reports whether
/// there was somewhere to put it; there is not if the pane it was split from
/// has itself gone.
pub(super) fn reinsert_unplaced_pane(
    previous: &PaneLayout,
    layout: &mut PaneLayout,
    pane_id: u64,
) -> bool {
    let Some((axis, first_ratio, draft_is_first, sibling)) = pane_split_context(previous, pane_id)
    else {
        return false;
    };
    let anchor = sibling.first_pane();
    if !layout.contains_pane(anchor) {
        return false;
    }
    let pane = Box::new(PaneLayout::Pane(pane_id));
    let anchor_layout = Box::new(PaneLayout::Pane(anchor));
    let (first, second) = if draft_is_first {
        (pane, anchor_layout)
    } else {
        (anchor_layout, pane)
    };
    layout.replace(
        anchor,
        PaneLayout::Split {
            axis,
            first_ratio,
            first,
            second,
        },
    )
}

/// The split that holds `pane_id` directly: its axis and ratio, whether the
/// pane is the first child, and the subtree on the other side.
pub(super) fn pane_split_context(
    layout: &PaneLayout,
    pane_id: u64,
) -> Option<(SplitAxis, u16, bool, &PaneLayout)> {
    let PaneLayout::Split {
        axis,
        first_ratio,
        first,
        second,
    } = layout
    else {
        return None;
    };
    if matches!(first.as_ref(), PaneLayout::Pane(id) if *id == pane_id) {
        return Some((*axis, *first_ratio, true, second));
    }
    if matches!(second.as_ref(), PaneLayout::Pane(id) if *id == pane_id) {
        return Some((*axis, *first_ratio, false, first));
    }
    pane_split_context(first, pane_id).or_else(|| pane_split_context(second, pane_id))
}

/// The panes in this tab whose session pane is gone.
///
/// A pane with no translation at all is *not* one of them: it is a draft the
/// session has not been told about yet, which is exactly the state a tab is in
/// while a split is being proposed. Treating "no translation" as "lost" would
/// delete the pane the proposal is about.
pub(super) fn panes_the_session_lost(
    tab: &Tab,
    local_to_mux: &HashMap<u64, u64>,
    state: &SharedSessionState,
) -> Vec<u64> {
    tab.panes
        .iter()
        .filter(|pane| {
            local_to_mux
                .get(&pane.id)
                .is_some_and(|mux_pane_id| !state.contains_pane(*mux_pane_id))
        })
        .map(|pane| pane.id)
        .collect()
}

/// Takes a pane out of a tab and out of its layout.
///
/// `Tab::remove_pane` deliberately leaves the layout alone, because most callers
/// reshape it themselves. The shared-session removers do not have a reshape of
/// their own — they expect the canonical snapshot to supply one — so they have
/// to collapse the split here. A snapshot that turns out to be unapplicable
/// would otherwise leave the pane's region reserved forever.
pub(super) fn detach_pane_from_tab(tab: &mut Tab, pane_id: u64) {
    tab.remove_pane(pane_id);
    if let Some(layout) = tab.layout.clone().without(pane_id) {
        tab.layout = layout;
    }
    tab.restore_focus_after_close(pane_id, tab.layout.first_pane());
}

/// The one canonical geometry change a publication owes, or `None` when the
/// daemon's geometry already matches this window's.
///
/// Geometry converges a dimension at a time, deliberately: each operation names
/// what changed rather than replacing the tree, so two viewers moving different
/// dividers do not undo one another. The caller sends the durable tab state
/// once this returns `None`, which is what keeps an icon or a rename from being
/// dropped because the layout happened to differ in the same publication.
pub(super) fn shared_geometry_operation(
    canonical: &zmux::messages::SharedPresentationState,
    summary: &BackgroundSessionSummary,
    maximized_pane: Option<u64>,
    minimized_panes: &[u64],
    explicit_focus_pending: bool,
) -> Option<zmux::messages::SharedSessionOperation> {
    if summary.layout != canonical.layout {
        return Some(
            shared_layout_operation(&canonical.layout, &summary.layout, summary.active_pane)
                .unwrap_or(zmux::messages::SharedSessionOperation::SetLayout {
                    layout: summary.layout.clone(),
                }),
        );
    }
    if !explicit_focus_pending && summary.active_pane != canonical.active_pane {
        return Some(zmux::messages::SharedSessionOperation::SetFocus {
            pane_id: summary.active_pane,
        });
    }
    if maximized_pane != canonical.maximized_pane {
        return Some(zmux::messages::SharedSessionOperation::SetMaximized {
            pane_id: maximized_pane,
        });
    }
    if minimized_panes == canonical.minimized_panes {
        return None;
    }
    let changed = minimized_panes
        .iter()
        .chain(&canonical.minimized_panes)
        .copied()
        .find(|pane_id| {
            minimized_panes.contains(pane_id) != canonical.minimized_panes.contains(pane_id)
        })?;
    Some(zmux::messages::SharedSessionOperation::SetMinimized {
        pane_id: changed,
        minimized: minimized_panes.contains(&changed),
    })
}

/// How the tab state the daemon stores without reading it — icons, titles, pane
/// names, themes and overlays — is carried to the other viewers.
///
/// `ReplaceTab` when the summary can be described, because that also refreshes
/// what the catalog and the pickers show; `SetTabState` otherwise, so a tab
/// holding a pane the daemon has not committed yet still publishes what the user
/// chose. The daemon discards a `ReplaceTab`'s layout and focus in favour of its
/// own presentation, so neither form can move geometry — which is why the two
/// halves of a publication are independent.
pub(super) fn durable_state_operation(
    state: serde_json::Value,
    summary: Option<BackgroundSessionSummary>,
) -> zmux::messages::SharedSessionOperation {
    match summary {
        Some(summary) => zmux::messages::SharedSessionOperation::ReplaceTab { summary, state },
        None => zmux::messages::SharedSessionOperation::SetTabState { state },
    }
}

/// The publication to queue when this one has not converged yet, or `None` once
/// it has asked as many times as it may.
///
/// Geometry the daemon will not accept would otherwise re-queue for ever: each
/// answer installs the canonical state, the tab still disagrees, and the next
/// publication proposes the same change again.
pub(super) fn publication_retry(attempts: u32) -> Option<SharedOperation> {
    (attempts + 1 < SHARED_PUBLISH_ATTEMPTS).then_some(SharedOperation::PublishState {
        attempts: attempts + 1,
    })
}

pub(super) fn focus_retry(
    mux_pane_id: u64,
    generation: u64,
    attempts: u32,
) -> Option<SharedOperation> {
    (attempts + 1 < SHARED_FOCUS_ATTEMPTS).then_some(SharedOperation::FocusPane {
        mux_pane_id,
        generation,
        attempts: attempts + 1,
    })
}

pub(super) fn shared_layout_operation(
    current: &BackgroundPaneLayout,
    desired: &BackgroundPaneLayout,
    active_pane: u64,
) -> Option<zmux::messages::SharedSessionOperation> {
    let mut change = None;
    if find_single_ratio_change(current, desired, &mut change) {
        return change.map(|(first_pane_id, second_pane_id, first_ratio)| {
            zmux::messages::SharedSessionOperation::SetSplitRatio {
                first_pane_id,
                second_pane_id,
                first_ratio,
            }
        });
    }
    let current_ids = layout_ids(current);
    let desired_ids = layout_ids(desired);
    if let Some(operation) = find_directional_move(current, desired, active_pane) {
        return Some(operation);
    }
    let different = current_ids
        .iter()
        .zip(&desired_ids)
        .filter_map(|(current, desired)| (current != desired).then_some((*current, *desired)))
        .collect::<Vec<_>>();
    if same_layout_shape(current, desired)
        && different.len() == 2
        && different[0] == (different[1].1, different[1].0)
    {
        return Some(zmux::messages::SharedSessionOperation::SwapPanes {
            first_pane_id: different[0].0,
            second_pane_id: different[0].1,
        });
    }
    find_rotation(current, desired, active_pane)
}

pub(super) fn find_directional_move(
    current: &BackgroundPaneLayout,
    desired: &BackgroundPaneLayout,
    pane_id: u64,
) -> Option<zmux::messages::SharedSessionOperation> {
    use zmux::messages::SharedPaneDirection;
    for direction in [
        SharedPaneDirection::Left,
        SharedPaneDirection::Right,
        SharedPaneDirection::Up,
        SharedPaneDirection::Down,
    ] {
        let mut candidate = current.clone();
        if zmux::messages::move_layout_pane(&mut candidate, pane_id, direction)
            && candidate == *desired
        {
            return Some(zmux::messages::SharedSessionOperation::MovePane { pane_id, direction });
        }
    }
    None
}

pub(super) fn find_rotation(
    current: &BackgroundPaneLayout,
    desired: &BackgroundPaneLayout,
    pane_id: u64,
) -> Option<zmux::messages::SharedSessionOperation> {
    use zmux::messages::SharedRotationDirection;
    for direction in [
        SharedRotationDirection::Clockwise,
        SharedRotationDirection::CounterClockwise,
    ] {
        let mut candidate = current.clone();
        if zmux::messages::rotate_layout_pane(&mut candidate, pane_id, direction)
            && candidate == *desired
        {
            return Some(zmux::messages::SharedSessionOperation::RotateSplit {
                pane_id,
                direction,
            });
        }
    }
    None
}

pub(super) fn find_single_ratio_change(
    current: &BackgroundPaneLayout,
    desired: &BackgroundPaneLayout,
    change: &mut Option<(u64, u64, u16)>,
) -> bool {
    match (current, desired) {
        (
            BackgroundPaneLayout::Pane { pane_id: current },
            BackgroundPaneLayout::Pane { pane_id: desired },
        ) => current == desired,
        (
            BackgroundPaneLayout::Split {
                axis: current_axis,
                first_ratio: current_ratio,
                first: current_first,
                second: current_second,
            },
            BackgroundPaneLayout::Split {
                axis: desired_axis,
                first_ratio: desired_ratio,
                first: desired_first,
                second: desired_second,
            },
        ) => {
            if current_axis != desired_axis {
                return false;
            }
            if current_ratio != desired_ratio {
                if change.is_some() {
                    return false;
                }
                *change = Some((
                    first_layout_pane(current_first),
                    first_layout_pane(current_second),
                    *desired_ratio,
                ));
            }
            find_single_ratio_change(current_first, desired_first, change)
                && find_single_ratio_change(current_second, desired_second, change)
        }
        _ => false,
    }
}

pub(super) fn first_layout_pane(layout: &BackgroundPaneLayout) -> u64 {
    match layout {
        BackgroundPaneLayout::Pane { pane_id } => *pane_id,
        BackgroundPaneLayout::Split { first, .. } => first_layout_pane(first),
    }
}

pub(super) fn layout_ids(layout: &BackgroundPaneLayout) -> Vec<u64> {
    match layout {
        BackgroundPaneLayout::Pane { pane_id } => vec![*pane_id],
        BackgroundPaneLayout::Split { first, second, .. } => {
            let mut ids = layout_ids(first);
            ids.extend(layout_ids(second));
            ids
        }
    }
}

pub(super) fn same_layout_shape(
    current: &BackgroundPaneLayout,
    desired: &BackgroundPaneLayout,
) -> bool {
    match (current, desired) {
        (BackgroundPaneLayout::Pane { .. }, BackgroundPaneLayout::Pane { .. }) => true,
        (
            BackgroundPaneLayout::Split {
                axis: current_axis,
                first_ratio: current_ratio,
                first: current_first,
                second: current_second,
            },
            BackgroundPaneLayout::Split {
                axis: desired_axis,
                first_ratio: desired_ratio,
                first: desired_first,
                second: desired_second,
            },
        ) => {
            current_axis == desired_axis
                && current_ratio == desired_ratio
                && same_layout_shape(current_first, desired_first)
                && same_layout_shape(current_second, desired_second)
        }
        _ => false,
    }
}

pub(super) fn remap_tab_state(
    tab_state: &mut TabState,
    presentation: &zmux::messages::SharedPresentationState,
    mux_to_local: &HashMap<u64, u64>,
) -> Result<()> {
    let canonical_to_mux = canonical_pane_mappings(tab_state);
    tab_state.panes.retain_mut(|pane| {
        let Ok(id) = remap_canonical_id(pane.id, pane.mux_pane_id, &canonical_to_mux, mux_to_local)
        else {
            return false;
        };
        pane.id = id;
        pane.stack.retain_mut(|entry| {
            let Ok(id) =
                remap_canonical_id(entry.id, entry.mux_pane_id, &canonical_to_mux, mux_to_local)
            else {
                return false;
            };
            entry.id = id;
            true
        });
        true
    });
    anyhow::ensure!(
        !tab_state.panes.is_empty(),
        "shared tab state has no panes attached in this window"
    );
    // A pane lifecycle event can legitimately carry the previous opaque tab
    // blob. The summary is canonical for topology, so use its mapped layout
    // and focus before applying the durable per-tab fields from the blob.
    tab_state.layout = map_background_layout_state(&presentation.layout, mux_to_local)?;
    tab_state.active_pane = mux_to_local
        .get(&presentation.active_pane)
        .copied()
        .with_context(|| {
            format!(
                "active shared pane {} is not attached",
                presentation.active_pane
            )
        })?;
    tab_state.focus_history = tab_state
        .focus_history
        .iter()
        .filter_map(|id| remap_id(*id, &canonical_to_mux, mux_to_local).ok())
        .collect();
    tab_state.maximized_pane = tab_state
        .maximized_pane
        .and_then(|id| remap_id(id, &canonical_to_mux, mux_to_local).ok());
    tab_state.minimized_panes = tab_state
        .minimized_panes
        .iter()
        .filter_map(|id| remap_id(*id, &canonical_to_mux, mux_to_local).ok())
        .collect();
    tab_state.selected_minimized_pane = tab_state
        .selected_minimized_pane
        .and_then(|id| remap_id(id, &canonical_to_mux, mux_to_local).ok());
    Ok(())
}

pub(super) fn map_background_layout_state(
    layout: &BackgroundPaneLayout,
    mux_to_local: &HashMap<u64, u64>,
) -> Result<LayoutState> {
    Ok(match layout {
        BackgroundPaneLayout::Pane { pane_id } => LayoutState::Pane {
            pane_id: mux_to_local
                .get(pane_id)
                .copied()
                .with_context(|| format!("shared pane {pane_id} is not attached"))?,
        },
        BackgroundPaneLayout::Split {
            axis,
            first_ratio,
            first,
            second,
        } => LayoutState::Split {
            axis: if axis.eq_ignore_ascii_case("vertical") {
                AxisState::Vertical
            } else {
                AxisState::Horizontal
            },
            first_ratio: *first_ratio,
            first: Box::new(map_background_layout_state(first, mux_to_local)?),
            second: Box::new(map_background_layout_state(second, mux_to_local)?),
        },
    })
}

pub(super) fn canonical_pane_mappings(tab_state: &TabState) -> HashMap<u64, u64> {
    tab_state
        .panes
        .iter()
        .flat_map(|pane| {
            std::iter::once((pane.id, pane.mux_pane_id))
                .chain(pane.stack.iter().map(|entry| (entry.id, entry.mux_pane_id)))
        })
        .filter_map(|(local_id, mux_id)| mux_id.map(|mux_id| (local_id, mux_id)))
        .collect()
}

pub(super) fn remap_canonical_id(
    canonical_id: u64,
    mux_id: Option<u64>,
    canonical_to_mux: &HashMap<u64, u64>,
    mux_to_local: &HashMap<u64, u64>,
) -> Result<u64> {
    let mux_id = mux_id
        .or_else(|| canonical_to_mux.get(&canonical_id).copied())
        .with_context(|| format!("shared pane {canonical_id} has no stable multiplexer id"))?;
    remap_id(canonical_id, canonical_to_mux, mux_to_local).or_else(|_| {
        mux_to_local
            .get(&mux_id)
            .copied()
            .with_context(|| format!("shared pane {mux_id} is not attached in this window"))
    })
}

pub(super) fn remap_id(
    canonical_id: u64,
    canonical_to_mux: &HashMap<u64, u64>,
    mux_to_local: &HashMap<u64, u64>,
) -> Result<u64> {
    let mux_id = canonical_to_mux
        .get(&canonical_id)
        .with_context(|| format!("shared pane {canonical_id} has no stable mapping"))?;
    mux_to_local
        .get(mux_id)
        .copied()
        .with_context(|| format!("shared pane {mux_id} is not attached in this window"))
}

/// Refuses a layout that names a pane the tab does not hold.
///
/// `render_pane_leaf` draws an empty region for a layout id with no
/// `TerminalPane`, and the split node holding it goes on reserving the space,
/// so the survivors never grow back. The reverse direction — a pane the layout
/// does not place — is reported but not refused, because a pane whose creation
/// the daemon has not committed yet is legitimately in neither.
pub(super) fn ensure_layout_covers_tab(tab: &Tab, layout: &PaneLayout) -> Result<()> {
    let placed = layout.pane_ids();
    if let Some(missing) = placed.iter().find(|pane_id| tab.pane(**pane_id).is_none()) {
        anyhow::bail!(
            "shared layout places pane {missing}, which tab {} does not hold",
            tab.id
        );
    }
    let unplaced = tab
        .panes
        .iter()
        .filter(|pane| !placed.contains(&pane.id))
        .map(|pane| pane.id)
        .collect::<Vec<_>>();
    if !unplaced.is_empty() {
        log::warn!(
            "shared layout for tab {} does not place pane(s) {unplaced:?}; they are awaiting a commit",
            tab.id
        );
    }
    Ok(())
}

pub(super) fn apply_canonical_presentation(
    tab: &mut Tab,
    state: &SharedSessionState,
    layout: PaneLayout,
    mux_to_local: &HashMap<u64, u64>,
    active_pane_override: Option<u64>,
) -> Result<()> {
    tab.layout = layout;
    tab.active_pane = active_pane_override.unwrap_or(
        mux_to_local
            .get(&state.presentation.active_pane)
            .copied()
            .with_context(|| {
                format!(
                    "active shared pane {} is not attached",
                    state.presentation.active_pane
                )
            })?,
    );
    tab.maximized_pane = state
        .presentation
        .maximized_pane
        .and_then(|pane_id| mux_to_local.get(&pane_id).copied());
    tab.minimized_panes = state
        .presentation
        .minimized_panes
        .iter()
        .filter_map(|pane_id| mux_to_local.get(pane_id).copied())
        .collect();
    tab.selected_minimized_pane = tab
        .selected_minimized_pane
        .filter(|pane_id| tab.minimized_panes.contains(pane_id));
    for pane in &state.summary.panes {
        if let Some(local_id) = mux_to_local.get(&pane.id).copied()
            && let Some(local) = tab.pane_mut(local_id)
        {
            apply_pane_summary(local, pane);
        }
    }
    Ok(())
}

pub(super) fn map_background_layout(
    layout: &BackgroundPaneLayout,
    mux_to_local: &HashMap<u64, u64>,
) -> Result<PaneLayout> {
    Ok(match layout {
        BackgroundPaneLayout::Pane { pane_id } => PaneLayout::Pane(
            mux_to_local
                .get(pane_id)
                .copied()
                .with_context(|| format!("shared pane {pane_id} is not attached"))?,
        ),
        BackgroundPaneLayout::Split {
            axis,
            first_ratio,
            first,
            second,
        } => PaneLayout::Split {
            axis: if axis.eq_ignore_ascii_case("vertical") {
                SplitAxis::Vertical
            } else {
                SplitAxis::Horizontal
            },
            first_ratio: *first_ratio,
            first: Box::new(map_background_layout(first, mux_to_local)?),
            second: Box::new(map_background_layout(second, mux_to_local)?),
        },
    })
}

pub(crate) fn remap_summary_to_mux(
    summary: &mut BackgroundSessionSummary,
    local_to_mux: &HashMap<u64, u64>,
) -> Result<()> {
    summary.active_pane = local_to_mux
        .get(&summary.active_pane)
        .copied()
        .context("the active pane has no multiplexer id")?;
    for pane in &mut summary.panes {
        pane.id = local_to_mux
            .get(&pane.id)
            .copied()
            .with_context(|| format!("pane {} has no multiplexer id", pane.id))?;
    }
    summary.layout = remap_summary_layout(&summary.layout, local_to_mux)?;
    Ok(())
}

pub(super) fn remap_summary_layout(
    layout: &BackgroundPaneLayout,
    local_to_mux: &HashMap<u64, u64>,
) -> Result<BackgroundPaneLayout> {
    Ok(match layout {
        BackgroundPaneLayout::Pane { pane_id } => BackgroundPaneLayout::Pane {
            pane_id: local_to_mux
                .get(pane_id)
                .copied()
                .with_context(|| format!("pane {pane_id} has no multiplexer id"))?,
        },
        BackgroundPaneLayout::Split {
            axis,
            first_ratio,
            first,
            second,
        } => BackgroundPaneLayout::Split {
            axis: axis.clone(),
            first_ratio: *first_ratio,
            first: Box::new(remap_summary_layout(first, local_to_mux)?),
            second: Box::new(remap_summary_layout(second, local_to_mux)?),
        },
    })
}

pub(super) fn apply_tab_state(tab: &mut Tab, state: TabState) {
    let icon = state
        .icon
        .as_deref()
        .and_then(crate::tab_icon_picker::parse_tab_icon_name);
    let icon_override = match state.icon_override {
        None => TabIconOverride::None,
        Some(None) => TabIconOverride::Hidden,
        Some(Some(name)) => crate::tab_icon_picker::parse_tab_icon_name(&name)
            .map(TabIconOverride::Icon)
            .unwrap_or_default(),
    };
    tab.icon = match icon_override {
        TabIconOverride::None => icon,
        TabIconOverride::Icon(icon) => Some(icon),
        TabIconOverride::Hidden => None,
    };
    tab.icon_override = icon_override;
    tab.next_pane_label = state.next_pane_label;
    tab.layout = state.layout.into_layout();
    tab.active_pane = state.active_pane;
    tab.focus_history = state.focus_history;
    tab.maximized_pane = state.maximized_pane;
    tab.minimized_panes = state.minimized_panes;
    tab.selected_minimized_pane = state.selected_minimized_pane;
    tab.broadcast_input = state.broadcast_input;
    tab.silent_mode = state.silent_mode;
    tab.shared = state.shared;
    tab.close_policy = if state.keep_running {
        TabClosePolicy::Background {
            authentication: None,
        }
    } else {
        TabClosePolicy::Close
    };
    tab.custom_title = state.custom_title;
    tab.worktree_seed_title = state.worktree_seed_title;
    tab.process_title = state.process_title;
    tab.pinned = state.pinned;
    tab.theme_override = state.theme_override;
    for pane_state in state.panes {
        let Some(pane) = tab.pane_mut(pane_state.id) else {
            continue;
        };
        apply_pane_state(pane, pane_state);
    }
}

pub(super) fn apply_pane_state(pane: &mut TerminalPane, state: PaneState) {
    pane.label_number = state.label_number;
    pane.generated_label = state.generated_label;
    pane.custom_label = state.custom_label;
    pane.theme_override = state.theme_override;
    pane.environment_overrides = state.environment_overrides;
    pane.exit = state.exit;
    pane.base_exited = state.base_exited;
    pane.pending_command = state.pending_command;
    pane.active_command = state.active_command;
    pane.detected_worktree_title = state.detected_worktree_title;
    apply_pane_overlay(pane, state.overlay.as_ref());
}

pub(super) fn apply_pane_overlay(
    pane: &mut TerminalPane,
    overlay: Option<&crate::session_state::OverlayState>,
) {
    let Some(overlay) = overlay else {
        pane.overlay_text = None;
        pane.overlay_font_size = None;
        pane.overlay_opacity = None;
        pane.overlay_color = None;
        return;
    };
    pane.overlay_text = Some(overlay.text.clone());
    pane.overlay_font_size = overlay
        .font_size
        .as_deref()
        .and_then(crate::OverlayFontSize::parse);
    pane.overlay_opacity = overlay.opacity;
    pane.overlay_color = overlay.color.map(|[h, s, l, a]| gpui::Hsla { h, s, l, a });
}

pub(super) fn apply_pane_summary(pane: &mut TerminalPane, summary: &BackgroundPaneSummary) {
    if pane.custom_label.is_none() && !summary.label.is_empty() {
        pane.generated_label = Some(summary.label.clone());
    }
    pane.base_exited = matches!(summary.state, BackgroundPaneState::Exited);
    pane.exit = summary.exit.clone();
}
