//! Copy and paste actions, including reads that wait for pending selection extraction.

use super::*;

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
        if !self.input_enabled {
            return;
        }
        // Copy-on-select and vi yank can still be queued for the next prepaint.
        self.terminal
            .update(cx, |terminal, cx| terminal.sync(window, cx));
        let clipboard = terminal::selection_clipboard::read(cx);
        let clipboard = match terminal::selection_clipboard::try_read(clipboard) {
            Ok(Some(clipboard)) => {
                self.apply_clipboard(clipboard, kind, cx);
                return;
            }
            Ok(None) => return,
            Err(clipboard) => clipboard,
        };
        cx.spawn(async move |view, cx| {
            if let Some(clipboard) = clipboard.await {
                let _ = view.update(cx, |view, cx| view.apply_clipboard(clipboard, kind, cx));
            }
        })
        .detach();
    }

    fn apply_clipboard(
        &mut self,
        clipboard: ClipboardItem,
        kind: PasteKind,
        cx: &mut Context<Self>,
    ) {
        if !self.input_enabled {
            return;
        }
        if self.search_query.is_none()
            && matches!(kind, PasteKind::Any)
            && let Some(image) = first_clipboard_image(&clipboard)
        {
            self.paste_image(image, cx);
            return;
        }
        if let Some(text) = clipboard.text() {
            let text = if matches!(kind, PasteKind::Trimmed) {
                trim_paste_text(&text)
            } else {
                &text
            };
            if self.search_query.is_some() {
                self.insert_search_text(text, cx);
            } else {
                self.paste_text_value(text, cx);
            }
        }
    }

    pub(super) fn paste_text_value(&mut self, text: &str, cx: &mut Context<Self>) {
        self.terminal.update(cx, |terminal, _| terminal.paste(text));
        if let Some(event) = enabled_input_event(self.emit_input_events, || {
            TerminalViewEvent::Input(TerminalInput::Paste(text.to_owned()))
        }) {
            cx.emit(event);
        }
    }

    pub(super) fn paste_image(&mut self, image: Arc<gpui::Image>, cx: &mut Context<Self>) {
        let option_as_meta = TerminalSettings::get_global(cx).option_as_meta;
        self.terminal.update(cx, |terminal, _| {
            terminal.paste_image(image.clone(), option_as_meta)
        });
        if let Some(event) = enabled_input_event(self.emit_input_events, || {
            TerminalViewEvent::Input(TerminalInput::PasteImage(image))
        }) {
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

    pub(super) fn clipboard_has_content(cx: &App) -> bool {
        cx.read_from_clipboard().is_some_and(|clipboard| {
            clipboard.text().is_some() || first_clipboard_image(&clipboard).is_some()
        })
    }
}
