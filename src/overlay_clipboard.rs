//! Asynchronous overlay paste and the input that follows it.
//!
//! Reads start at invocation, including consecutive pastes. Keyboard edits are
//! replayed in order through each surface's normal handler, preserving search,
//! validation and submit effects. Replacing, dismissing or editing the target
//! outside this queue invalidates it; a late read never edits another field.

use crate::*;
use std::collections::VecDeque;

#[derive(Default)]
pub(crate) struct OverlayClipboard {
    next: u64,
    target: Option<TextField>,
    events: VecDeque<PendingKey>,
}

struct PendingKey {
    id: u64,
    event: KeyDownEvent,
    // Outer None is an unfinished paste; Some(None) is an ordinary key or an
    // empty/failed read. These all keep their place in the input sequence.
    text: Option<Option<String>>,
}

struct ResolvedPaste(Option<String>);
impl Global for ResolvedPaste {}

/// A dispatched paste uses its captured value. Direct callers can consume a
/// ready self-owned clipboard, but never wait synchronously on an external one.
pub(crate) fn resolved_text(cx: &App) -> Option<String> {
    if let Some(paste) = cx.try_global::<ResolvedPaste>() {
        return paste.0.clone();
    }
    terminal::selection_clipboard::try_read(terminal::selection_clipboard::read(cx))
        .ok()
        .flatten()
        .and_then(|item| item.text())
}

impl OverlayClipboard {
    fn matches(&self, field: Option<&TextField>) -> bool {
        match (&self.target, field) {
            (Some(target), Some(field)) => target.identity() == field.identity() && target == field,
            (None, None) => true,
            _ => false,
        }
    }

    fn clear(&mut self) {
        self.events.clear();
        self.target = None;
    }

    fn complete(&mut self, id: u64, text: Option<String>) {
        if let Some(event) = self.events.iter_mut().find(|event| event.id == id) {
            event.text = Some(text);
        }
    }
}

impl Zetta {
    pub(crate) fn queue_overlay_clipboard(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let paste_chord = crate::text_edit::is_paste_chord(&event.keystroke);
        if !paste_chord && self.overlay_clipboard.events.is_empty() {
            return false;
        }
        let field = self.overlay_text_field().cloned();
        if !self.overlay_clipboard.matches(field.as_ref()) || event.keystroke.key == "escape" {
            self.overlay_clipboard.clear();
        }
        let paste = field.is_some() && paste_chord;
        if !paste && self.overlay_clipboard.events.is_empty() {
            return false;
        }
        self.overlay_clipboard.target = field;
        self.overlay_clipboard.next = self.overlay_clipboard.next.wrapping_add(1);
        let id = self.overlay_clipboard.next;
        self.overlay_clipboard.events.push_back(PendingKey {
            id,
            event: event.clone(),
            text: (!paste).then_some(None),
        });
        if paste {
            // Invoke before spawning: the platform captures the current offer.
            let read = terminal::selection_clipboard::read(cx);
            match terminal::selection_clipboard::try_read(read) {
                Ok(item) => self
                    .overlay_clipboard
                    .complete(id, item.and_then(|item| item.text())),
                Err(read) => {
                    cx.spawn_in(window, async move |this, cx| {
                        let text = read.await.and_then(|item| item.text());
                        this.update_in(cx, |this, window, cx| {
                            this.overlay_clipboard.complete(id, text);
                            this.drain_overlay_clipboard(window, cx);
                        })
                        .ok();
                    })
                    .detach();
                }
            }
        }
        self.drain_overlay_clipboard(window, cx);
        cx.stop_propagation();
        true
    }

    fn drain_overlay_clipboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let field = self.overlay_text_field().cloned();
        if !self.overlay_clipboard.matches(field.as_ref()) {
            self.overlay_clipboard.clear();
            return;
        }
        while self
            .overlay_clipboard
            .events
            .front()
            .is_some_and(|event| event.text.is_some())
        {
            let pending = self.overlay_clipboard.events.pop_front().unwrap();
            cx.set_global(ResolvedPaste(pending.text.flatten()));
            self.dispatch_overlay_key(&pending.event, window, cx);
            cx.remove_global::<ResolvedPaste>();
            self.overlay_clipboard.target = self.overlay_text_field().cloned();
            // A submission/dismissal must not send queued keys to a terminal.
            if self.overlay_clipboard.target.is_none() {
                self.overlay_clipboard.clear();
                break;
            }
        }
        if self.overlay_clipboard.events.is_empty() {
            self.overlay_clipboard.target = None;
        }
    }

    /// Same priority as the modal key dispatcher. Non-text controls and keymap
    /// recording deliberately have no paste target.
    fn overlay_text_field(&mut self) -> Option<&mut TextField> {
        let picking_style = self.is_picking_overlay_style();
        if self.close_tab_confirmation.is_some() {
            return None;
        }
        if let Some(prompt) = self.session_authentication.as_mut() {
            if prompt.working {
                return None;
            }
            return Some(match prompt.field {
                crate::session_auth_ui::SessionAuthenticationField::Secret => &mut prompt.secret,
                crate::session_auth_ui::SessionAuthenticationField::Confirmation => {
                    &mut prompt.confirmation
                }
            });
        }
        #[cfg(feature = "zmux")]
        if let Some(picker) = self.remote_session_picker.as_mut() {
            use crate::remote_session_ui::RemoteSessionField;
            return match picker.field {
                RemoteSessionField::Target => Some(&mut picker.target),
                RemoteSessionField::Port => Some(&mut picker.port),
                RemoteSessionField::KeepAlive => Some(&mut picker.keep_alive),
                _ => None,
            };
        }
        #[cfg(feature = "serial-console")]
        if let Some(prompt) = self.serial_console.as_mut() {
            return (prompt.field == crate::serial_console::SerialField::BaudRate)
                .then_some(&mut prompt.baud_rate);
        }
        if picking_style {
            return None;
        }
        if let Some(picker) = self.theme_picker.as_mut() {
            return Some(&mut picker.query);
        }
        if let Some(picker) = self.tab_icon_picker.as_mut() {
            return Some(&mut picker.query);
        }
        if let Some(editor) = self.settings_editor.as_mut() {
            if editor.keymap_capture.is_some() || editor.open_dropdown.is_some() {
                return None;
            }
            return crate::settings_ui::settings_text_field(editor, editor.focused_input?);
        }
        if let Some(search) = self.tab_search.as_mut() {
            return Some(&mut search.query);
        }
        if let Some(prompt) = self.multi_command.as_mut() {
            return Some(&mut prompt.query);
        }
        if let Some(palette) = self.command_palette.as_mut() {
            return Some(&mut palette.query);
        }
        let tab = self.tabs.get_mut(self.active_tab)?;
        tab.overlay_buffer.as_mut().or(tab.rename_buffer.as_mut())
    }
}

#[cfg(test)]
#[path = "tests/overlay_clipboard.rs"]
mod tests;
