//! Validating and rearranging a shared session's pane layout: the checks a
//! summary has to pass, and the move, swap and rotate operations the daemon
//! applies to the tree. The tree algebra itself is `zetta_layout`'s, shared
//! with the application's own pane layout.

use super::*;

pub(super) fn validate_summary_panes(summary: &BackgroundSessionSummary) -> anyhow::Result<()> {
    anyhow::ensure!(
        summary.panes.iter().all(|pane| pane.id != 0),
        "shared pane IDs must be positive multiplexer IDs"
    );
    let mut pane_ids = summary.panes.iter().map(|pane| pane.id).collect::<Vec<_>>();
    pane_ids.sort_unstable();
    anyhow::ensure!(
        pane_ids.windows(2).all(|pair| pair[0] != pair[1]),
        "shared pane IDs must be unique"
    );
    validate_layout_panes(&summary.layout, |pane_id| {
        summary.panes.iter().any(|pane| pane.id == pane_id)
    })?;
    let mut layout_ids = Vec::new();
    zetta_layout::collect_pane_ids(&summary.layout, &mut layout_ids);
    layout_ids.sort_unstable();
    anyhow::ensure!(
        pane_ids == layout_ids,
        "shared layout must contain every pane exactly once"
    );
    Ok(())
}

pub(super) fn validate_layout_panes(
    layout: &BackgroundPaneLayout,
    contains: impl Fn(u64) -> bool + Copy,
) -> anyhow::Result<()> {
    match layout {
        BackgroundPaneLayout::Pane { pane_id } => {
            anyhow::ensure!(
                contains(*pane_id),
                "shared layout references missing pane {pane_id}"
            );
            Ok(())
        }
        BackgroundPaneLayout::Split {
            first_ratio,
            first,
            second,
            ..
        } => {
            anyhow::ensure!(
                *first_ratio > 0
                    && *first_ratio < crate::protocol::BACKGROUND_PANE_SPLIT_RATIO_SCALE,
                "invalid shared split ratio"
            );
            validate_layout_panes(first, contains)?;
            validate_layout_panes(second, contains)
        }
    }
}

pub(super) fn remove_pane_from_layout(
    layout: &BackgroundPaneLayout,
    pane_id: u64,
) -> Option<BackgroundPaneLayout> {
    match layout {
        BackgroundPaneLayout::Pane { pane_id: id } if *id == pane_id => None,
        BackgroundPaneLayout::Pane { .. } => Some(layout.clone()),
        BackgroundPaneLayout::Split {
            first,
            second,
            axis,
            first_ratio,
        } => {
            let first = remove_pane_from_layout(first, pane_id);
            let second = remove_pane_from_layout(second, pane_id);
            match (first, second) {
                (Some(first), Some(second)) => Some(BackgroundPaneLayout::Split {
                    axis: *axis,
                    first_ratio: *first_ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(remaining), None) | (None, Some(remaining)) => Some(remaining),
                (None, None) => None,
            }
        }
    }
}

pub(super) fn find_divider(
    layout: &BackgroundPaneLayout,
    first_pane_id: u64,
    second_pane_id: u64,
) -> Option<u16> {
    match layout {
        BackgroundPaneLayout::Pane { .. } => None,
        BackgroundPaneLayout::Split {
            first_ratio,
            first,
            second,
            ..
        } => {
            if zetta_layout::contains_pane(&**first, first_pane_id)
                && zetta_layout::contains_pane(&**second, second_pane_id)
            {
                Some(*first_ratio)
            } else {
                find_divider(first, first_pane_id, second_pane_id)
                    .or_else(|| find_divider(second, first_pane_id, second_pane_id))
            }
        }
    }
}

pub(super) fn find_divider_mut(
    layout: &mut BackgroundPaneLayout,
    first_pane_id: u64,
    second_pane_id: u64,
) -> Option<&mut u16> {
    match layout {
        BackgroundPaneLayout::Pane { .. } => None,
        BackgroundPaneLayout::Split {
            first_ratio,
            first,
            second,
            ..
        } => {
            if zetta_layout::contains_pane(&**first, first_pane_id)
                && zetta_layout::contains_pane(&**second, second_pane_id)
            {
                Some(first_ratio)
            } else if zetta_layout::contains_pane(&**first, first_pane_id)
                || zetta_layout::contains_pane(&**first, second_pane_id)
            {
                find_divider_mut(first, first_pane_id, second_pane_id)
            } else {
                find_divider_mut(second, first_pane_id, second_pane_id)
            }
        }
    }
}

pub fn move_layout_pane(
    layout: &mut BackgroundPaneLayout,
    pane_id: u64,
    direction: SharedPaneDirection,
) -> bool {
    zetta_layout::move_pane(layout, pane_id, direction.into())
}

pub(super) fn can_move_layout_pane(
    layout: &BackgroundPaneLayout,
    pane_id: u64,
    direction: SharedPaneDirection,
) -> bool {
    move_layout_pane(&mut layout.clone(), pane_id, direction)
}

pub fn rotate_layout_pane(
    layout: &mut BackgroundPaneLayout,
    pane_id: u64,
    direction: SharedRotationDirection,
) -> bool {
    zetta_layout::rotate_pane(layout, pane_id, direction.into())
}

pub(super) fn can_rotate_layout_pane(
    layout: &BackgroundPaneLayout,
    pane_id: u64,
    direction: SharedRotationDirection,
) -> bool {
    rotate_layout_pane(&mut layout.clone(), pane_id, direction)
}
