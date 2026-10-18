//! Reflows retained history on a private terminal while a bounded live grid
//! continues drawing and parsing output. Semantic ANSI operations are mirrored
//! in order, including operations split across parser batches. Publication only
//! swaps a caught-up result; it never holds the live grid lock during reflow.

use super::*;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};

/// The live parser answers viewport queries immediately, including cursor and
/// size requests. Replaying state changes must never duplicate their events.
#[derive(Clone)]
pub struct ReflowListener;

/// A bounded search viewport and the source position of each displayed cell.
pub struct ReflowSearchView {
    pub term: Term<ReflowListener>,
    pub offset: usize,
    pub total_lines: usize,
    pub cells: Vec<(Point, Point)>,
}

impl EventListener for ReflowListener {
    fn send_event(&self, _event: Event) {}
}

type Operation = Box<dyn FnOnce(&mut Term<ReflowListener>) + Send>;

enum Update {
    Operation(Operation),
    Ascii(Vec<u8>),
    Characters(Vec<char>),
    Resize(usize, usize),
}

#[derive(Default)]
struct State {
    updates: VecDeque<Update>,
    prepared: Option<Term<ReflowListener>>,
    stopped: bool,
    source: Option<Arc<Term<ReflowListener>>>,
    view_offset: usize,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    recording: AtomicBool,
}

fn lock(shared: &Shared) -> MutexGuard<'_, State> {
    shared.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A blocking read of the complete grid, for background exports and searches.
/// Never wait on this on the UI thread or while holding the live grid lock.
#[derive(Clone)]
pub struct ReflowReader(Arc<Shared>);

impl ReflowReader {
    pub fn snapshot(&self) -> Option<Term<ReflowListener>> {
        self.read(Clone::clone)
    }
    pub fn read<R>(&self, read: impl FnOnce(&Term<ReflowListener>) -> R) -> Option<R> {
        let mut state = lock(&self.0);
        loop {
            if state.stopped {
                return None;
            }
            if state.updates.is_empty()
                && let Some(term) = &state.prepared
            {
                return Some(read(term));
            }
            state = self.0.changed.wait(state).unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }
}

#[derive(Default)]
pub(super) struct PendingReflow(Option<Arc<Shared>>);

// Snapshots own their grids; they must never record into a live terminal's job.
impl Clone for PendingReflow {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl Drop for PendingReflow {
    fn drop(&mut self) {
        if let Some(shared) = &self.0 {
            lock(shared).stopped = true;
            shared.changed.notify_all();
        }
    }
}

pub(super) struct Recording(Arc<Shared>);

impl Drop for Recording {
    fn drop(&mut self) {
        self.0.recording.store(false, Ordering::Relaxed);
    }
}

impl PendingReflow {
    fn enter(&self) -> Option<Recording> {
        let shared = self.0.as_ref()?;
        if shared.recording.swap(true, Ordering::Relaxed) {
            return None;
        }
        Some(Recording(Arc::clone(shared)))
    }

    #[inline]
    pub(super) fn record<F: FnOnce(&mut Term<ReflowListener>) + Send + 'static>(
        &self,
        operation: impl FnOnce() -> F,
    ) -> Option<Recording> {
        self.0.as_ref()?;
        self.record_active(operation)
    }

    #[cold]
    #[inline(never)]
    fn record_active<F: FnOnce(&mut Term<ReflowListener>) + Send + 'static>(
        &self,
        operation: impl FnOnce() -> F,
    ) -> Option<Recording> {
        let scope = self.enter()?;
        lock(&scope.0).updates.push_back(Update::Operation(Box::new(operation())));
        scope.0.changed.notify_one();
        Some(scope)
    }

    #[inline]
    pub(super) fn record_ascii(&self, run: &[u8]) -> Option<Recording> {
        self.0.as_ref()?;
        self.record_ascii_active(run)
    }

    #[cold]
    #[inline(never)]
    fn record_ascii_active(&self, run: &[u8]) -> Option<Recording> {
        let scope = self.enter()?;
        let mut state = lock(&scope.0);
        match state.updates.back_mut() {
            Some(Update::Ascii(bytes)) if bytes.len() + run.len() <= 64 * 1024 => {
                bytes.extend_from_slice(run);
            },
            _ => state.updates.push_back(Update::Ascii(run.to_vec())),
        }
        drop(state);
        scope.0.changed.notify_one();
        Some(scope)
    }

    #[inline]
    pub(super) fn record_resize(&self, columns: usize, lines: usize) -> Option<Recording> {
        self.0.as_ref()?;
        self.record_resize_active(columns, lines)
    }

    #[cold]
    #[inline(never)]
    fn record_resize_active(&self, columns: usize, lines: usize) -> Option<Recording> {
        let scope = self.enter()?;
        let mut state = lock(&scope.0);
        match state.updates.back_mut() {
            Some(Update::Resize(pending_columns, pending_lines)) => {
                *pending_columns = columns;
                *pending_lines = lines;
            },
            _ => state.updates.push_back(Update::Resize(columns, lines)),
        }
        drop(state);
        scope.0.changed.notify_one();
        Some(scope)
    }

    #[inline]
    pub(super) fn record_char(&self, character: char) -> Option<Recording> {
        self.0.as_ref()?;
        self.record_char_active(character)
    }

    #[cold]
    #[inline(never)]
    fn record_char_active(&self, character: char) -> Option<Recording> {
        let scope = self.enter()?;
        let mut state = lock(&scope.0);
        match state.updates.back_mut() {
            Some(Update::Characters(characters)) if characters.len() < 16 * 1024 => {
                characters.push(character);
            },
            _ => state.updates.push_back(Update::Characters(vec![character])),
        }
        drop(state);
        scope.0.changed.notify_one();
        Some(scope)
    }
}

fn run(mut term: Term<ReflowListener>, shared: Arc<Shared>, wake: impl Fn()) {
    loop {
        let mut state = lock(&shared);
        if state.stopped {
            // Both the source and any unpublished result are destroyed here,
            // including when the window closes while its history is reflowing.
            state.source.take();
            return;
        }
        if state.updates.is_empty() {
            state.prepared = Some(term);
            shared.changed.notify_all();
            drop(state);
            wake();
            state = lock(&shared);
            while state.updates.is_empty() && !state.stopped {
                state = shared.changed.wait(state).unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            let Some(prepared) = state.prepared.take() else {
                state.source.take();
                return;
            };
            term = prepared;
            if state.stopped {
                state.source.take();
                return;
            }
        }
        let updates = mem::take(&mut state.updates);
        drop(state);
        for update in updates {
            match update {
                Update::Operation(operation) => operation(&mut term),
                Update::Ascii(bytes) => term.input_ascii(&bytes),
                Update::Characters(characters) => {
                    for character in characters {
                        term.input(character);
                    }
                },
                Update::Resize(columns, lines) => term.resize(test::TermSize::new(columns, lines)),
            }
        }
    }
}

impl<T> Term<T> {
    /// Scroll an immutable source while full-history reflow runs. This is a
    /// viewport operation: it neither waits for the worker nor decodes history
    /// outside the requested screen-sized window.
    pub(super) fn scroll_reflow_view(&self, scroll: Scroll) -> bool {
        let Some(shared) = &self.reflow.0 else { return false };
        let mut state = lock(shared);
        let Some(source) = state.source.as_ref() else { return false };
        let history = source.history_size();
        state.view_offset = match scroll {
            Scroll::Delta(delta) => {
                state.view_offset.saturating_add_signed(delta as isize).min(history)
            },
            Scroll::PageUp => (state.view_offset + self.screen_lines()).min(history),
            Scroll::PageDown => state.view_offset.saturating_sub(self.screen_lines()),
            Scroll::Top => history,
            Scroll::Bottom => 0,
        };
        state
            .updates
            .push_back(Update::Operation(Box::new(move |term| term.scroll_display(scroll))));
        shared.changed.notify_one();
        true
    }

    /// A bounded, already reflowed browsing viewport and its source-row scroll
    /// coordinates. The full history is never cloned or expanded here.
    pub fn pending_reflow_view(&self) -> Option<(Term<ReflowListener>, usize, usize)> {
        let state = lock(self.reflow.0.as_ref()?);
        if state.view_offset == 0 {
            return None;
        }
        let source = state.source.as_ref()?;
        let mut view = Term::new(
            source.config.clone(),
            &test::TermSize::new(self.columns(), self.screen_lines()),
            ReflowListener,
        );
        view.grid =
            source.grid.reflow_window(state.view_offset, self.screen_lines(), self.columns());
        view.colors = self.colors;
        view.mode = self.mode;
        view.mode.remove(TermMode::SHOW_CURSOR);
        Some((view, state.view_offset, source.history_size() + self.screen_lines()))
    }

    pub fn pending_reflow_total_lines(&self) -> Option<usize> {
        let state = lock(self.reflow.0.as_ref()?);
        Some(state.source.as_ref()?.history_size() + self.screen_lines())
    }

    /// History in both screens. An alternate-screen application's visible
    /// history is empty, but resizing must still reflow its hidden primary.
    pub fn retained_history_size(&self) -> usize {
        self.grid.history_size().saturating_add(self.inactive_grid.history_size())
    }

    /// Copy the current state without a live event recipient. Callers needing
    /// complete history must first resolve `reflow_reader` off the UI thread.
    pub fn snapshot_without_events(&self) -> Term<ReflowListener> {
        self.reflow_snapshot(ReflowListener)
    }

    fn reflow_snapshot(&self, event_proxy: ReflowListener) -> Term<ReflowListener> {
        self.snapshot_with_grids(event_proxy, self.grid.clone(), self.inactive_grid.clone())
    }

    fn snapshot_with_grids(
        &self,
        event_proxy: ReflowListener,
        grid: Grid<Cell>,
        inactive_grid: Grid<Cell>,
    ) -> Term<ReflowListener> {
        Term {
            is_focused: self.is_focused,
            vi_mode_cursor: self.vi_mode_cursor,
            selection: self.selection.clone(),
            grid,
            inactive_grid,
            active_charset: self.active_charset,
            tabs: self.tabs.clone(),
            mode: self.mode,
            hosted_output_mode: self.hosted_output_mode,
            scroll_region: self.scroll_region.clone(),
            colors: self.colors,
            cursor_style: self.cursor_style,
            event_proxy,
            title: self.title.clone(),
            title_stack: self.title_stack.clone(),
            keyboard_mode_stack: self.keyboard_mode_stack.clone(),
            inactive_keyboard_mode_stack: self.inactive_keyboard_mode_stack.clone(),
            damage: self.damage.clone(),
            config: self.config.clone(),
            reflow: PendingReflow::default(),
        }
    }

    pub fn reflow_reader(&self) -> Option<ReflowReader> {
        self.reflow.0.as_ref().map(|shared| ReflowReader(Arc::clone(shared)))
    }

    /// The last complete, immutable history. Searching it must not wait for
    /// the private resize worker; source positions remain valid for browsing.
    pub fn reflow_search_source(&self) -> Option<Arc<Term<ReflowListener>>> {
        lock(self.reflow.0.as_ref()?).source.clone()
    }

    pub fn reflow_search_offset(&self) -> Option<usize> {
        Some(lock(self.reflow.0.as_ref()?).view_offset)
    }

    pub fn reflow_search_view(&self, focus: Option<Point>) -> Option<ReflowSearchView> {
        let shared = self.reflow.0.as_ref()?;
        let state = lock(shared);
        let source = state.source.clone()?;
        let offset = state.view_offset;
        drop(state);
        let (grid, mapping) =
            source.grid.mapped_reflow_window(offset, self.screen_lines(), self.columns(), focus);
        let mut view = Term::new(
            source.config.clone(),
            &test::TermSize::new(self.columns(), self.screen_lines()),
            ReflowListener,
        );
        view.grid = grid;
        view.colors = self.colors;
        view.mode = self.mode;
        view.mode.remove(TermMode::SHOW_CURSOR);
        Some(ReflowSearchView {
            term: view,
            offset,
            total_lines: source.history_size() + self.screen_lines(),
            cells: mapping,
        })
    }

    /// Publish only when no parser operation can have been omitted. The caller
    /// holds the live terminal lock; the worker never takes that lock.
    pub fn finish_pending_reflow(&mut self) -> bool {
        let Some(shared) = &self.reflow.0 else {
            return false;
        };
        let mut state = lock(shared);
        if !state.updates.is_empty() {
            return false;
        }
        let Some(mut prepared) = state.prepared.take() else {
            return false;
        };
        state.stopped = true;
        shared.changed.notify_all();
        drop(state);
        self.reflow = PendingReflow::default();
        macro_rules! publish {
            ($($field:ident),+ $(,)?) => {$(mem::swap(&mut self.$field, &mut prepared.$field);)+};
        }
        // Focus and the selection/vi cursor belong to the viewer. Everything
        // the output parser can change comes from the ordered private terminal.
        let vi = self.mode.contains(TermMode::VI);
        publish!(
            grid,
            inactive_grid,
            active_charset,
            tabs,
            mode,
            hosted_output_mode,
            scroll_region,
            colors,
            cursor_style,
            title,
            title_stack,
            keyboard_mode_stack,
            inactive_keyboard_mode_stack,
            damage,
            config
        );
        self.mode.set(TermMode::VI, vi);
        self.damage.full = true;
        true
    }
}

impl<T: EventListener + Clone + Send + Sync + 'static> Term<T> {
    #[cfg(feature = "test-support")]
    pub fn cancel_pending_reflow_for_test(&mut self) {
        self.reflow = PendingReflow::default();
    }

    #[cfg(feature = "test-support")]
    pub fn resize_with_async_history_for_test<S: Dimensions>(
        &mut self,
        size: S,
        before_reflow: impl FnOnce() + Send + 'static,
    ) -> std::io::Result<()> {
        self.start_reflow(size, before_reflow)
    }

    /// Reflow a large history privately; the mutable recent-history tier and
    /// both visible screens resize immediately and continue accepting output.
    pub fn resize_with_async_history<S: Dimensions>(&mut self, size: S) -> std::io::Result<()> {
        self.start_reflow(size, || {})
    }

    fn start_reflow<S: Dimensions>(
        &mut self,
        size: S,
        before_reflow: impl FnOnce() + Send + 'static,
    ) -> std::io::Result<()> {
        if self.reflow.0.is_some() {
            defer_history_drop(self.grid.take_archived_history());
            defer_history_drop(self.inactive_grid.take_archived_history());
            self.resize(size);
            return Ok(());
        }
        if self.columns() == size.columns() {
            self.resize(size);
            return Ok(());
        }
        let columns = size.columns();
        let lines = size.screen_lines();
        let shared = Arc::new(Shared::default());
        let (send_source, receive_source) = mpsc::channel::<Arc<Term<ReflowListener>>>();
        let worker = Arc::clone(&shared);
        let listener = self.event_proxy.clone();
        thread::Builder::new().name("terminal-history-reflow".into()).spawn(move || {
            let Ok(source) = receive_source.recv() else { return };
            before_reflow();
            let mut state = lock(&worker);
            if state.stopped {
                state.source.take();
                return;
            }
            drop(state);
            // The browsing source and parser share compact chunks. Clone the
            // worker's chunk index here rather than copying it twice on the UI.
            let mut prepared = source.as_ref().clone();
            drop(source);
            prepared.resize(test::TermSize::new(columns, lines));
            run(prepared, worker, || listener.send_event(Event::Wakeup));
        })?;
        // Start the worker before mutating the live grid: a thread creation
        // failure must leave history and backend geometry untouched. Moving
        // the archive index avoids history-sized copying on the UI thread.
        let view_offset = self.grid.display_offset();
        let grid = self.grid.take_reflow_source();
        let inactive_grid = self.inactive_grid.take_reflow_source();
        let source = Arc::new(self.snapshot_with_grids(ReflowListener, grid, inactive_grid));
        {
            let mut state = lock(&shared);
            state.view_offset = view_offset;
            state.source = Some(Arc::clone(&source));
        }
        send_source.send(source).expect("reflow worker waits for its source");
        self.reflow = PendingReflow(Some(shared));
        // The snapshot owns every old cell. Only the live preview detaches the
        // older tier, so no off-screen suffix is ever truncated or overwritten.
        defer_history_drop(self.grid.take_archived_history());
        defer_history_drop(self.inactive_grid.take_archived_history());
        let _initial_resize = self.reflow.enter();
        self.resize(size);
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/term/reflow.rs"]
mod tests;
