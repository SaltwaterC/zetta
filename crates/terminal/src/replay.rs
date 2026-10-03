//! Builds restored grids privately after layout supplies geometry. Live readers
//! stay behind the replay barrier until a short swap publishes the result.
//! Resizes and injected output received during parsing are applied in order on
//! the worker before publication; neither a timer nor a second replay is needed.

use super::*;

#[derive(Default)]
pub(super) struct ReplayJob {
    updates: Option<Vec<Update>>,
    events: VecDeque<InternalEvent>,
    ready: Option<Receiver<()>>,
    #[cfg(test)]
    gate: Option<async_channel::Receiver<()>>,
}

/// Parser events can query colors and cursor state. Keep the existing bounded
/// event drain, but let it observe those events only after the private grid is
/// published, just as it did when parsing ran synchronously inside `sync`.
pub(super) async fn after_replay(
    terminal: &gpui::WeakEntity<Terminal>,
    cx: &mut gpui::AsyncApp,
    apply: impl FnOnce(&mut Terminal, &mut Context<Terminal>),
) -> Result<()> {
    let mut apply = Some(apply);
    let ready = terminal.update(cx, |terminal, cx| {
        if terminal.replay_job.is_running() {
            terminal.replay_job.ready.clone()
        } else {
            apply.take().unwrap()(terminal, cx);
            None
        }
    })?;
    if let Some(ready) = ready {
        ready.recv().await.ok();
        terminal.update(cx, apply.unwrap())?;
    }
    Ok(())
}

enum Update {
    Resize(TerminalBounds, bool),
    Configure(AlacrittyTermConfig),
    Output(Vec<u8>),
}

impl ReplayJob {
    pub(super) fn is_running(&self) -> bool {
        self.updates.is_some()
    }

    pub(super) fn resize(&mut self, bounds: TerminalBounds, reflow: bool) {
        if let Some(updates) = &mut self.updates {
            // As with the foreground resize queue, superseded geometry needs
            // no reflow. Never coalesce across output: those bytes were
            // produced for the preceding size.
            if let Some(Update::Resize(pending_bounds, pending_reflow)) = updates.last_mut() {
                *pending_bounds = bounds;
                *pending_reflow &= reflow;
            } else {
                updates.push(Update::Resize(bounds, reflow));
            }
        }
    }

    pub(super) fn configure(&mut self, config: AlacrittyTermConfig) {
        if let Some(updates) = &mut self.updates {
            updates.push(Update::Configure(config));
        }
    }

    pub(super) fn output(&mut self, bytes: &[u8]) -> bool {
        if let Some(updates) = &mut self.updates {
            updates.push(Update::Output(bytes.to_vec()));
            true
        } else {
            false
        }
    }

    pub(super) fn defer_event(&mut self, event: &InternalEvent, awaiting_replay: bool) -> bool {
        if (self.is_running() || awaiting_replay)
            && !matches!(
                event,
                InternalEvent::Resize { .. }
                    | InternalEvent::InitializeSize
                    | InternalEvent::ReplayFreshShell
            )
        {
            self.events.push_back(event.clone());
            true
        } else {
            false
        }
    }
}

struct PreparedReplay {
    term: AlacrittyTerm,
    processor: Processor<StdSyncHandler>,
}

impl PreparedReplay {
    fn apply(&mut self, updates: Vec<Update>) {
        for update in updates {
            match update {
                // Private reflow need not truncate large histories to satisfy
                // the foreground's synchronous cell budget.
                Update::Resize(bounds, reflow) => resize(&mut self.term, bounds, reflow),
                Update::Configure(config) => self.term.set_options(config),
                Update::Output(bytes) => self.processor.advance(&mut self.term, &bytes),
            }
        }
    }
}

impl Terminal {
    pub(super) fn start_pending_replay(&mut self, term: &AlacrittyTerm, cx: &mut Context<Self>) {
        if !self.terminal_size_initialized
            || (self.fresh_shell_restore && !self.restore_startup_ready)
        {
            return;
        }
        let Some(bytes) = self.pending_replay.take() else {
            return;
        };
        // A shared viewport may close the barrier without carrying any replay.
        // Geometry alone is enough in that case; do not delay its first output
        // by scheduling an empty parse.
        if bytes.is_empty() && !self.fresh_shell_restore {
            self.replay_barrier.release();
            return;
        }
        // This is the pre-replay grid: retained history has not been parsed
        // into it. Sealed history, if startup produced any, is shared by Clone.
        let mut prepared = PreparedReplay {
            term: term.clone(),
            processor: mem::replace(&mut self.output_processor, Processor::new()),
        };
        self.replay_job.updates = Some(Vec::new());
        let (finished, ready) = async_channel::bounded(1);
        self.replay_job.ready = Some(ready);
        let fresh_shell = self.fresh_shell_restore;
        #[cfg(test)]
        let gate = self.replay_job.gate.take();
        let parse = cx.background_executor().spawn(async move {
            #[cfg(test)]
            if let Some(gate) = gate {
                gate.recv().await.ok();
            }
            if fresh_shell {
                prepared.term.reset_for_fresh_shell_replay();
            }
            prepared.processor.advance(&mut prepared.term, &bytes);
            drop(bytes);
            if fresh_shell {
                prepared.term.normalize_for_fresh_shell();
            }
            prepared
        });
        cx.spawn(async move |terminal, cx| {
            let mut prepared = parse.await;
            loop {
                let mut completed = Some(prepared);
                let next = terminal.update(cx, |terminal, cx| {
                    let prepared = completed.take().unwrap();
                    let updates = terminal.replay_job.updates.as_mut().unwrap();
                    if !updates.is_empty() {
                        return Some((prepared, mem::take(updates)));
                    }
                    // No foreground event can interleave the generation check
                    // and this swap. Destroy the replaced grid off-thread too.
                    let old = mem::replace(&mut *terminal.term.lock(), prepared.term);
                    terminal.output_processor = prepared.processor;
                    terminal.replay_job.updates = None;
                    terminal.replay_job.ready = None;
                    terminal.replay_job.events.append(&mut terminal.events);
                    mem::swap(&mut terminal.events, &mut terminal.replay_job.events);
                    terminal.content_dirty = true;
                    if fresh_shell {
                        terminal.start_fresh_shell_input();
                    }
                    terminal.replay_barrier.release();
                    cx.emit(Event::Wakeup);
                    cx.notify();
                    cx.background_executor()
                        .spawn(async move { drop(old) })
                        .detach();
                    None
                });
                if let Some(abandoned) = completed {
                    cx.background_executor()
                        .spawn(async move { drop(abandoned) })
                        .detach();
                }
                let Ok(Some((mut grid, updates))) = next else {
                    break;
                };
                prepared = cx
                    .background_executor()
                    .spawn(async move {
                        grid.apply(updates);
                        grid
                    })
                    .await;
            }
            finished.close();
        })
        .detach();
    }
}

#[cfg(test)]
#[path = "tests/replay.rs"]
mod tests;
