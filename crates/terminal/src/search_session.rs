//! The search lifecycle shared by pane and tab search. A session searches one
//! or several ordered terminals; callers own query editing and revealing the
//! pane identified by an activation event. Only an invalidated terminal restarts.
//! Snapshots, reflow projection and scanning remain in `Terminal::find_matches`.

use crate::{Event, Search, SearchUpdate, Terminal};
use gpui::{Context, Entity, EventEmitter, Subscription, Task};
use std::{sync::Arc, time::Duration};

const DEBOUNCE: Duration = Duration::from_millis(75);

/// A terminal's position in the session and a match's oldest-first index in it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SearchMatch {
    pub target: usize,
    pub index: usize,
}

/// The pane containing this match should be revealed, without moving input focus.
#[derive(Clone, Copy, Debug)]
pub struct SearchActivated(pub SearchMatch);

/// Counts and navigation position for display. Counts remain provisional until
/// every target completes; `shown` is bounded independently of the exact count.
#[derive(Clone, Copy, Debug)]
pub struct SearchSessionStatus {
    pub active_match: Option<usize>,
    pub shown: usize,
    pub total_count: usize,
    pub limit_reached: bool,
    pub complete: bool,
}

impl Default for SearchSessionStatus {
    fn default() -> Self {
        Self {
            active_match: None,
            shown: 0,
            total_count: 0,
            limit_reached: false,
            complete: true,
        }
    }
}

struct Target {
    terminal: Entity<Terminal>,
    generation: u64,
    shown: usize,
    total_count: usize,
    limit_reached: bool,
    complete: bool,
    task: Option<Task<()>>,
    _subscription: Subscription,
}

/// Owns subscriptions, debounced jobs, highlights, counts, and navigation.
/// Call `clear` before discarding a session to remove its terminal highlights;
/// dropping it cancels all jobs and subscriptions.
pub struct SearchSession {
    query: String,
    pattern: Option<Search>,
    targets: Vec<Target>,
    active: Option<SearchMatch>,
    generation: u64,
}

impl EventEmitter<SearchActivated> for SearchSession {}

impl SearchSession {
    pub fn new(terminals: Vec<Entity<Terminal>>, cx: &mut Context<Self>) -> Self {
        let mut session = Self {
            query: String::new(),
            pattern: None,
            targets: Vec::new(),
            active: None,
            generation: 0,
        };
        session.set_targets(terminals, cx);
        session
    }

    /// Preserve completed work when the ordered terminal set has not changed.
    pub fn set_targets(&mut self, terminals: Vec<Entity<Terminal>>, cx: &mut Context<Self>) {
        if self
            .targets
            .iter()
            .map(|target| &target.terminal)
            .eq(terminals.iter())
        {
            return;
        }
        self.clear_targets(cx);
        self.targets = terminals
            .into_iter()
            .enumerate()
            .map(|(index, terminal)| {
                let subscription = cx.subscribe(&terminal, move |session, terminal, event, cx| {
                    if matches!(event, Event::Wakeup | Event::GridSizeChanged)
                        && session
                            .targets
                            .get(index)
                            .is_some_and(|target| target.terminal == terminal)
                    {
                        session.restart_target(index, cx);
                    }
                });
                Target {
                    terminal,
                    generation: 0,
                    shown: 0,
                    total_count: 0,
                    limit_reached: false,
                    complete: true,
                    task: None,
                    _subscription: subscription,
                }
            })
            .collect();
        self.restart_all(cx);
    }

    pub fn set_query(&mut self, query: &str, cx: &mut Context<Self>) {
        if self.query == query {
            return;
        }
        self.query = query.to_owned();
        self.pattern = (!query.is_empty())
            .then(|| Search::new_literal(query))
            .flatten();
        self.restart_all(cx);
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.query.clear();
        self.pattern = None;
        self.restart_all(cx);
    }

    pub fn status(&self) -> SearchSessionStatus {
        let mut status = SearchSessionStatus::default();
        for (index, target) in self.targets.iter().enumerate() {
            if let Some(active) = self.active.filter(|active| active.target == index) {
                status.active_match = Some(status.shown + active.index);
            }
            status.shown += target.shown;
            status.total_count = status.total_count.saturating_add(target.total_count);
            status.limit_reached |= target.limit_reached;
            status.complete &= target.complete;
        }
        status
    }

    pub fn match_at(&self, mut index: usize) -> Option<SearchMatch> {
        for (target, state) in self.targets.iter().enumerate() {
            if index < state.shown {
                return Some(SearchMatch { target, index });
            }
            index -= state.shown;
        }
        None
    }

    pub fn navigate(&mut self, previous: bool, cx: &mut Context<Self>) {
        let status = self.status();
        if status.shown == 0 {
            return;
        }
        let current = status
            .active_match
            .unwrap_or(if previous { 0 } else { status.shown - 1 });
        let index = if previous {
            current.checked_sub(1).unwrap_or(status.shown - 1)
        } else {
            (current + 1) % status.shown
        };
        if let Some(found) = self.match_at(index) {
            self.activate(found, cx);
        }
    }

    fn activate(&mut self, found: SearchMatch, cx: &mut Context<Self>) {
        self.active = Some(found);
        self.targets[found.target]
            .terminal
            .update(cx, |terminal, cx| {
                terminal.activate_match(found.index);
                cx.notify();
            });
        cx.emit(SearchActivated(found));
        cx.notify();
    }

    fn clear_target(target: &mut Target, cx: &mut Context<Self>) {
        target.task.take();
        target.shown = 0;
        target.total_count = 0;
        target.limit_reached = false;
        target.terminal.update(cx, |terminal, cx| {
            if !terminal.matches.is_empty() {
                // The renderer can still hold the old ranges. Replacing their
                // Arc avoids cloning all of them just to discard the clone.
                terminal.matches = Arc::default();
                terminal.clear_reflow_search_selection();
                cx.notify();
            }
        });
    }

    fn clear_targets(&mut self, cx: &mut Context<Self>) {
        self.active = None;
        for target in &mut self.targets {
            Self::clear_target(target, cx);
        }
    }

    fn restart_all(&mut self, cx: &mut Context<Self>) {
        self.active = None;
        for index in 0..self.targets.len() {
            self.restart_target(index, cx);
        }
        // Query editing must repaint even before the debounce or a reflow completes.
        cx.notify();
    }

    fn restart_target(&mut self, index: usize, cx: &mut Context<Self>) {
        let target = &mut self.targets[index];
        if self.query.is_empty() && target.complete && target.task.is_none() {
            return;
        }
        Self::clear_target(target, cx);
        if self.active.is_some_and(|active| active.target == index) {
            self.active = None;
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        target.generation = generation;
        target.complete = self.query.is_empty();
        cx.notify();
        let Some(pattern) = self.pattern.clone() else {
            return;
        };
        let terminal = target.terminal.clone();
        let executor = cx.background_executor().clone();
        target.task = Some(cx.spawn(async move |this, cx| {
            executor.timer(DEBOUNCE).await;
            let Some(mut job) = this
                .update(cx, |session, cx| {
                    session.is_current(index, generation).then(|| {
                        terminal.update(cx, |terminal, cx| terminal.find_matches(pattern, cx))
                    })
                })
                .ok()
                .flatten()
            else {
                return;
            };
            while let Some(update) = job.next().await {
                let current = this
                    .update(cx, |session, cx| {
                        if !session.is_current(index, generation) {
                            return false;
                        }
                        session.apply_update(index, update, cx);
                        true
                    })
                    .unwrap_or(false);
                if !current {
                    return;
                }
            }
        }));
    }

    fn is_current(&self, index: usize, generation: u64) -> bool {
        self.targets
            .get(index)
            .is_some_and(|target| target.generation == generation)
    }

    fn apply_update(&mut self, index: usize, update: SearchUpdate, cx: &mut Context<Self>) {
        let target = &mut self.targets[index];
        let added = update.older_matches.len();
        target.shown += added;
        target.total_count = update.total_count;
        target.limit_reached = update.limit_reached;
        target.complete = update.complete;
        if added > 0 {
            target.terminal.update(cx, |terminal, cx| {
                Arc::make_mut(&mut terminal.matches).extend_older(update.older_matches);
                cx.notify();
            });
        }
        if let Some(active) = &mut self.active {
            if active.target == index {
                active.index += added;
            }
        } else if added > 0 {
            let found = SearchMatch {
                target: index,
                index: target.shown - 1,
            };
            self.activate(found, cx);
        }
        cx.notify();
    }
}

#[cfg(test)]
#[path = "tests/search_session.rs"]
mod tests;
