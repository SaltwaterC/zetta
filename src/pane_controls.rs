use super::*;

const PANE_CONTROLS_IDLE_DELAY: Duration = Duration::from_millis(1200);

pub(crate) fn pane_controls_hide_delay(last_motion: Instant, now: Instant) -> Option<Duration> {
    let elapsed = now.saturating_duration_since(last_motion);
    let remaining = PANE_CONTROLS_IDLE_DELAY.checked_sub(elapsed)?;
    (!remaining.is_zero()).then_some(remaining)
}

pub(crate) fn toggle_hidden_pane_controls(
    hidden_panes: &mut HashSet<u64>,
    pane_ids: &[u64],
) -> bool {
    let hide = pane_ids
        .iter()
        .any(|pane_id| !hidden_panes.contains(pane_id));
    if hide {
        hidden_panes.extend(pane_ids.iter().copied());
    } else {
        for pane_id in pane_ids {
            hidden_panes.remove(pane_id);
        }
    }
    hide
}

pub(crate) fn default_hidden_pane_controls(
    pane_controls_hidden_by_default: bool,
    pane_ids: impl IntoIterator<Item = u64>,
) -> HashSet<u64> {
    if pane_controls_hidden_by_default {
        pane_ids.into_iter().collect()
    } else {
        HashSet::default()
    }
}

pub(crate) fn reset_pane_controls_visibility(
    hidden_panes: &mut HashSet<u64>,
    pane_controls_hidden_by_default: bool,
    pane_ids: impl IntoIterator<Item = u64>,
) {
    for pane_id in pane_ids {
        if pane_controls_hidden_by_default {
            hidden_panes.insert(pane_id);
        } else {
            hidden_panes.remove(&pane_id);
        }
    }
}

impl Zetta {
    pub(crate) fn show_pane_controls(
        &mut self,
        pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pane_controls_hidden_for.contains(&pane_id) {
            return;
        }
        let visibility_changed = self.pane_controls_visible_for != Some(pane_id);
        self.pane_controls_visible_for = Some(pane_id);
        self.pane_controls_last_motion = Instant::now();

        if self.pane_controls_hide_task.is_none() {
            let executor = cx.background_executor().clone();
            self.pane_controls_hide_task = Some(cx.spawn_in(window, async move |this, cx| {
                let mut remaining = PANE_CONTROLS_IDLE_DELAY;
                loop {
                    executor.timer(remaining).await;
                    let next_delay = this
                        .update(cx, |this, cx| {
                            let next_delay = pane_controls_hide_delay(
                                this.pane_controls_last_motion,
                                Instant::now(),
                            );
                            if next_delay.is_none() {
                                this.pane_controls_visible_for = None;
                                this.pane_controls_hide_task.take();
                                cx.notify();
                            }
                            next_delay
                        })
                        .ok()
                        .flatten();
                    let Some(next_delay) = next_delay else {
                        break;
                    };
                    remaining = next_delay;
                }
            }));
        }

        if visibility_changed {
            cx.notify();
        }
    }

    pub(crate) fn forget_pane_controls(&mut self, pane_ids: impl IntoIterator<Item = u64>) {
        for pane_id in pane_ids {
            self.pane_controls_hidden_for.remove(&pane_id);
            if self.pane_controls_visible_for == Some(pane_id) {
                self.pane_controls_visible_for = None;
            }
        }
    }

    pub(crate) fn toggle_pane_controls_for(&mut self, pane_ids: &[u64], cx: &mut Context<Self>) {
        if pane_ids.is_empty() {
            return;
        }
        if toggle_hidden_pane_controls(&mut self.pane_controls_hidden_for, pane_ids)
            && self
                .pane_controls_visible_for
                .is_some_and(|pane_id| pane_ids.contains(&pane_id))
        {
            self.pane_controls_visible_for = None;
        }
        cx.notify();
    }

    pub(crate) fn toggle_pane_controls(
        &mut self,
        _: &TogglePaneControls,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane_id = self.tabs.get(self.active_tab).map(|tab| tab.active_pane);
        if let Some(pane_id) = pane_id {
            self.toggle_pane_controls_for(&[pane_id], cx);
        }
    }

    pub(crate) fn toggle_tab_pane_controls(
        &mut self,
        _: &ToggleTabPaneControls,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane_ids = self
            .tabs
            .get(self.active_tab)
            .map(|tab| tab.panes.iter().map(|pane| pane.id).collect::<Vec<_>>())
            .unwrap_or_default();
        self.toggle_pane_controls_for(&pane_ids, cx);
    }
}

#[cfg(test)]
#[path = "tests/pane_controls.rs"]
mod tests;
