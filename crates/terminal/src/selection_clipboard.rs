//! Selection text belongs to clipboard requests, never render snapshots.
//!
//! Capture the grid at the request's position in the terminal event stream.
//! Archived rows are shared; only the bounded mutable prefix is copied. Text
//! traversal runs on that immutable snapshot without the live grid lock.
//! Versions are application-wide because different panes share the clipboard.

use crate::alacritty::AlacrittyTerm;
use futures::{FutureExt as _, future::Shared};
use gpui::{App, ClipboardItem, Global, Task};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub(crate) mod primary;

#[derive(Default)]
struct Slot {
    version: Arc<AtomicU64>,
    extraction_gate: Arc<futures::lock::Mutex<()>>,
    pending: Option<Shared<Task<()>>>,
    #[cfg(test)]
    extractions: Arc<AtomicU64>,
}

#[derive(Default)]
struct SelectionClipboard([Slot; 2]);

impl Global for SelectionClipboard {}

fn invalidate(slot: usize, cx: &mut App) -> u64 {
    let slot = &mut cx.default_global::<SelectionClipboard>().0[slot];
    let version = slot
        .version
        .load(Ordering::Relaxed)
        .checked_add(1)
        .expect("clipboard version overflow");
    slot.version.store(version, Ordering::Release);
    slot.pending = None;
    version
}

fn request(
    slot: usize,
    term: &AlacrittyTerm,
    write: fn(&App, ClipboardItem),
    cx: &mut App,
) -> bool {
    // An empty selection leaves clipboard ownership unchanged, including a
    // pending primary copy when CopyAndClearSelection clears its highlight.
    if term
        .selection
        .as_ref()
        .and_then(|selection| selection.to_range(term))
        .is_none()
    {
        return false;
    }
    let version = invalidate(slot, cx);
    let snapshot = term.clone();
    let state = &cx.global::<SelectionClipboard>().0[slot];
    let current = state.version.clone();
    let gate = state.extraction_gate.clone();
    #[cfg(test)]
    let extractions = state.extractions.clone();
    let extraction = cx.background_executor().spawn(async move {
        // Cancelling a task cannot interrupt a synchronous extraction already
        // running. Serialize workers per clipboard so rapid drag updates cannot
        // leave many history-sized traversals running in parallel. Waiting work
        // checks its version before traversing, even if a reader kept it alive.
        let _guard = gate.lock().await;
        if current.load(Ordering::Acquire) != version {
            return None;
        }
        #[cfg(test)]
        extractions.fetch_add(1, Ordering::Relaxed);
        log::trace!("extracting selection for clipboard slot {slot}, version {version}");
        snapshot.selection_to_string()
    });
    publish_when_ready(slot, version, extraction, write, cx);
    true
}

fn publish_when_ready(
    slot: usize,
    version: u64,
    extraction: Task<Option<String>>,
    write: fn(&App, ClipboardItem),
    cx: &mut App,
) {
    let publication = cx
        .spawn(async move |cx| {
            let text = extraction.await;
            cx.update(|cx| {
                let state = &mut cx.default_global::<SelectionClipboard>().0[slot];
                if state.version.load(Ordering::Acquire) != version {
                    return;
                }
                state.pending = None;
                if let Some(text) = text {
                    write(cx, ClipboardItem::new_string(text));
                }
            });
        })
        .shared();
    cx.default_global::<SelectionClipboard>().0[slot].pending = Some(publication);
}

pub(crate) fn copy(term: &AlacrittyTerm, cx: &mut App) -> bool {
    request(0, term, App::write_to_clipboard, cx)
}

/// Replace the clipboard and invalidate older selection extractions.
pub fn write(item: ClipboardItem, cx: &mut App) {
    invalidate(0, cx);
    cx.write_to_clipboard(item);
}

fn pending(slot: usize, cx: &App) -> Option<Shared<Task<()>>> {
    cx.try_global::<SelectionClipboard>()?.0[slot]
        .pending
        .clone()
}

fn read_slot(
    slot: usize,
    read: fn(&App) -> Option<ClipboardItem>,
    cx: &App,
) -> Task<Option<ClipboardItem>> {
    if pending(slot, cx).is_none() {
        return Task::ready(read(cx));
    }
    cx.spawn(async move |cx| {
        loop {
            let next = cx.update(|cx| pending(slot, cx));
            if let Some(next) = next {
                next.await;
            } else {
                return cx.update(|cx| read(cx));
            }
        }
    })
}

/// Wait for the newest requested selection before reading the clipboard.
pub fn read(cx: &App) -> Task<Option<ClipboardItem>> {
    read_slot(0, App::read_from_clipboard, cx)
}

/// Return an already available read without scheduling a foreground callback.
/// An unfinished read is returned intact so callers can await it.
pub fn try_read(
    mut read: Task<Option<ClipboardItem>>,
) -> Result<Option<ClipboardItem>, Task<Option<ClipboardItem>>> {
    match (&mut read).now_or_never() {
        Some(item) => Ok(item),
        None => Err(read),
    }
}

impl crate::Terminal {
    pub(super) fn copy_selection(
        &mut self,
        keep_selection: Option<bool>,
        term: &AlacrittyTerm,
        cx: &mut gpui::Context<Self>,
    ) {
        if copy(term, cx)
            && !keep_selection
                .unwrap_or_else(|| crate::TerminalSettings::get_global(cx).keep_selection_on_copy)
        {
            self.events
                .push_back(crate::InternalEvent::SetSelection(None));
        }
    }

    pub(super) fn read_selection_clipboard(
        &mut self,
        format: crate::ClipboardFormatter,
        cx: &mut gpui::Context<Self>,
    ) {
        match try_read(read(cx)) {
            Ok(item) => {
                let text = item.and_then(|item| item.text()).unwrap_or_default();
                self.write_to_pty(format(&text).into_bytes());
            }
            Err(clipboard) => {
                cx.spawn(async move |terminal, cx| {
                    let text = clipboard
                        .await
                        .and_then(|item| item.text())
                        .unwrap_or_default();
                    let _ = terminal.update(cx, |terminal, _| {
                        terminal.write_to_pty(format(&text).into_bytes());
                    });
                })
                .detach();
            }
        }
    }

    pub(super) fn paste_selection_clipboard(&mut self, cx: &mut gpui::Context<Self>) {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        let clipboard = primary::read_for_paste(cx);
        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        let clipboard = read(cx);
        match try_read(clipboard) {
            Ok(item) => {
                if let Some(text) = item.and_then(|item| item.text()) {
                    self.paste(&text);
                }
            }
            Err(clipboard) => {
                cx.spawn(async move |terminal, cx| {
                    if let Some(text) = clipboard.await.and_then(|item| item.text()) {
                        let _ = terminal.update(cx, |terminal, _| terminal.paste(&text));
                    }
                })
                .detach();
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/selection_clipboard.rs"]
mod tests;
