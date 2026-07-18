use super::*;

pub(crate) const MAX_PANES_PER_TAB: usize = 64;
pub(crate) const MAX_CONCURRENT_MULTI_COMMAND_SPAWNS: usize = 4;
pub(crate) const TERMINAL_SPAWN_NOTIFY_INTERVAL: Duration = Duration::from_millis(16);
pub(crate) const PANE_OUTPUT_DEFAULT_FILENAME: &str = "terminal-output.txt";

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
    pub(crate) profile: Profile,
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
    pub(crate) fn wsl_working_directory(&self, cx: &App) -> Option<String> {
        if let Some(directory) = self.view.as_ref().and_then(|view| {
            view.read(cx)
                .terminal()
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy)]
pub(crate) enum PaneDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PaneRegion {
    pub(crate) id: u64,
    pub(crate) left: f32,
    pub(crate) right: f32,
    pub(crate) top: f32,
    pub(crate) bottom: f32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PaneLayout {
    Pane(u64),
    Split {
        axis: SplitAxis,
        first: Box<PaneLayout>,
        second: Box<PaneLayout>,
    },
}

impl PaneLayout {
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
                first: Box::new(build(&pane_ids[..midpoint], next_axis)),
                second: Box::new(build(&pane_ids[midpoint..], next_axis)),
            }
        }

        (!pane_ids.is_empty()).then(|| build(pane_ids, SplitAxis::Vertical))
    }

    pub(crate) fn split(&mut self, target: u64, axis: SplitAxis, new_pane: u64) -> bool {
        match self {
            Self::Pane(id) if *id == target => {
                *self = Self::Split {
                    axis,
                    first: Box::new(Self::Pane(target)),
                    second: Box::new(Self::Pane(new_pane)),
                };
                true
            }
            Self::Pane(_) => false,
            Self::Split { first, second, .. } => {
                first.split(target, axis, new_pane) || second.split(target, axis, new_pane)
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
                first,
                second,
            } => match (first.without(target), second.without(target)) {
                (Some(first), Some(second)) => Some(Self::Split {
                    axis,
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
                first,
                second,
            } => {
                first.collect_regions(left, top, width, height / 2., regions);
                second.collect_regions(left, top + height / 2., width, height / 2., regions);
            }
            Self::Split {
                axis: SplitAxis::Vertical,
                first,
                second,
            } => {
                first.collect_regions(left, top, width / 2., height, regions);
                second.collect_regions(left + width / 2., top, width / 2., height, regions);
            }
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

pub(crate) struct Tab {
    pub(crate) id: u64,
    pub(crate) panes: Vec<TerminalPane>,
    pub(crate) pane_indices: HashMap<u64, usize>,
    pub(crate) layout: PaneLayout,
    pub(crate) active_pane: u64,
    pub(crate) focus_history: Vec<u64>,
    pub(crate) broadcast_input: bool,
    pub(crate) custom_title: Option<String>,
    pub(crate) rename_buffer: Option<String>,
    pub(crate) rename_cursor: usize,
    pub(crate) rename_select_all: bool,
}

impl Tab {
    pub(crate) fn pane(&self, id: u64) -> Option<&TerminalPane> {
        self.pane_indices
            .get(&id)
            .and_then(|index| self.panes.get(*index))
    }

    pub(crate) fn pane_mut(&mut self, id: u64) -> Option<&mut TerminalPane> {
        let index = *self.pane_indices.get(&id)?;
        self.panes.get_mut(index)
    }

    pub(crate) fn push_pane(&mut self, pane: TerminalPane) {
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

    pub(crate) fn restore_focus_after_close(&mut self, closed: u64, fallback: u64) {
        let surviving = self.panes.iter().map(|pane| pane.id).collect::<Vec<_>>();
        self.focus_history
            .retain(|pane_id| *pane_id != closed && surviving.contains(pane_id));

        if self.active_pane != closed && surviving.contains(&self.active_pane) {
            return;
        }
        let next = self.focus_history.last().copied().unwrap_or(fallback);
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
