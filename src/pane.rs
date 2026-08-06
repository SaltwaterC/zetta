use super::*;

pub(crate) const MAX_PANES_PER_TAB: usize = 64;
pub(crate) const MAX_CONCURRENT_MULTI_COMMAND_SPAWNS: usize = 4;
pub(crate) const TERMINAL_SPAWN_NOTIFY_INTERVAL: Duration = Duration::from_millis(16);
pub(crate) const PANE_OUTPUT_DEFAULT_FILENAME: &str = "terminal-output.txt";
pub(crate) const PANE_SPLIT_RATIO_SCALE: u16 = 1_000;
pub(crate) const DEFAULT_PANE_SPLIT_RATIO: u16 = PANE_SPLIT_RATIO_SCALE / 2;
const PANE_ROTATION_AREA_EPSILON: f64 = 1e-9;

pub(crate) fn terminal_size_label(columns: usize, rows: usize) -> String {
    format!("{columns} × {rows}")
}

pub(crate) fn can_add_panes(current: usize, additional: usize) -> bool {
    current
        .checked_add(additional)
        .is_some_and(|total| total <= MAX_PANES_PER_TAB)
}

pub(crate) fn begin_coalesced_notification(pending: &mut bool) -> bool {
    if *pending {
        false
    } else {
        *pending = true;
        true
    }
}

pub(crate) fn begin_pane_output_save(in_progress: &mut bool) -> bool {
    if *in_progress {
        false
    } else {
        *in_progress = true;
        true
    }
}

pub(crate) fn finish_pane_output_save(in_progress: &mut bool) {
    *in_progress = false;
}

pub(crate) fn prepare_pane_launches<T>(
    pane_ids: impl IntoIterator<Item = u64>,
    mut prepare: impl FnMut(u64) -> T,
) -> Vec<(u64, T)> {
    pane_ids
        .into_iter()
        .map(|pane_id| (pane_id, prepare(pane_id)))
        .collect()
}

pub(crate) fn pane_layout_from_configured_template(
    templates: &HashMap<String, PaneSplitTemplate>,
    name: &str,
    pane_ids: &mut impl Iterator<Item = u64>,
) -> Option<PaneLayout> {
    templates
        .get(name)
        .map(|template| PaneLayout::from_template(template, pane_ids))
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = zetta)]
#[serde(deny_unknown_fields)]
pub(crate) struct OpenProfile {
    pub(crate) slot: usize,
}

pub(crate) struct TerminalPane {
    pub(crate) id: u64,
    pub(crate) label_number: usize,
    pub(crate) generated_label: Option<String>,
    pub(crate) custom_label: Option<String>,
    pub(crate) profile: Profile,
    pub(crate) terminal: Option<Entity<Terminal>>,
    pub(crate) view: Option<Entity<TerminalView>>,
    pub(crate) error: Option<String>,
    pub(crate) wsl_cwd_file: Option<PathBuf>,
    pub(crate) pending_command: Option<String>,
}

pub(crate) struct TerminalSpawnSettings {
    pub(crate) cursor_shape: terminal::terminal_settings::CursorShape,
    pub(crate) alternate_scroll: terminal::terminal_settings::AlternateScroll,
    pub(crate) max_scroll_history_lines: Option<usize>,
    pub(crate) path_hyperlink_regexes: Vec<String>,
    pub(crate) path_hyperlink_timeout_ms: u64,
}

pub(crate) struct QueuedTerminalLaunch {
    pub(crate) tab_id: u64,
    pub(crate) pane_id: u64,
    pub(crate) profile: Profile,
    pub(crate) working_directory: Option<PathBuf>,
    pub(crate) wsl_directory: Option<String>,
    pub(crate) wsl_cwd_file: Option<PathBuf>,
    pub(crate) terminal_theme: Option<Arc<Theme>>,
    pub(crate) settings: Arc<TerminalSpawnSettings>,
}

pub(crate) struct BoundedLaunchQueue<T> {
    pending: VecDeque<T>,
    in_flight: usize,
    limit: usize,
}

impl<T> BoundedLaunchQueue<T> {
    pub(crate) fn new(limit: usize) -> Self {
        assert!(limit > 0, "a launch queue must allow at least one launch");
        Self {
            pending: VecDeque::new(),
            in_flight: 0,
            limit,
        }
    }

    pub(crate) fn extend(&mut self, launches: impl IntoIterator<Item = T>) {
        self.pending.extend(launches);
    }

    pub(crate) fn pop_ready(&mut self) -> Option<T> {
        if self.in_flight >= self.limit {
            return None;
        }
        let launch = self.pending.pop_front()?;
        self.in_flight += 1;
        Some(launch)
    }

    pub(crate) fn complete(&mut self) {
        self.in_flight = self
            .in_flight
            .checked_sub(1)
            .expect("only an in-flight launch can complete");
    }
}

impl TerminalSpawnSettings {
    pub(crate) fn current(cx: &App) -> Self {
        let settings = TerminalSettings::get_global(cx);
        Self {
            cursor_shape: settings.cursor_shape,
            alternate_scroll: settings.alternate_scroll,
            max_scroll_history_lines: settings.max_scroll_history_lines,
            path_hyperlink_regexes: settings.path_hyperlink_regexes.clone(),
            path_hyperlink_timeout_ms: settings.path_hyperlink_timeout_ms,
        }
    }

    pub(crate) fn path_hyperlink_regexes(&mut self, final_spawn: bool) -> Vec<String> {
        clone_or_take_for_final_spawn(&mut self.path_hyperlink_regexes, final_spawn)
    }
}

pub(crate) fn clone_or_take_for_final_spawn<T: Clone + Default>(
    value: &mut T,
    final_spawn: bool,
) -> T {
    if final_spawn {
        std::mem::take(value)
    } else {
        value.clone()
    }
}

impl TerminalPane {
    pub(crate) fn label(&self) -> String {
        self.custom_label
            .clone()
            .or_else(|| self.generated_label.clone())
            .unwrap_or_else(|| format!("Pane {}", self.label_number))
    }

    pub(crate) fn wsl_working_directory(&self, cx: &App) -> Option<String> {
        if let Some(directory) = self.terminal.as_ref().and_then(|terminal| {
            terminal
                .read(cx)
                .reported_working_directory()
                .map(str::to_owned)
        }) {
            return Some(directory);
        }

        let path = self.wsl_cwd_file.as_ref()?;
        let directory = fs::read_to_string(path).ok()?;
        let directory = directory.trim_end_matches(['\r', '\n']);
        (directory.starts_with('/') && !directory.contains(['\r', '\n', '\0']))
            .then(|| directory.to_owned())
    }

    pub(crate) fn working_directory(&self, cx: &App) -> Option<PathBuf> {
        let terminal = self.terminal.as_ref()?.read(cx);
        if let Some((root, _)) = msys2_profile(&self.profile.command)
            && let Some(directory) = terminal.reported_working_directory()
            && let Some(directory) = msys2_path_to_windows(&root, directory)
        {
            return Some(directory);
        }
        terminal.working_directory()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy)]
pub(crate) enum SplitPosition {
    Before,
    After,
}

#[derive(Clone, Copy)]
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

    fn remap_pane_ids(&mut self, pane_ids: &HashMap<u64, u64>) {
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
            PaneSplitTemplate::Pane => Self::Pane(
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

    fn move_pane_inner(&mut self, pane_id: u64, axis: SplitAxis, toward_first: bool) -> Option<bool> {
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

    pub(crate) fn adjacent_pane(&self, active: u64, direction: PaneDirection) -> Option<u64> {
        let regions = self.regions();
        let source = regions.iter().find(|region| region.id == active)?;
        let source_x = (source.left + source.right) / 2.;
        let source_y = (source.top + source.bottom) / 2.;

        regions
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
            .min_by(|(left_score, _), (right_score, _)| left_score.total_cmp(right_score))
            .map(|(_, id)| id)
    }
}

pub(crate) enum TabClosePolicy {
    Close,
    Background {
        authentication: Option<SessionAuthentication>,
    },
}

impl TabClosePolicy {
    pub(crate) fn background_authentication(&self) -> Option<Option<SessionAuthentication>> {
        match self {
            Self::Close => None,
            Self::Background { authentication } => Some(authentication.clone()),
        }
    }
}

pub(crate) struct Tab {
    pub(crate) id: u64,
    pub(crate) panes: Vec<TerminalPane>,
    pub(crate) pane_indices: HashMap<u64, usize>,
    pub(crate) next_pane_label: usize,
    pub(crate) layout: PaneLayout,
    pub(crate) active_pane: u64,
    pub(crate) focus_history: Vec<u64>,
    pub(crate) maximized_pane: Option<u64>,
    pub(crate) minimized_panes: Vec<u64>,
    pub(crate) selected_minimized_pane: Option<u64>,
    pub(crate) broadcast_input: bool,
    pub(crate) close_policy: TabClosePolicy,
    pub(crate) custom_title: Option<String>,
    pub(crate) icon: Option<IconName>,
    pub(crate) renaming_pane: Option<u64>,
    pub(crate) rename_buffer: Option<String>,
    pub(crate) rename_cursor: usize,
    pub(crate) rename_select_all: bool,
}

impl Tab {
    pub(crate) fn reassign_ids(&mut self, tab_id: u64, next_pane_id: &mut u64) {
        self.id = tab_id;
        let pane_ids = self
            .panes
            .iter_mut()
            .map(|pane| {
                let old_id = pane.id;
                pane.id = *next_pane_id;
                *next_pane_id += 1;
                (old_id, pane.id)
            })
            .collect::<HashMap<_, _>>();
        self.pane_indices = self
            .panes
            .iter()
            .enumerate()
            .map(|(index, pane)| (pane.id, index))
            .collect();
        self.layout.remap_pane_ids(&pane_ids);
        self.active_pane = pane_ids[&self.active_pane];
        self.focus_history = self
            .focus_history
            .iter()
            .filter_map(|pane_id| pane_ids.get(pane_id).copied())
            .collect();
        self.maximized_pane = self
            .maximized_pane
            .and_then(|pane_id| pane_ids.get(&pane_id).copied());
        self.minimized_panes = self
            .minimized_panes
            .iter()
            .filter_map(|pane_id| pane_ids.get(pane_id).copied())
            .collect();
        self.selected_minimized_pane = self
            .selected_minimized_pane
            .and_then(|pane_id| pane_ids.get(&pane_id).copied());
        self.renaming_pane = None;
        self.rename_buffer = None;
    }

    pub(crate) fn displayed_pane_label(&self, id: u64) -> Option<String> {
        let pane = self.pane(id)?;
        if self.renaming_pane != Some(id) {
            return Some(pane.label());
        }
        let buffer = self.rename_buffer.as_ref()?;
        if self.rename_select_all {
            return Some(buffer.clone());
        }
        let cursor = self.rename_cursor.min(buffer.len());
        let (before, after) = buffer.split_at(cursor);
        Some(format!("{before}|{after}"))
    }

    pub(crate) fn pane(&self, id: u64) -> Option<&TerminalPane> {
        self.pane_indices
            .get(&id)
            .and_then(|index| self.panes.get(*index))
    }

    pub(crate) fn pane_mut(&mut self, id: u64) -> Option<&mut TerminalPane> {
        let index = *self.pane_indices.get(&id)?;
        self.panes.get_mut(index)
    }

    pub(crate) fn push_pane(&mut self, mut pane: TerminalPane) {
        if pane.label_number == 0 {
            pane.label_number = self.next_pane_label;
        }
        self.next_pane_label = self.next_pane_label.max(pane.label_number + 1);
        self.pane_indices.insert(pane.id, self.panes.len());
        self.panes.push(pane);
    }

    pub(crate) fn remove_pane(&mut self, id: u64) -> Option<TerminalPane> {
        let index = self.pane_indices.remove(&id)?;
        let pane = self.panes.remove(index);
        for (index, pane) in self.panes.iter().enumerate().skip(index) {
            self.pane_indices.insert(pane.id, index);
        }
        Some(pane)
    }

    pub(crate) fn active_pane(&self) -> Option<&TerminalPane> {
        self.pane(self.active_pane)
    }

    pub(crate) fn active_profile(&self) -> Option<&Profile> {
        self.active_pane().map(|pane| &pane.profile)
    }

    pub(crate) fn activate_pane(&mut self, id: u64) {
        if self.pane(id).is_none() {
            return;
        }
        self.focus_history.retain(|pane_id| *pane_id != id);
        self.focus_history.push(id);
        self.active_pane = id;
    }

    pub(crate) fn visible_layout(&self) -> Option<PaneLayout> {
        if let Some(pane_id) = self.maximized_pane {
            return self.pane(pane_id).map(|_| PaneLayout::Pane(pane_id));
        }

        if self.minimized_panes.is_empty() {
            return Some(self.layout.clone());
        }
        let minimized = self.minimized_panes.iter().copied().collect::<HashSet<_>>();
        self.layout.without_all(&minimized)
    }

    pub(crate) fn pane_is_visible(&self, pane_id: u64) -> bool {
        self.pane(pane_id).is_some()
            && self
                .maximized_pane
                .map_or(!self.minimized_panes.contains(&pane_id), |maximized| {
                    maximized == pane_id
                })
    }

    pub(crate) fn toggle_maximize(&mut self, pane_id: u64) -> bool {
        if self.pane(pane_id).is_none() {
            return false;
        }
        let pane_was_minimized = self.minimized_panes.contains(&pane_id);
        let visible_pane_count = self
            .panes
            .len()
            .saturating_sub(self.minimized_panes.len())
            .saturating_add(usize::from(pane_was_minimized));
        if self.maximized_pane != Some(pane_id) && visible_pane_count < 2 {
            return false;
        }
        self.minimized_panes.retain(|id| *id != pane_id);
        self.repair_minimized_selection();
        self.maximized_pane = (self.maximized_pane != Some(pane_id)).then_some(pane_id);
        self.activate_pane(pane_id);
        true
    }

    pub(crate) fn minimize(&mut self, pane_id: u64) -> bool {
        if self.pane(pane_id).is_none()
            || self.minimized_panes.contains(&pane_id)
            || self.panes.len().saturating_sub(self.minimized_panes.len()) <= 1
        {
            return false;
        }

        self.maximized_pane = None;
        self.minimized_panes.push(pane_id);
        self.selected_minimized_pane = Some(pane_id);
        let fallback = self
            .focus_history
            .iter()
            .rev()
            .copied()
            .find(|id| *id != pane_id && !self.minimized_panes.contains(id))
            .or_else(|| {
                self.layout
                    .regions()
                    .into_iter()
                    .map(|region| region.id)
                    .find(|id| !self.minimized_panes.contains(id))
            })
            .expect("minimizing is only allowed when another pane remains visible");
        self.activate_pane(fallback);
        true
    }

    pub(crate) fn restore_minimized(&mut self, pane_id: u64) -> bool {
        let Some(index) = self.minimized_panes.iter().position(|id| *id == pane_id) else {
            return false;
        };
        self.minimized_panes.remove(index);
        if self.selected_minimized_pane == Some(pane_id) {
            self.selected_minimized_pane = if self.minimized_panes.is_empty() {
                None
            } else {
                Some(self.minimized_panes[index % self.minimized_panes.len()])
            };
        } else {
            self.repair_minimized_selection();
        }
        self.maximized_pane = None;
        self.activate_pane(pane_id);
        true
    }

    pub(crate) fn restore_last_minimized(&mut self) -> bool {
        self.selected_minimized_pane
            .filter(|pane_id| self.minimized_panes.contains(pane_id))
            .or_else(|| self.minimized_panes.last().copied())
            .is_some_and(|pane_id| self.restore_minimized(pane_id))
    }

    pub(crate) fn select_previous_minimized(&mut self) -> bool {
        self.select_adjacent_minimized(false)
    }

    pub(crate) fn select_next_minimized(&mut self) -> bool {
        self.select_adjacent_minimized(true)
    }

    fn select_adjacent_minimized(&mut self, forward: bool) -> bool {
        if self.minimized_panes.is_empty() {
            self.selected_minimized_pane = None;
            return false;
        }
        let index = self
            .selected_minimized_pane
            .and_then(|pane_id| self.minimized_panes.iter().position(|id| *id == pane_id))
            .map(|index| {
                if forward {
                    (index + 1) % self.minimized_panes.len()
                } else {
                    index
                        .checked_sub(1)
                        .unwrap_or(self.minimized_panes.len() - 1)
                }
            })
            .unwrap_or_else(|| {
                if forward {
                    0
                } else {
                    self.minimized_panes.len() - 1
                }
            });
        self.selected_minimized_pane = Some(self.minimized_panes[index]);
        true
    }

    fn repair_minimized_selection(&mut self) {
        if !self
            .selected_minimized_pane
            .is_some_and(|pane_id| self.minimized_panes.contains(&pane_id))
        {
            self.selected_minimized_pane = self.minimized_panes.last().copied();
        }
    }

    pub(crate) fn restore_focus_after_close(&mut self, closed: u64, fallback: u64) {
        if self.renaming_pane == Some(closed) {
            self.renaming_pane = None;
            self.rename_buffer = None;
            self.rename_select_all = false;
        }
        if self.maximized_pane == Some(closed) {
            self.maximized_pane = None;
        }
        self.minimized_panes.retain(|pane_id| *pane_id != closed);
        let surviving = self.panes.iter().map(|pane| pane.id).collect::<Vec<_>>();
        if !surviving.is_empty() && surviving.len() == self.minimized_panes.len() {
            let restored = self
                .minimized_panes
                .pop()
                .expect("a surviving minimized pane must be available");
            self.activate_pane(restored);
        }
        self.repair_minimized_selection();
        if self.panes.len() == 1 {
            self.maximized_pane = None;
        }
        self.focus_history
            .retain(|pane_id| *pane_id != closed && surviving.contains(pane_id));

        if self.active_pane != closed
            && surviving.contains(&self.active_pane)
            && !self.minimized_panes.contains(&self.active_pane)
        {
            return;
        }
        let next = self
            .focus_history
            .iter()
            .rev()
            .copied()
            .find(|pane_id| !self.minimized_panes.contains(pane_id))
            .or_else(|| self.visible_layout().map(|layout| layout.first_pane()))
            .or(self.selected_minimized_pane)
            .or_else(|| surviving.first().copied())
            .unwrap_or(fallback);
        self.activate_pane(next);
    }

    pub(crate) fn theme(&self, cx: &App) -> Arc<Theme> {
        self.active_pane()
            .and_then(|pane| pane.view.as_ref())
            .and_then(|view| view.read(cx).theme().cloned())
            .or_else(|| {
                self.active_profile()
                    .and_then(|profile| resolve_profile_theme(profile, cx).ok().flatten())
            })
            .unwrap_or_else(|| cx.theme().clone())
    }
}

#[cfg(test)]
#[path = "tests/pane.rs"]
mod tests;
