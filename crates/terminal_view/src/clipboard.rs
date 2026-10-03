//! Copy and paste actions. Pastes read the clipboard without blocking — waiting
//! for pending selection extraction or for another client to send its
//! contents — and hold the pane's later input behind them while they wait.

use super::*;

#[derive(Clone, Copy)]
enum PasteKind {
    Any,
    Text,
    Trimmed,
}

impl TerminalView {
    pub(super) fn copy(&mut self, _: &Copy, window: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |terminal, cx| {
            terminal.copy(None);
            terminal.sync(window, cx);
        });
    }

    pub(super) fn copy_and_clear_selection(
        &mut self,
        _: &CopyAndClearSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal.update(cx, |terminal, cx| {
            terminal.copy(Some(false));
            terminal.sync(window, cx);
        });
    }

    pub(super) fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        self.paste_clipboard(PasteKind::Any, window, cx);
    }

    pub(super) fn paste_text(
        &mut self,
        _: &PasteText,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.paste_clipboard(PasteKind::Text, window, cx);
    }

    pub(super) fn paste_trimmed(
        &mut self,
        _: &PasteTrimmed,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.paste_clipboard(PasteKind::Trimmed, window, cx);
    }

    fn paste_clipboard(&mut self, kind: PasteKind, window: &mut Window, cx: &mut Context<Self>) {
        self.read_clipboard_for_paste(kind, None, window, cx);
    }

    /// A right click that would paste: the clipboard's content is only known
    /// once it has been read, so an empty one opens the menu at `position`
    /// when the read resolves instead of being probed with a blocking read.
    pub(super) fn paste_or_deploy_context_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.read_clipboard_for_paste(PasteKind::Any, Some(position), window, cx);
    }

    /// Reads the clipboard without blocking and pastes what it holds.
    ///
    /// Where the paste goes is decided now, not when the read resolves, and a
    /// paste into the terminal that has to wait reserves its place in the
    /// pane's input — and in the input events this view rebroadcasts — so
    /// keys typed while the owner is still sending land after it.
    fn read_clipboard_for_paste(
        &mut self,
        kind: PasteKind,
        menu_if_empty: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.input_enabled {
            if let Some(position) = menu_if_empty {
                self.deploy_context_menu(position, window, cx);
            }
            return;
        }
        // Copy-on-select and vi yank can still be queued for the next prepaint.
        self.terminal
            .update(cx, |terminal, cx| terminal.sync(window, cx));
        let request = PasteRequest {
            kind,
            target: if self.search_query.is_some() {
                PasteTarget::Search
            } else {
                PasteTarget::Terminal
            },
            menu_if_empty,
        };
        let clipboard = terminal::selection_clipboard::read(cx);
        let clipboard = match terminal::selection_clipboard::try_read(clipboard) {
            Ok(clipboard) => {
                self.deliver_paste(clipboard, request, None, window, cx);
                return;
            }
            Err(clipboard) => clipboard,
        };
        let order = (request.target == PasteTarget::Terminal).then(|| PasteOrderTickets {
            terminal: self.terminal.downgrade(),
            input: self
                .terminal
                .update(cx, |terminal, _| terminal.begin_paste()),
            events: self.input_event_order.begin(),
        });
        cx.spawn_in(window, async move |view, cx| {
            let clipboard = clipboard.await;
            let mut order = order;
            let delivered = view.update_in(cx, |view, window, cx| {
                let order = order.take();
                view.deliver_paste(clipboard, request, order, window, cx);
            });
            // The view or its window is gone. The terminal can outlive both,
            // and would otherwise hold its input behind this paste forever.
            if delivered.is_err()
                && let Some(order) = order
            {
                order
                    .terminal
                    .update(cx, |terminal, _| terminal.finish_paste(order.input, |_| {}))
                    .ok();
            }
        })
        .detach();
    }

    fn deliver_paste(
        &mut self,
        clipboard: Option<ClipboardItem>,
        request: PasteRequest,
        order: Option<PasteOrderTickets>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let clipboard = clipboard.filter(clipboard_has_content);
        if clipboard.is_none()
            && let Some(position) = request.menu_if_empty
        {
            self.deploy_context_menu(position, window, cx);
        }
        let pasted = match clipboard {
            Some(clipboard) if self.input_enabled => self.pasted(&clipboard, request),
            _ => Pasted::Nothing,
        };
        self.apply_pasted(pasted, order, cx);
    }

    /// What `clipboard` pastes as, for the paste `request` asked for.
    fn pasted(&self, clipboard: &ClipboardItem, request: PasteRequest) -> Pasted {
        let text = || {
            clipboard.text().map(|text| match request.kind {
                PasteKind::Trimmed => trim_paste_text(&text).to_owned(),
                PasteKind::Any | PasteKind::Text => text,
            })
        };
        match request.target {
            // A search that closed while the clipboard was read has nowhere to
            // put the text, and the pty was not what was asked for.
            PasteTarget::Search if self.search_query.is_none() => Pasted::Nothing,
            PasteTarget::Search => text().map_or(Pasted::Nothing, Pasted::Search),
            PasteTarget::Terminal => match request.kind {
                PasteKind::Any => first_clipboard_image(clipboard).map(Pasted::Image),
                PasteKind::Text | PasteKind::Trimmed => None,
            }
            .or_else(|| text().map(Pasted::Text))
            .unwrap_or(Pasted::Nothing),
        }
    }

    fn apply_pasted(
        &mut self,
        pasted: Pasted,
        order: Option<PasteOrderTickets>,
        cx: &mut Context<Self>,
    ) {
        if let Pasted::Search(text) = &pasted {
            self.insert_search_text(text, cx);
        }
        let event = match &pasted {
            Pasted::Text(text) => Some(TerminalInput::Paste(text.clone())),
            Pasted::Image(image) => Some(TerminalInput::PasteImage(image.clone())),
            Pasted::Search(_) | Pasted::Nothing => None,
        }
        .filter(|_| self.emit_input_events)
        .map(TerminalViewEvent::Input);
        let option_as_meta = TerminalSettings::get_global(cx).option_as_meta;
        let write = |terminal: &mut Terminal| match &pasted {
            Pasted::Text(text) => terminal.paste(text),
            Pasted::Image(image) => terminal.paste_image(image.clone(), option_as_meta),
            Pasted::Search(_) | Pasted::Nothing => {}
        };
        let Some(order) = order else {
            // Still ordered behind any earlier paste that is waiting: both
            // queues admit it only once nothing is pending ahead of it.
            self.terminal.update(cx, |terminal, _| write(terminal));
            if let Some(event) = event {
                self.emit_input_event(event, cx);
            }
            return;
        };
        // On the terminal the ticket came from; a terminal that has gone has
        // no input left to order.
        order
            .terminal
            .update(cx, |terminal, _| terminal.finish_paste(order.input, write))
            .ok();
        self.input_event_order.capture();
        if let Some(event) = event {
            self.input_event_order.admit(event);
        }
        for event in self.input_event_order.resolve(order.events) {
            cx.emit(event);
        }
    }

    pub(super) fn clear_clipboard(
        &mut self,
        _: &ClearClipboard,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        terminal::selection_clipboard::write(
            ClipboardItem {
                entries: Vec::new(),
            },
            cx,
        );
    }
}

/// Where a paste goes. Decided when the paste is asked for: a paste aimed at
/// the search box must not reach the pty because the search closed while the
/// clipboard was read, nor the reverse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PasteTarget {
    Terminal,
    Search,
}

/// One paste, as it was asked for.
#[derive(Clone, Copy)]
struct PasteRequest {
    kind: PasteKind,
    target: PasteTarget,
    /// Where a right click that found the clipboard empty opens the menu.
    menu_if_empty: Option<Point<Pixels>>,
}

/// What a resolved paste delivers.
enum Pasted {
    Text(String),
    Image(Arc<gpui::Image>),
    Search(String),
    Nothing,
}

/// A waiting paste's places in the pane's pty input and in the input events
/// this view rebroadcasts. Both are resolved on every outcome.
struct PasteOrderTickets {
    terminal: gpui::WeakEntity<Terminal>,
    input: PasteTicket,
    events: PasteTicket,
}

fn clipboard_has_content(clipboard: &ClipboardItem) -> bool {
    clipboard.text().is_some() || first_clipboard_image(clipboard).is_some()
}
