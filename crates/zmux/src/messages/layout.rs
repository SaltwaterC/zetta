//! Validating and rearranging a shared session's pane layout: the checks a
//! summary has to pass, and the move, swap and rotate operations the daemon
//! applies to the tree.

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
    collect_layout_pane_ids(&summary.layout, &mut layout_ids);
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
            axis,
            first_ratio,
            first,
            second,
        } => {
            anyhow::ensure!(
                axis == "horizontal" || axis == "vertical",
                "invalid shared split axis {axis}"
            );
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

pub(super) fn collect_layout_pane_ids(layout: &BackgroundPaneLayout, ids: &mut Vec<u64>) {
    match layout {
        BackgroundPaneLayout::Pane { pane_id } => ids.push(*pane_id),
        BackgroundPaneLayout::Split { first, second, .. } => {
            collect_layout_pane_ids(first, ids);
            collect_layout_pane_ids(second, ids);
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
                    axis: axis.clone(),
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

pub(super) fn layout_contains(layout: &BackgroundPaneLayout, pane_id: u64) -> bool {
    match layout {
        BackgroundPaneLayout::Pane { pane_id: id } => *id == pane_id,
        BackgroundPaneLayout::Split { first, second, .. } => {
            layout_contains(first, pane_id) || layout_contains(second, pane_id)
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
            if layout_contains(first, first_pane_id) && layout_contains(second, second_pane_id) {
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
            if layout_contains(first, first_pane_id) && layout_contains(second, second_pane_id) {
                Some(first_ratio)
            } else if layout_contains(first, first_pane_id)
                || layout_contains(first, second_pane_id)
            {
                find_divider_mut(first, first_pane_id, second_pane_id)
            } else {
                find_divider_mut(second, first_pane_id, second_pane_id)
            }
        }
    }
}

pub(super) fn swap_layout_panes(layout: &mut BackgroundPaneLayout, first_id: u64, second_id: u64) {
    match layout {
        BackgroundPaneLayout::Pane { pane_id } if *pane_id == first_id => *pane_id = second_id,
        BackgroundPaneLayout::Pane { pane_id } if *pane_id == second_id => *pane_id = first_id,
        BackgroundPaneLayout::Pane { .. } => {}
        BackgroundPaneLayout::Split { first, second, .. } => {
            swap_layout_panes(first, first_id, second_id);
            swap_layout_panes(second, first_id, second_id);
        }
    }
}

pub fn move_layout_pane(
    layout: &mut BackgroundPaneLayout,
    pane_id: u64,
    direction: SharedPaneDirection,
) -> bool {
    let (axis, toward_first) = match direction {
        SharedPaneDirection::Left => ("vertical", true),
        SharedPaneDirection::Right => ("vertical", false),
        SharedPaneDirection::Up => ("horizontal", true),
        SharedPaneDirection::Down => ("horizontal", false),
    };
    move_layout_pane_inner(layout, pane_id, axis, toward_first).unwrap_or(false)
}

pub(super) fn can_move_layout_pane(
    layout: &BackgroundPaneLayout,
    pane_id: u64,
    direction: SharedPaneDirection,
) -> bool {
    move_layout_pane(&mut layout.clone(), pane_id, direction)
}

pub(super) fn move_layout_pane_inner(
    layout: &mut BackgroundPaneLayout,
    pane_id: u64,
    axis: &str,
    toward_first: bool,
) -> Option<bool> {
    let BackgroundPaneLayout::Split {
        axis: split_axis,
        first_ratio,
        first,
        second,
    } = layout
    else {
        return None;
    };
    if layout_contains(first, pane_id) {
        if let Some(handled) = move_layout_pane_inner(first, pane_id, axis, toward_first) {
            return Some(handled);
        }
        (split_axis == axis && !toward_first).then(|| {
            std::mem::swap(first, second);
            *first_ratio = crate::protocol::BACKGROUND_PANE_SPLIT_RATIO_SCALE - *first_ratio;
            true
        })
    } else if layout_contains(second, pane_id) {
        if let Some(handled) = move_layout_pane_inner(second, pane_id, axis, toward_first) {
            return Some(handled);
        }
        (split_axis == axis && toward_first).then(|| {
            std::mem::swap(first, second);
            *first_ratio = crate::protocol::BACKGROUND_PANE_SPLIT_RATIO_SCALE - *first_ratio;
            true
        })
    } else {
        None
    }
}

pub fn rotate_layout_pane(
    layout: &mut BackgroundPaneLayout,
    pane_id: u64,
    direction: SharedRotationDirection,
) -> bool {
    let Some(active_area) = layout_pane_area(layout, pane_id, 1.) else {
        return false;
    };
    let Some(path) = rotation_target(layout, pane_id, active_area, 1.) else {
        return false;
    };
    rotate_at_path(layout, &path, direction);
    true
}

pub(super) fn can_rotate_layout_pane(
    layout: &BackgroundPaneLayout,
    pane_id: u64,
    direction: SharedRotationDirection,
) -> bool {
    rotate_layout_pane(&mut layout.clone(), pane_id, direction)
}

pub(super) fn layout_pane_area(
    layout: &BackgroundPaneLayout,
    pane_id: u64,
    area: f64,
) -> Option<f64> {
    match layout {
        BackgroundPaneLayout::Pane { pane_id: id } => (*id == pane_id).then_some(area),
        BackgroundPaneLayout::Split {
            first_ratio,
            first,
            second,
            ..
        } => {
            let first_area = area * f64::from(*first_ratio)
                / f64::from(crate::protocol::BACKGROUND_PANE_SPLIT_RATIO_SCALE);
            layout_pane_area(first, pane_id, first_area)
                .or_else(|| layout_pane_area(second, pane_id, area - first_area))
        }
    }
}

pub(super) fn rotation_target(
    layout: &BackgroundPaneLayout,
    pane_id: u64,
    active_area: f64,
    area: f64,
) -> Option<Vec<bool>> {
    if has_four_equal_panes(layout, area) || is_two_pane_split(layout) {
        return Some(Vec::new());
    }
    let BackgroundPaneLayout::Split {
        first_ratio,
        first,
        second,
        ..
    } = layout
    else {
        return None;
    };
    let first_area = area * f64::from(*first_ratio)
        / f64::from(crate::protocol::BACKGROUND_PANE_SPLIT_RATIO_SCALE);
    let (child, child_area, sibling_area, child_is_first) = if layout_contains(first, pane_id) {
        (first, first_area, area - first_area, true)
    } else if layout_contains(second, pane_id) {
        (second, area - first_area, first_area, false)
    } else {
        return None;
    };
    if let Some(mut path) = rotation_target(child, pane_id, active_area, child_area) {
        path.insert(0, child_is_first);
        return Some(path);
    }
    (active_area + 1e-9 >= sibling_area).then_some(Vec::new())
}

pub(super) fn is_two_pane_split(layout: &BackgroundPaneLayout) -> bool {
    matches!(
        layout,
        BackgroundPaneLayout::Split { first, second, .. }
            if matches!(first.as_ref(), BackgroundPaneLayout::Pane { .. })
                && matches!(second.as_ref(), BackgroundPaneLayout::Pane { .. })
    )
}

pub(super) fn has_four_equal_panes(layout: &BackgroundPaneLayout, area: f64) -> bool {
    let mut areas = Vec::with_capacity(4);
    collect_leaf_areas(layout, area, &mut areas);
    areas.len() == 4
        && areas
            .iter()
            .all(|candidate| (*candidate - areas[0]).abs() <= 1e-9)
}

pub(super) fn collect_leaf_areas(layout: &BackgroundPaneLayout, area: f64, areas: &mut Vec<f64>) {
    match layout {
        BackgroundPaneLayout::Pane { .. } => areas.push(area),
        BackgroundPaneLayout::Split {
            first_ratio,
            first,
            second,
            ..
        } => {
            let first_area = area * f64::from(*first_ratio)
                / f64::from(crate::protocol::BACKGROUND_PANE_SPLIT_RATIO_SCALE);
            collect_leaf_areas(first, first_area, areas);
            collect_leaf_areas(second, area - first_area, areas);
        }
    }
}

pub(super) fn rotate_at_path(
    layout: &mut BackgroundPaneLayout,
    path: &[bool],
    direction: SharedRotationDirection,
) {
    if let Some((first_path, remaining)) = path.split_first() {
        if let BackgroundPaneLayout::Split { first, second, .. } = layout {
            rotate_at_path(
                if *first_path { first } else { second },
                remaining,
                direction,
            );
        }
        return;
    }
    rotate_layout_geometry(layout, direction);
}

pub(super) fn rotate_layout_geometry(
    layout: &mut BackgroundPaneLayout,
    direction: SharedRotationDirection,
) {
    let BackgroundPaneLayout::Split {
        axis,
        first_ratio,
        first,
        second,
    } = layout
    else {
        return;
    };
    rotate_layout_geometry(first, direction);
    rotate_layout_geometry(second, direction);
    let reverse_children = matches!(
        (direction, axis.as_str()),
        (SharedRotationDirection::Clockwise, "horizontal")
            | (SharedRotationDirection::CounterClockwise, "vertical")
    );
    *axis = if axis == "horizontal" {
        "vertical"
    } else {
        "horizontal"
    }
    .to_owned();
    if reverse_children {
        std::mem::swap(first, second);
        *first_ratio = crate::protocol::BACKGROUND_PANE_SPLIT_RATIO_SCALE - *first_ratio;
    }
}
