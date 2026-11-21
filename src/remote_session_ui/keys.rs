//! Keyboard handling for the picker: field navigation, dropdowns opened from
//! the keyboard, and the shortcuts its buttons advertise.

use super::*;

impl Zetta {
    pub(crate) fn remote_session_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.remote_session_picker.is_none() {
            return false;
        }
        if self.remote_session_shortcut(event, window, cx) {
            return true;
        }
        if self.remote_session_dropdown_key_down(event, window, cx) {
            return true;
        }
        if event.keystroke.key == "escape" {
            self.dismiss_remote_session_picker(window, cx);
            cx.stop_propagation();
            return true;
        }
        if self.open_remote_dropdown_from_key(event, window, cx) {
            return true;
        }
        if event.keystroke.key == "enter" {
            let action = self
                .remote_session_picker
                .as_ref()
                .map(RemoteSessionPicker::enter_action);
            match action {
                Some(RemoteSessionEnterAction::Attach(selected)) => {
                    self.select_remote_session(selected, window, cx);
                }
                Some(RemoteSessionEnterAction::Cancel) => {
                    self.dismiss_remote_session_picker(window, cx);
                }
                Some(RemoteSessionEnterAction::Load) => self.load_remote_sessions(window, cx),
                Some(RemoteSessionEnterAction::Create) => self.create_remote_session(window, cx),
                Some(RemoteSessionEnterAction::Ignore) | None => {}
            }
            cx.stop_propagation();
            return true;
        }
        let Some(picker) = self.remote_session_picker.as_mut() else {
            return false;
        };
        match (event.keystroke.key.as_str(), picker.field) {
            ("tab", _) => {
                picker.cycle_field(event.keystroke.modifiers.shift);
                picker.reset_suggestion_navigation();
                cx.notify();
            }
            ("left" | "right" | "space", RemoteSessionField::Protocol) => {
                picker.toggle_transport();
                cx.notify();
            }
            ("left" | "right" | "space", RemoteSessionField::ForwardAgent) => {
                picker.forward_agent = !picker.forward_agent;
                cx.notify();
            }
            ("up" | "down", RemoteSessionField::Target) => {
                picker.navigate_suggestions(event.keystroke.key == "up");
                cx.notify();
            }
            ("up" | "down", RemoteSessionField::List) if !picker.sessions.is_empty() => {
                if event.keystroke.key == "up" {
                    picker.selected = picker.selected.saturating_sub(1);
                } else {
                    picker.selected = (picker.selected + 1).min(picker.sessions.len() - 1);
                }
                picker
                    .scroll
                    .scroll_to_item(picker.selected, ScrollStrategy::Nearest);
                cx.notify();
            }
            (
                _,
                RemoteSessionField::Target
                | RemoteSessionField::Port
                | RemoteSessionField::KeepAlive,
            ) => {
                let numeric = matches!(
                    picker.field,
                    RemoteSessionField::Port | RemoteSessionField::KeepAlive
                );
                let field = match picker.field {
                    RemoteSessionField::Target => &mut picker.target,
                    RemoteSessionField::Port => &mut picker.port,
                    RemoteSessionField::KeepAlive => &mut picker.keep_alive,
                    RemoteSessionField::Protocol
                    | RemoteSessionField::ForwardAgent
                    | RemoteSessionField::Profile
                    | RemoteSessionField::Template
                    | RemoteSessionField::List
                    | RemoteSessionField::Cancel
                    | RemoteSessionField::Load
                    | RemoteSessionField::Create
                    | RemoteSessionField::Attach => unreachable!(),
                };
                match apply_clipboard_shortcut(field, &event.keystroke, cx) {
                    ClipboardOutcome::Unchanged => {
                        cx.notify();
                        cx.stop_propagation();
                        return true;
                    }
                    ClipboardOutcome::Edited => {
                        if picker.field == RemoteSessionField::Target {
                            picker.reset_suggestion_navigation();
                        }
                        picker.invalidate_results();
                        cx.notify();
                        cx.stop_propagation();
                        return true;
                    }
                    ClipboardOutcome::Ignored => {}
                }
                // The port and keep-alive fields take digits only, so a
                // character that is not one is dropped before the field sees
                // it; everything else is the shared editing behaviour.
                let typed_a_rejected_character = numeric
                    && event
                        .keystroke
                        .key_char
                        .as_ref()
                        .is_some_and(|text| !text.chars().all(|c| c.is_ascii_digit()));
                let edit = if typed_a_rejected_character {
                    TextFieldEdit::Ignored
                } else {
                    apply_text_field_key(field, &event.keystroke)
                };
                if picker.field == RemoteSessionField::Target && edit == TextFieldEdit::Edited {
                    picker.reset_suggestion_navigation();
                }
                picker.invalidate_results();
                cx.notify();
            }
            _ => {}
        }
        cx.stop_propagation();
        true
    }

    pub(super) fn remote_session_shortcut(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let action = self
            .remote_session_picker
            .as_ref()
            .and_then(|picker| picker.shortcut_action(event));
        let Some(action) = action else {
            return false;
        };
        match action {
            RemoteSessionEnterAction::Create => self.create_remote_session(window, cx),
            RemoteSessionEnterAction::Attach(selected) => {
                self.select_remote_session(selected, window, cx);
            }
            RemoteSessionEnterAction::Cancel
            | RemoteSessionEnterAction::Load
            | RemoteSessionEnterAction::Ignore => {}
        }
        cx.stop_propagation();
        true
    }

    pub(super) fn remote_session_dropdown_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self
            .remote_session_picker
            .as_ref()
            .is_none_or(|picker| picker.open_dropdown.is_none())
        {
            return false;
        }
        let action = self.remote_session_picker.as_mut().map(|picker| {
            let command = event.keystroke.modifiers.control || event.keystroke.modifiers.platform;
            picker.dropdown.key_down(event, command)
        });
        match action {
            Some(SearchableDropdownAction::Close) => {
                if let Some(picker) = self.remote_session_picker.as_mut() {
                    picker.close_dropdown();
                }
                cx.notify();
            }
            Some(SearchableDropdownAction::Commit(Some(choice))) => {
                if let Some(picker) = self.remote_session_picker.as_mut() {
                    picker.commit_dropdown(choice.value);
                }
                cx.notify();
            }
            Some(SearchableDropdownAction::Tab { reverse }) => {
                if let Some(picker) = self.remote_session_picker.as_mut() {
                    picker.close_dropdown();
                    picker.cycle_field(reverse);
                    picker.reset_suggestion_navigation();
                }
                cx.notify();
            }
            Some(SearchableDropdownAction::Commit(None))
            | Some(SearchableDropdownAction::Handled)
            | None => cx.notify(),
        }
        cx.stop_propagation();
        true
    }

    pub(super) fn open_remote_dropdown_from_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(picker) = self.remote_session_picker.as_ref() else {
            return false;
        };
        let dropdown = match picker.field {
            RemoteSessionField::Profile => RemoteSessionDropdown::Profile,
            RemoteSessionField::Template => RemoteSessionDropdown::Template,
            _ => return false,
        };
        let direction = match event.keystroke.key.as_str() {
            "up" | "left" => Some(-1),
            "down" | "right" => Some(1),
            "enter" | "space" => Some(0),
            _ => None,
        };
        let Some(direction) = direction else {
            return false;
        };
        let opened = self
            .remote_session_picker
            .as_mut()
            .is_some_and(|picker| picker.open_dropdown(dropdown, window.mouse_position()));
        if !opened {
            return false;
        }
        if direction != 0
            && let Some(picker) = self.remote_session_picker.as_mut()
        {
            picker.dropdown.move_selection(direction);
        }
        cx.notify();
        cx.stop_propagation();
        true
    }

    pub(crate) fn remote_session_key_down_capture(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key == "escape"
            && let Some(picker) = self.remote_session_picker.as_mut()
        {
            if picker.close_dropdown() {
                cx.notify();
            } else {
                self.dismiss_remote_session_picker(window, cx);
            }
            cx.stop_propagation();
        }
    }
}
