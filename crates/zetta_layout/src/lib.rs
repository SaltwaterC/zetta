//! The pane-layout tree algebra shared by the application and the multiplexer.
//!
//! A tab's panes are a binary tree of splits, each with an axis and the first
//! child's share of it. The application keeps that tree as its own
//! `PaneLayout`, and the multiplexer keeps a shared session's as the
//! `BackgroundPaneLayout` it puts on the wire; both have to move, swap and
//! rotate panes identically, or a shared session would rearrange differently
//! for its viewers than for its owner. This crate is that one implementation,
//! written against [`LayoutTree`], a borrowed view of either tree's nodes.
//!
//! It depends on nothing but `serde`, so the daemon can link it and a build of
//! the application without the multiplexer still has it.

use serde::{Deserialize, Serialize};

/// The scale a split's `first_ratio` is expressed in: the first child's share
/// of the split, in thousandths. An integer keeps layouts comparable and keeps
/// floating-point drift from accumulating while a split is resized.
pub const SPLIT_RATIO_SCALE: u16 = 1_000;

/// How close two areas have to be to count as equal when choosing what a
/// rotation turns.
const ROTATION_AREA_EPSILON: f64 = 1e-9;

/// The direction a split divides its region in.
///
/// Serialised as the lowercase word, which is what the multiplexer's protocol
/// carried when the axis was a string.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitAxis {
    /// Children stacked top to bottom.
    Horizontal,
    /// Children side by side.
    Vertical,
}

impl std::fmt::Display for SplitAxis {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Horizontal => "horizontal",
            Self::Vertical => "vertical",
        })
    }
}

impl SplitAxis {
    /// The other axis.
    pub fn rotated(self) -> Self {
        match self {
            Self::Horizontal => Self::Vertical,
            Self::Vertical => Self::Horizontal,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneRotationDirection {
    Clockwise,
    CounterClockwise,
}

/// One node of a layout tree, borrowed.
pub enum Node<'a, T> {
    Pane(u64),
    Split {
        axis: SplitAxis,
        first_ratio: u16,
        first: &'a T,
        second: &'a T,
    },
}

/// One node of a layout tree, borrowed mutably.
pub enum NodeMut<'a, T> {
    Pane(&'a mut u64),
    Split {
        axis: &'a mut SplitAxis,
        first_ratio: &'a mut u16,
        first: &'a mut T,
        second: &'a mut T,
    },
}

/// A pane-layout tree the operations here can walk and rewrite.
pub trait LayoutTree: Sized {
    fn node(&self) -> Node<'_, Self>;
    fn node_mut(&mut self) -> NodeMut<'_, Self>;
}

/// Whether `pane_id` is one of the tree's panes.
pub fn contains_pane<T: LayoutTree>(tree: &T, pane_id: u64) -> bool {
    match tree.node() {
        Node::Pane(id) => id == pane_id,
        Node::Split { first, second, .. } => {
            contains_pane(first, pane_id) || contains_pane(second, pane_id)
        }
    }
}

/// Appends the tree's panes to `ids` in left-to-right, top-to-bottom order.
pub fn collect_pane_ids<T: LayoutTree>(tree: &T, ids: &mut Vec<u64>) {
    match tree.node() {
        Node::Pane(id) => ids.push(id),
        Node::Split { first, second, .. } => {
            collect_pane_ids(first, ids);
            collect_pane_ids(second, ids);
        }
    }
}

/// Exchanges the two panes wherever they are, whatever their ancestry. A pane
/// that is neither is left alone, so swapping with a missing pane renames the
/// other to it.
pub fn swap_pane_ids<T: LayoutTree>(tree: &mut T, first_id: u64, second_id: u64) {
    match tree.node_mut() {
        NodeMut::Pane(id) => {
            if *id == first_id {
                *id = second_id;
            } else if *id == second_id {
                *id = first_id;
            }
        }
        NodeMut::Split { first, second, .. } => {
            swap_pane_ids(first, first_id, second_id);
            swap_pane_ids(second, first_id, second_id);
        }
    }
}

/// Moves `pane_id` one step in `direction`, swapping it (and, when nested, its
/// whole subtree) with the sibling subtree of the nearest ancestor split on the
/// matching axis. Returns `false` when there is no such ancestor: the pane is
/// already at that edge of the layout.
pub fn move_pane<T: LayoutTree>(tree: &mut T, pane_id: u64, direction: PaneDirection) -> bool {
    let (axis, toward_first) = match direction {
        PaneDirection::Left => (SplitAxis::Vertical, true),
        PaneDirection::Right => (SplitAxis::Vertical, false),
        PaneDirection::Up => (SplitAxis::Horizontal, true),
        PaneDirection::Down => (SplitAxis::Horizontal, false),
    };
    move_pane_inner(tree, pane_id, axis, toward_first).unwrap_or(false)
}

/// `None` when `pane_id` is not under `tree` at all; otherwise whether some
/// split on the way down took the move.
fn move_pane_inner<T: LayoutTree>(
    tree: &mut T,
    pane_id: u64,
    axis: SplitAxis,
    toward_first: bool,
) -> Option<bool> {
    let NodeMut::Split {
        axis: split_axis,
        first_ratio,
        first,
        second,
    } = tree.node_mut()
    else {
        return None;
    };
    let in_first = if contains_pane(first, pane_id) {
        true
    } else if contains_pane(second, pane_id) {
        false
    } else {
        return None;
    };
    let child = if in_first { &mut *first } else { &mut *second };
    if let Some(handled) = move_pane_inner(child, pane_id, axis, toward_first) {
        return Some(handled);
    }
    (*split_axis == axis && in_first != toward_first).then(|| {
        std::mem::swap(first, second);
        *first_ratio = SPLIT_RATIO_SCALE - *first_ratio;
        true
    })
}

/// Rotates the part of the layout `pane_id` belongs to a quarter turn.
///
/// What turns is the smallest subtree around the pane that the pane is at
/// least as large as the rest of — so a focused large pane carries a
/// three-pane layout round with it, while a focused small pane turns only its
/// local split — or the whole layout when that is two panes or four equal ones.
/// Returns `false` when the pane is not in the layout or nothing can turn.
pub fn rotate_pane<T: LayoutTree>(
    tree: &mut T,
    pane_id: u64,
    direction: PaneRotationDirection,
) -> bool {
    let Some(pane_area) = pane_area(tree, pane_id, 1.) else {
        return false;
    };
    let Some(path) = rotation_target(tree, pane_id, pane_area, 1.) else {
        return false;
    };
    rotate_at_path(tree, &path, direction);
    true
}

/// The share of `area` that `pane_id` occupies, when it is in the tree.
fn pane_area<T: LayoutTree>(tree: &T, pane_id: u64, area: f64) -> Option<f64> {
    match tree.node() {
        Node::Pane(id) => (id == pane_id).then_some(area),
        Node::Split {
            first_ratio,
            first,
            second,
            ..
        } => {
            let first_area = first_share(area, first_ratio);
            pane_area(first, pane_id, first_area)
                .or_else(|| pane_area(second, pane_id, area - first_area))
        }
    }
}

fn first_share(area: f64, first_ratio: u16) -> f64 {
    area * f64::from(first_ratio) / f64::from(SPLIT_RATIO_SCALE)
}

/// The path, as first-child choices from the root, to the subtree a rotation
/// around `pane_id` turns.
fn rotation_target<T: LayoutTree>(
    tree: &T,
    pane_id: u64,
    pane_area: f64,
    area: f64,
) -> Option<Vec<bool>> {
    if has_four_equal_panes(tree, area) || is_two_pane_split(tree) {
        return Some(Vec::new());
    }
    let Node::Split {
        first_ratio,
        first,
        second,
        ..
    } = tree.node()
    else {
        return None;
    };
    let first_area = first_share(area, first_ratio);
    let (child, child_area, sibling_area, child_is_first) = if contains_pane(first, pane_id) {
        (first, first_area, area - first_area, true)
    } else if contains_pane(second, pane_id) {
        (second, area - first_area, first_area, false)
    } else {
        return None;
    };
    if let Some(mut path) = rotation_target(child, pane_id, pane_area, child_area) {
        path.insert(0, child_is_first);
        return Some(path);
    }
    // A pane can rotate with the complete subtree on the other side only when
    // it is at least as large as that subtree.
    (pane_area + ROTATION_AREA_EPSILON >= sibling_area).then_some(Vec::new())
}

fn is_two_pane_split<T: LayoutTree>(tree: &T) -> bool {
    matches!(
        tree.node(),
        Node::Split { first, second, .. }
            if matches!(first.node(), Node::Pane(_)) && matches!(second.node(), Node::Pane(_))
    )
}

fn has_four_equal_panes<T: LayoutTree>(tree: &T, area: f64) -> bool {
    let mut areas = Vec::with_capacity(4);
    collect_leaf_areas(tree, area, &mut areas);
    areas.len() == 4
        && areas
            .iter()
            .all(|candidate| (*candidate - areas[0]).abs() <= ROTATION_AREA_EPSILON)
}

fn collect_leaf_areas<T: LayoutTree>(tree: &T, area: f64, areas: &mut Vec<f64>) {
    match tree.node() {
        Node::Pane(_) => areas.push(area),
        Node::Split {
            first_ratio,
            first,
            second,
            ..
        } => {
            let first_area = first_share(area, first_ratio);
            collect_leaf_areas(first, first_area, areas);
            collect_leaf_areas(second, area - first_area, areas);
        }
    }
}

fn rotate_at_path<T: LayoutTree>(tree: &mut T, path: &[bool], direction: PaneRotationDirection) {
    let Some((&into_first, remaining)) = path.split_first() else {
        rotate_geometry(tree, direction);
        return;
    };
    if let NodeMut::Split { first, second, .. } = tree.node_mut() {
        rotate_at_path(
            if into_first { first } else { second },
            remaining,
            direction,
        );
    }
}

/// Turns every split under `tree` a quarter turn: each axis flips, and the
/// children swap where the turn carries the first one past the second.
fn rotate_geometry<T: LayoutTree>(tree: &mut T, direction: PaneRotationDirection) {
    let NodeMut::Split {
        axis,
        first_ratio,
        first,
        second,
    } = tree.node_mut()
    else {
        return;
    };
    rotate_geometry(first, direction);
    rotate_geometry(second, direction);
    let reverse_children = matches!(
        (direction, *axis),
        (PaneRotationDirection::Clockwise, SplitAxis::Horizontal)
            | (PaneRotationDirection::CounterClockwise, SplitAxis::Vertical)
    );
    *axis = axis.rotated();
    if reverse_children {
        std::mem::swap(first, second);
        *first_ratio = SPLIT_RATIO_SCALE - *first_ratio;
    }
}

#[cfg(test)]
#[path = "tests/lib.rs"]
mod tests;
