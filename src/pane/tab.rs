//! A tab: the panes it holds, which one is active, and what it shows.
//!
//! Maximize, minimize and the focus that follows a close are the tab's, not
//! the layout's: they change which panes are visible without changing the
//! layout tree, so the tree stays the thing a restore returns to.

use super::*;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TabAttention {
    pub(crate) summary: String,
    pub(crate) body: Option<String>,
}

impl TabAttention {
    pub(crate) fn tooltip_text(&self) -> String {
        match &self.body {
            Some(body) if !body.is_empty() => format!("{}\n{}", self.summary, body),
            _ => self.summary.clone(),
        }
    }
}

/// The user-selected tab icon, kept separate from `Tab::icon`, which is the
/// currently effective icon after project/default resolution.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TabIconOverride {
    /// No user choice: the active project or the configured default may supply
    /// the effective icon.
    #[default]
    None,
    /// A user-selected icon that takes precedence over project/default values.
    Icon(IconName),
    /// A user-selected absence of an icon, which must not be confused with no
    /// override at all.
    Hidden,
}

impl TabIconOverride {
    pub(crate) const fn from_icon(icon: Option<IconName>) -> Self {
        match icon {
            Some(icon) => Self::Icon(icon),
            None => Self::Hidden,
        }
    }
}

pub(crate) struct Tab {
    pub(crate) id: u64,
    /// Stable within this Zetta process. Unlike `id`, this survives moving a
    /// tab between visible and background storage and any pane/tab ID remap.
    pub(crate) attention_id: u64,
    pub(crate) attention: Option<TabAttention>,
    pub(crate) panes: Vec<TerminalPane>,
    pub(crate) pane_indices: HashMap<u64, usize>,
    pub(crate) next_pane_label: usize,
    /// A session-scoped theme shared by this tab's terminal content and tab
    /// chrome. Pane overrides take precedence over this value for terminal
    /// content, but never change it.
    pub(crate) theme_override: Option<String>,
    pub(crate) layout: PaneLayout,
    pub(crate) active_pane: u64,
    pub(crate) focus_history: Vec<u64>,
    pub(crate) maximized_pane: Option<u64>,
    pub(crate) minimized_panes: Vec<u64>,
    pub(crate) selected_minimized_pane: Option<u64>,
    pub(crate) broadcast_input: bool,
    pub(crate) silent_mode: bool,
    pub(crate) close_policy: TabClosePolicy,
    /// Whether this tab's session is offered to other Zetta windows, so one of
    /// them can join it and both then drive the same panes.
    ///
    /// Separate from `close_policy`: sharing says who may see the session now,
    /// and a shared tab is handed back to the multiplexer when its last viewer
    /// closes. `keep_running` remains the explicit lifecycle policy for an
    /// unshared tab (and the process-local fallback in `--no-mux` mode).
    pub(crate) shared: bool,
    /// A title entered through the tab rename UI. This is the highest-priority
    /// title source and is intentionally separate from process/worktree state.
    pub(crate) custom_title: Option<String>,
    /// A worktree title supplied by `wt new`. CWD detection may provide a
    /// fresher title, but terminal-side title updates can never replace this
    /// seed; `wt done` and `wt abort` explicitly clear it.
    pub(crate) worktree_seed_title: Option<String>,
    /// A lower-priority title supplied by process control. It remains
    /// available outside a worktree, but is masked by the active worktree
    /// title.
    pub(crate) process_title: Option<String>,
    pub(crate) icon: Option<IconName>,
    /// Explicit per-tab icon choice. `icon` remains the effective value so
    /// rendering does not need to know whether it came from this override or
    /// from project/default resolution.
    pub(crate) icon_override: TabIconOverride,
    /// Session-only visual pinning. Pinned tabs stay in the leading tab-bar
    /// prefix and are independent from the keep-running close policy.
    pub(crate) pinned: bool,
    pub(crate) renaming_pane: Option<u64>,
    pub(crate) rename_buffer: Option<TextField>,
    pub(crate) editing_overlay_pane: Option<u64>,
    pub(crate) overlay_buffer: Option<TextField>,
    /// Live overlay-style selector, opened right after the overlay's text is
    /// entered from the command palette. Combines font size, colour, and
    /// opacity.
    pub(crate) overlay_style_picker: Option<OverlayStylePicker>,
}

impl Tab {
    /// Applies a user choice to both the effective icon and its precedence
    /// marker. Project resolution must never be able to replace this choice.
    pub(crate) fn set_icon_override(&mut self, icon: Option<IconName>) {
        self.icon = icon;
        self.icon_override = TabIconOverride::from_icon(icon);
    }

    /// Renumbers a tab that is entering this window, and reports how.
    ///
    /// Every window-scoped registry is keyed by pane id alone — the multiplexer
    /// pane map, the shared-pane registry, pane controls, the project registry —
    /// so a tab arriving with another window's pane ids shares those entries with
    /// whatever tab already holds them. The returned map is old id to new id, for
    /// callers holding anything else addressed by the old ones.
    pub(crate) fn reassign_ids(
        &mut self,
        tab_id: u64,
        next_pane_id: &mut u64,
    ) -> HashMap<u64, u64> {
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
        if !pane_ids.is_empty() {
            for pane in &mut self.panes {
                for entry in &mut pane.stack.entries {
                    let old_id = entry.id;
                    entry.id = *next_pane_id;
                    *next_pane_id += 1;
                    if pane.stack.selected == PaneStackSelection::Stacked(old_id) {
                        pane.stack.selected = PaneStackSelection::Stacked(entry.id);
                    }
                }
                pane.stack.repair_selection();
            }
        }
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
        self.editing_overlay_pane = None;
        self.overlay_buffer = None;
        self.overlay_style_picker = None;
        pane_ids
    }

    pub(crate) fn displayed_pane_label(&self, id: u64) -> Option<String> {
        let pane = self.pane(id)?;
        if self.renaming_pane != Some(id) {
            return Some(pane.label());
        }
        Some(self.rename_buffer.as_ref()?.caret_marker_display())
    }

    /// Whether `id` is the pane being renamed with its whole label selected.
    /// The pane's label carries the selection highlight, since the label is
    /// rendered as text rather than as a field.
    pub(crate) fn pane_rename_selected(&self, id: u64) -> bool {
        self.renaming_pane == Some(id) && self.rename_selected()
    }

    /// The same for the tab title, which is renamed through the same buffer:
    /// `renaming_pane` is what distinguishes the two.
    pub(crate) fn tab_rename_selected(&self) -> bool {
        self.renaming_pane.is_none() && self.rename_selected()
    }

    fn rename_selected(&self) -> bool {
        self.rename_buffer
            .as_ref()
            .is_some_and(|field| field.select_all)
    }

    /// The pane's overlay text: the committed `overlay_text` normally, or the
    /// in-progress edit buffer (with a `|` cursor marker) while it is being
    /// edited. `None` means no overlay should be shown for this pane.
    pub(crate) fn displayed_pane_overlay(&self, id: u64) -> Option<String> {
        let pane = self.pane(id)?;
        if self.editing_overlay_pane != Some(id) {
            return pane.overlay_text.clone();
        }
        Some(self.overlay_buffer.as_ref()?.caret_marker_display())
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

    pub(crate) fn apply_generated_labels(
        &mut self,
        pane_labels: impl IntoIterator<Item = (u64, Option<String>)>,
    ) {
        for (pane_id, generated_label) in pane_labels {
            if let Some(pane) = self.pane_mut(pane_id) {
                pane.generated_label = generated_label;
            }
        }
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

    pub(crate) fn activate_stack_entry(&mut self, pane_id: u64, entry: PaneStackSelection) {
        let Some(pane) = self.pane_mut(pane_id) else {
            return;
        };
        if !pane.stack.select(entry) {
            return;
        }
        self.activate_pane(pane_id);
    }

    pub(crate) fn active_view(&self) -> Option<Entity<TerminalView>> {
        self.active_pane().and_then(TerminalPane::selected_view)
    }

    pub(crate) fn active_terminal(&self) -> Option<Entity<Terminal>> {
        self.active_pane().and_then(TerminalPane::selected_terminal)
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

    /// The configured fallback for chrome shared across a tab's panes (tab bar,
    /// pane borders, error banners). `Zetta::theme_for_tab` checks the tab's
    /// session override and project theme before calling this; a pane's
    /// non-persistent override never repaints the rest of the tab.
    ///
    /// `fallback` supplies the theme for a tab whose active profile selects
    /// none, and is lazy so the common case never resolves it. It is the
    /// caller's application theme rather than `cx.theme()`, which
    /// `Zetta::render` repoints per window (see `Zetta::application_theme`).
    pub(crate) fn theme(&self, cx: &App, fallback: impl FnOnce() -> Arc<Theme>) -> Arc<Theme> {
        self.active_profile()
            .and_then(|profile| resolve_profile_theme(profile, cx).ok().flatten())
            .unwrap_or_else(fallback)
    }
}

#[cfg(test)]
#[path = "../tests/pane/tab.rs"]
mod tests;
