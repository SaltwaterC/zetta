//! The pane layout tree: how a tab's region is divided, and every operation
//! that reshapes it.
//!
//! A `PaneLayout` is a binary tree of splits with a ratio per split, so a
//! layout operation is a path through it rather than a coordinate: splitting,
//! removing, rotating, moving, swapping and resizing all rewrite the tree, and
//! the regions a renderer draws are computed from it.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SplitPosition {
    Before,
    After,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PaneDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PaneRotationDirection {
    Clockwise,
    CounterClockwise,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PaneRegion {
    pub(crate) id: u64,
    pub(crate) left: f32,
    pub(crate) right: f32,
    pub(crate) top: f32,
    pub(crate) bottom: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PaneResizeBoundary {
    /// The fraction of the complete pane layout occupied by the split along
    /// the axis being resized.
    pub(crate) parent_fraction: f32,
    /// Whether the pane being resized is in the split's first child. Arrow
    /// keys move a screen-directional edge, so the second child uses the
    /// inverse size delta.
    pub(crate) active_is_first: bool,
    pub(crate) sibling_panes: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PaneLayout {
    Pane(u64),
    Split {
        axis: SplitAxis,
        /// The first child's share of the available extent on `axis`, scaled
        /// by [`PANE_SPLIT_RATIO_SCALE`]. An integer keeps layouts comparable
        /// and avoids accumulating floating point drift while resizing.
        first_ratio: u16,
        first: Box<PaneLayout>,
        second: Box<PaneLayout>,
    },
}

pub(crate) fn background_pane_layout(layout: &PaneLayout) -> BackgroundPaneLayout {
    match layout {
        PaneLayout::Pane(pane_id) => BackgroundPaneLayout::Pane { pane_id: *pane_id },
        PaneLayout::Split {
            axis,
            first,
            second,
            ..
        } => BackgroundPaneLayout::Split {
            axis: match axis {
                SplitAxis::Horizontal => "horizontal",
                SplitAxis::Vertical => "vertical",
            }
            .to_owned(),
            first: Box::new(background_pane_layout(first)),
            second: Box::new(background_pane_layout(second)),
        },
    }
}

impl PaneLayout {
    pub(crate) fn rotate_pane(
        &mut self,
        active_pane: u64,
        direction: PaneRotationDirection,
    ) -> bool {
        let Some(active_area) = self.pane_area(active_pane, 1.) else {
            return false;
        };
        let Some(path) = self.rotation_target(active_pane, active_area, 1.) else {
            return false;
        };
        self.rotate_at_path(&path, direction);
        true
    }

    fn pane_area(&self, pane_id: u64, area: f64) -> Option<f64> {
        match self {
            Self::Pane(id) => (*id == pane_id).then_some(area),
            Self::Split {
                first_ratio,
                first,
                second,
                ..
            } => {
                let first_area = area * f64::from(*first_ratio) / f64::from(PANE_SPLIT_RATIO_SCALE);
                first
                    .pane_area(pane_id, first_area)
                    .or_else(|| second.pane_area(pane_id, area - first_area))
            }
        }
    }

    fn rotation_target(&self, active_pane: u64, active_area: f64, area: f64) -> Option<Vec<bool>> {
        if self.has_four_equal_panes(area) || self.is_two_pane_split() {
            return Some(Vec::new());
        }

        let Self::Split {
            first_ratio,
            first,
            second,
            ..
        } = self
        else {
            return None;
        };

        let first_area = area * f64::from(*first_ratio) / f64::from(PANE_SPLIT_RATIO_SCALE);
        let (child, child_area, sibling_area, child_is_first) = if first.contains_pane(active_pane)
        {
            (first, first_area, area - first_area, true)
        } else if second.contains_pane(active_pane) {
            (second, area - first_area, first_area, false)
        } else {
            return None;
        };

        if let Some(mut path) = child.rotation_target(active_pane, active_area, child_area) {
            path.insert(0, child_is_first);
            return Some(path);
        }

        // A pane can rotate with the complete subtree on the other side only
        // when it is at least as large as that subtree. This is what makes a
        // focused large pane dominate a three-pane layout while allowing a
        // focused small pane to rotate its local equal split instead.
        (active_area + PANE_ROTATION_AREA_EPSILON >= sibling_area).then_some(Vec::new())
    }

    fn is_two_pane_split(&self) -> bool {
        matches!(
            self,
            Self::Split {
                first,
                second,
                ..
            } if matches!(first.as_ref(), Self::Pane(_)) && matches!(second.as_ref(), Self::Pane(_))
        )
    }

    fn has_four_equal_panes(&self, area: f64) -> bool {
        let mut areas = Vec::with_capacity(4);
        self.collect_leaf_areas(area, &mut areas);
        areas.len() == 4
            && areas
                .iter()
                .all(|candidate| (*candidate - areas[0]).abs() <= PANE_ROTATION_AREA_EPSILON)
    }

    fn collect_leaf_areas(&self, area: f64, areas: &mut Vec<f64>) {
        match self {
            Self::Pane(_) => areas.push(area),
            Self::Split {
                first_ratio,
                first,
                second,
                ..
            } => {
                let first_area = area * f64::from(*first_ratio) / f64::from(PANE_SPLIT_RATIO_SCALE);
                first.collect_leaf_areas(first_area, areas);
                second.collect_leaf_areas(area - first_area, areas);
            }
        }
    }

    fn rotate_at_path(&mut self, path: &[bool], direction: PaneRotationDirection) {
        if let Some((first, second)) = path.split_first() {
            if let Self::Split {
                first: first_child,
                second: second_child,
                ..
            } = self
            {
                if *first {
                    first_child.rotate_at_path(second, direction);
                } else {
                    second_child.rotate_at_path(second, direction);
                }
            }
            return;
        }

        self.rotate_geometry(direction);
    }

    fn rotate_geometry(&mut self, direction: PaneRotationDirection) {
        let Self::Split {
            axis,
            first_ratio,
            first,
            second,
        } = self
        else {
            return;
        };

        first.rotate_geometry(direction);
        second.rotate_geometry(direction);

        let reverse_children = matches!(
            (direction, *axis),
            (PaneRotationDirection::Clockwise, SplitAxis::Horizontal)
                | (PaneRotationDirection::CounterClockwise, SplitAxis::Vertical)
        );
        *axis = match axis {
            SplitAxis::Horizontal => SplitAxis::Vertical,
            SplitAxis::Vertical => SplitAxis::Horizontal,
        };
        if reverse_children {
            std::mem::swap(first, second);
            *first_ratio = PANE_SPLIT_RATIO_SCALE - *first_ratio;
        }
    }

    pub(super) fn remap_pane_ids(&mut self, pane_ids: &HashMap<u64, u64>) {
        match self {
            Self::Pane(pane_id) => *pane_id = pane_ids[pane_id],
            Self::Split { first, second, .. } => {
                first.remap_pane_ids(pane_ids);
                second.remap_pane_ids(pane_ids);
            }
        }
    }

    pub(crate) fn tiled(pane_ids: &[u64]) -> Option<Self> {
        fn build(pane_ids: &[u64], axis: SplitAxis) -> PaneLayout {
            if let [pane_id] = pane_ids {
                return PaneLayout::Pane(*pane_id);
            }
            let midpoint = if pane_ids.len() == 3 {
                1
            } else {
                pane_ids.len().div_ceil(2)
            };
            let next_axis = match axis {
                SplitAxis::Horizontal => SplitAxis::Vertical,
                SplitAxis::Vertical => SplitAxis::Horizontal,
            };
            PaneLayout::Split {
                axis,
                first_ratio: DEFAULT_PANE_SPLIT_RATIO,
                first: Box::new(build(&pane_ids[..midpoint], next_axis)),
                second: Box::new(build(&pane_ids[midpoint..], next_axis)),
            }
        }

        (!pane_ids.is_empty()).then(|| build(pane_ids, SplitAxis::Vertical))
    }

    pub(crate) fn split(
        &mut self,
        target: u64,
        axis: SplitAxis,
        new_pane: u64,
        position: SplitPosition,
    ) -> bool {
        match self {
            Self::Pane(id) if *id == target => {
                let (first, second) = match position {
                    SplitPosition::Before => (new_pane, target),
                    SplitPosition::After => (target, new_pane),
                };
                *self = Self::Split {
                    axis,
                    first_ratio: DEFAULT_PANE_SPLIT_RATIO,
                    first: Box::new(Self::Pane(first)),
                    second: Box::new(Self::Pane(second)),
                };
                true
            }
            Self::Pane(_) => false,
            Self::Split { first, second, .. } => {
                first.split(target, axis, new_pane, position)
                    || second.split(target, axis, new_pane, position)
            }
        }
    }

    pub(crate) fn replace(&mut self, target: u64, replacement: PaneLayout) -> bool {
        let mut replacement = Some(replacement);
        self.replace_inner(target, &mut replacement)
    }

    pub(crate) fn replace_inner(
        &mut self,
        target: u64,
        replacement: &mut Option<PaneLayout>,
    ) -> bool {
        match self {
            Self::Pane(id) if *id == target => {
                *self = replacement
                    .take()
                    .expect("a pane layout replacement must only be consumed once");
                true
            }
            Self::Pane(_) => false,
            Self::Split { first, second, .. } => {
                first.replace_inner(target, replacement)
                    || second.replace_inner(target, replacement)
            }
        }
    }

    pub(crate) fn from_template(
        template: &PaneSplitTemplate,
        pane_ids: &mut impl Iterator<Item = u64>,
    ) -> Self {
        match template {
            PaneSplitTemplate::Pane(_) => Self::Pane(
                pane_ids
                    .next()
                    .expect("pane template and allocated IDs must have equal lengths"),
            ),
            PaneSplitTemplate::Split {
                axis,
                first,
                second,
            } => Self::Split {
                axis: match axis {
                    PaneSplitAxis::Horizontal => SplitAxis::Horizontal,
                    PaneSplitAxis::Vertical => SplitAxis::Vertical,
                },
                first_ratio: DEFAULT_PANE_SPLIT_RATIO,
                first: Box::new(Self::from_template(first, pane_ids)),
                second: Box::new(Self::from_template(second, pane_ids)),
            },
        }
    }

    pub(crate) fn without(self, target: u64) -> Option<Self> {
        match self {
            Self::Pane(id) => (id != target).then_some(Self::Pane(id)),
            Self::Split {
                axis,
                first_ratio,
                first,
                second,
            } => match (first.without(target), second.without(target)) {
                (Some(first), Some(second)) => Some(Self::Split {
                    axis,
                    first_ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(layout), None) | (None, Some(layout)) => Some(layout),
                (None, None) => None,
            },
        }
    }

    pub(crate) fn without_all(&self, targets: &HashSet<u64>) -> Option<Self> {
        match self {
            Self::Pane(id) => (!targets.contains(id)).then_some(Self::Pane(*id)),
            Self::Split {
                axis,
                first_ratio,
                first,
                second,
            } => match (first.without_all(targets), second.without_all(targets)) {
                (Some(first), Some(second)) => Some(Self::Split {
                    axis: *axis,
                    first_ratio: *first_ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(layout), None) | (None, Some(layout)) => Some(layout),
                (None, None) => None,
            },
        }
    }

    pub(crate) fn first_pane(&self) -> u64 {
        match self {
            Self::Pane(id) => *id,
            Self::Split { first, .. } => first.first_pane(),
        }
    }

    pub(crate) fn regions(&self) -> Vec<PaneRegion> {
        let mut regions = Vec::new();
        self.collect_regions(0., 0., 1., 1., &mut regions);
        regions
    }

    pub(crate) fn collect_regions(
        &self,
        left: f32,
        top: f32,
        width: f32,
        height: f32,
        regions: &mut Vec<PaneRegion>,
    ) {
        match self {
            Self::Pane(id) => regions.push(PaneRegion {
                id: *id,
                left,
                right: left + width,
                top,
                bottom: top + height,
            }),
            Self::Split {
                axis: SplitAxis::Horizontal,
                first_ratio,
                first,
                second,
            } => {
                let first_height = height * Self::ratio_fraction(*first_ratio);
                first.collect_regions(left, top, width, first_height, regions);
                second.collect_regions(
                    left,
                    top + first_height,
                    width,
                    height - first_height,
                    regions,
                );
            }
            Self::Split {
                axis: SplitAxis::Vertical,
                first_ratio,
                first,
                second,
            } => {
                let first_width = width * Self::ratio_fraction(*first_ratio);
                first.collect_regions(left, top, first_width, height, regions);
                second.collect_regions(
                    left + first_width,
                    top,
                    width - first_width,
                    height,
                    regions,
                );
            }
        }
    }

    pub(crate) fn ratio_fraction(first_ratio: u16) -> f32 {
        f32::from(first_ratio) / f32::from(PANE_SPLIT_RATIO_SCALE)
    }

    /// Finds the closest split on `axis` that can resize `pane_id` against
    /// its sibling subtree.
    pub(crate) fn resize_boundary(
        &self,
        pane_id: u64,
        axis: SplitAxis,
    ) -> Option<PaneResizeBoundary> {
        self.resize_boundary_inner(pane_id, axis, 1.)
    }

    fn resize_boundary_inner(
        &self,
        pane_id: u64,
        axis: SplitAxis,
        parent_fraction: f32,
    ) -> Option<PaneResizeBoundary> {
        let Self::Split {
            axis: split_axis,
            first_ratio,
            first,
            second,
        } = self
        else {
            return None;
        };
        let first_fraction = Self::ratio_fraction(*first_ratio);
        if first.contains_pane(pane_id) {
            return first
                .resize_boundary_inner(pane_id, axis, parent_fraction * first_fraction)
                .or_else(|| {
                    (*split_axis == axis).then(|| PaneResizeBoundary {
                        parent_fraction,
                        active_is_first: true,
                        sibling_panes: second.pane_ids(),
                    })
                });
        }
        if second.contains_pane(pane_id) {
            return second
                .resize_boundary_inner(pane_id, axis, parent_fraction * (1. - first_fraction))
                .or_else(|| {
                    (*split_axis == axis).then(|| PaneResizeBoundary {
                        parent_fraction,
                        active_is_first: false,
                        sibling_panes: first.pane_ids(),
                    })
                });
        }
        None
    }

    /// Adjust the closest resize boundary for a pane. A positive delta grows
    /// the pane and a negative delta shrinks it. The delta is expressed as a
    /// fraction of that split's available size.
    pub(crate) fn adjust_resize_boundary(
        &mut self,
        pane_id: u64,
        axis: SplitAxis,
        delta: f32,
    ) -> bool {
        self.adjust_resize_boundary_inner(pane_id, axis, delta)
            .unwrap_or(false)
    }

    /// Returns the panes on both sides of the split identified by its first
    /// pane in each child. The pair uniquely identifies a split in a layout.
    pub(crate) fn split_panes(
        &self,
        first_pane: u64,
        second_pane: u64,
        axis: SplitAxis,
    ) -> Option<(Vec<u64>, Vec<u64>)> {
        let Self::Split {
            axis: split_axis,
            first,
            second,
            ..
        } = self
        else {
            return None;
        };
        if *split_axis == axis
            && first.first_pane() == first_pane
            && second.first_pane() == second_pane
        {
            return Some((first.pane_ids(), second.pane_ids()));
        }
        first
            .split_panes(first_pane, second_pane, axis)
            .or_else(|| second.split_panes(first_pane, second_pane, axis))
    }

    pub(crate) fn split_ratio(
        &self,
        first_pane: u64,
        second_pane: u64,
        axis: SplitAxis,
    ) -> Option<f32> {
        let Self::Split {
            axis: split_axis,
            first_ratio,
            first,
            second,
        } = self
        else {
            return None;
        };
        if *split_axis == axis
            && first.first_pane() == first_pane
            && second.first_pane() == second_pane
        {
            return Some(Self::ratio_fraction(*first_ratio));
        }
        first
            .split_ratio(first_pane, second_pane, axis)
            .or_else(|| second.split_ratio(first_pane, second_pane, axis))
    }

    /// Adjusts one exact split, rather than the nearest matching split for a
    /// pane. Mouse gutters use this to avoid changing a nested parallel split.
    pub(crate) fn adjust_split_ratio(
        &mut self,
        first_pane: u64,
        second_pane: u64,
        axis: SplitAxis,
        delta: f32,
    ) -> bool {
        let Self::Split {
            axis: split_axis,
            first_ratio,
            first,
            second,
        } = self
        else {
            return false;
        };
        if *split_axis == axis
            && first.first_pane() == first_pane
            && second.first_pane() == second_pane
        {
            return Self::adjust_first_ratio(first_ratio, delta);
        }
        first.adjust_split_ratio(first_pane, second_pane, axis, delta)
            || second.adjust_split_ratio(first_pane, second_pane, axis, delta)
    }

    fn adjust_resize_boundary_inner(
        &mut self,
        pane_id: u64,
        axis: SplitAxis,
        delta: f32,
    ) -> Option<bool> {
        let Self::Split {
            axis: split_axis,
            first_ratio,
            first,
            second,
        } = self
        else {
            return None;
        };
        if first.contains_pane(pane_id) {
            if let Some(result) = first.adjust_resize_boundary_inner(pane_id, axis, delta) {
                return Some(result);
            }
            if *split_axis == axis {
                return Some(Self::adjust_first_ratio(first_ratio, delta));
            }
        } else if second.contains_pane(pane_id) {
            if let Some(result) = second.adjust_resize_boundary_inner(pane_id, axis, delta) {
                return Some(result);
            }
            if *split_axis == axis {
                return Some(Self::adjust_first_ratio(first_ratio, -delta));
            }
        }
        None
    }

    fn adjust_first_ratio(first_ratio: &mut u16, delta: f32) -> bool {
        let delta = (delta * f32::from(PANE_SPLIT_RATIO_SCALE)).round() as i32;
        if delta == 0 {
            return false;
        }
        let ratio = i32::from(*first_ratio);
        let clamped = (ratio + delta).clamp(1, i32::from(PANE_SPLIT_RATIO_SCALE - 1));
        if clamped == ratio {
            return false;
        }
        *first_ratio = clamped as u16;
        true
    }

    fn contains_pane(&self, pane_id: u64) -> bool {
        match self {
            Self::Pane(id) => *id == pane_id,
            Self::Split { first, second, .. } => {
                first.contains_pane(pane_id) || second.contains_pane(pane_id)
            }
        }
    }

    fn pane_ids(&self) -> Vec<u64> {
        match self {
            Self::Pane(id) => vec![*id],
            Self::Split { first, second, .. } => first
                .pane_ids()
                .into_iter()
                .chain(second.pane_ids())
                .collect(),
        }
    }

    /// Moves `active_pane` one step in `direction`, swapping it (and, when
    /// nested, its whole subtree) with the sibling subtree that occupies the
    /// nearest ancestor split on the matching axis. Returns `false` if there
    /// is no such ancestor (the pane is already at that edge of the layout).
    pub(crate) fn move_pane(&mut self, active_pane: u64, direction: PaneDirection) -> bool {
        let (axis, toward_first) = match direction {
            PaneDirection::Left => (SplitAxis::Vertical, true),
            PaneDirection::Right => (SplitAxis::Vertical, false),
            PaneDirection::Up => (SplitAxis::Horizontal, true),
            PaneDirection::Down => (SplitAxis::Horizontal, false),
        };
        self.move_pane_inner(active_pane, axis, toward_first)
            .unwrap_or(false)
    }

    fn move_pane_inner(
        &mut self,
        pane_id: u64,
        axis: SplitAxis,
        toward_first: bool,
    ) -> Option<bool> {
        let Self::Split {
            axis: split_axis,
            first_ratio,
            first,
            second,
        } = self
        else {
            return None;
        };
        if first.contains_pane(pane_id) {
            if let Some(handled) = first.move_pane_inner(pane_id, axis, toward_first) {
                return Some(handled);
            }
            (*split_axis == axis && !toward_first).then(|| {
                std::mem::swap(first, second);
                *first_ratio = PANE_SPLIT_RATIO_SCALE - *first_ratio;
                true
            })
        } else if second.contains_pane(pane_id) {
            if let Some(handled) = second.move_pane_inner(pane_id, axis, toward_first) {
                return Some(handled);
            }
            (*split_axis == axis && toward_first).then(|| {
                std::mem::swap(first, second);
                *first_ratio = PANE_SPLIT_RATIO_SCALE - *first_ratio;
                true
            })
        } else {
            None
        }
    }

    /// Swaps the positions of two panes anywhere in the layout, regardless of
    /// their split ancestry. Mouse-driven pane move drops a pane onto an
    /// arbitrary target, unlike the directional keyboard move above, so it
    /// cannot rely on a shared axis-matching ancestor.
    pub(crate) fn swap_panes(&mut self, first: u64, second: u64) -> bool {
        if first == second || !self.contains_pane(first) || !self.contains_pane(second) {
            return false;
        }
        self.swap_panes_inner(first, second);
        true
    }

    fn swap_panes_inner(&mut self, first: u64, second: u64) {
        match self {
            Self::Pane(id) => {
                if *id == first {
                    *id = second;
                } else if *id == second {
                    *id = first;
                }
            }
            Self::Split {
                first: first_child,
                second: second_child,
                ..
            } => {
                first_child.swap_panes_inner(first, second);
                second_child.swap_panes_inner(first, second);
            }
        }
    }

    /// Finds the pane to focus when moving `direction` from `active`.
    ///
    /// `recent` is the tab's focus history (oldest first, most recently
    /// focused last, matching [`Tab::focus_history`]). When several
    /// candidates are equally close — e.g. moving right out of a full-height
    /// pane into a column split evenly into top/bottom panes — the one
    /// focused most recently wins, so leaving a column and returning to it
    /// restores the pane last focused there instead of always landing on the
    /// first one in tree order.
    pub(crate) fn adjacent_pane(
        &self,
        active: u64,
        direction: PaneDirection,
        recent: &[u64],
    ) -> Option<u64> {
        let regions = self.regions();
        let source = regions.iter().find(|region| region.id == active)?;
        let source_x = (source.left + source.right) / 2.;
        let source_y = (source.top + source.bottom) / 2.;

        let candidates: Vec<(f32, u64)> = regions
            .iter()
            .filter(|candidate| candidate.id != active)
            .filter_map(|candidate| {
                let candidate_x = (candidate.left + candidate.right) / 2.;
                let candidate_y = (candidate.top + candidate.bottom) / 2.;
                let (primary, perpendicular) = match direction {
                    PaneDirection::Left if candidate_x < source_x => {
                        (source_x - candidate_x, (source_y - candidate_y).abs())
                    }
                    PaneDirection::Right if candidate_x > source_x => {
                        (candidate_x - source_x, (source_y - candidate_y).abs())
                    }
                    PaneDirection::Up if candidate_y < source_y => {
                        (source_y - candidate_y, (source_x - candidate_x).abs())
                    }
                    PaneDirection::Down if candidate_y > source_y => {
                        (candidate_y - source_y, (source_x - candidate_x).abs())
                    }
                    _ => return None,
                };
                Some((primary + perpendicular * 2., candidate.id))
            })
            .collect();

        let best_score = candidates
            .iter()
            .map(|(score, _)| *score)
            .min_by(f32::total_cmp)?;

        let mut best: Option<(isize, u64)> = None;
        for (score, id) in &candidates {
            if (*score - best_score).abs() > ADJACENT_PANE_TIE_EPSILON {
                continue;
            }
            let recency = recent
                .iter()
                .rposition(|recent_id| *recent_id == *id)
                .map_or(-1, |position| position as isize);
            if best.is_none_or(|(best_recency, _)| recency > best_recency) {
                best = Some((recency, *id));
            }
        }
        best.map(|(_, id)| id)
    }
}

#[cfg(test)]
#[path = "../tests/pane/layout.rs"]
mod tests;
