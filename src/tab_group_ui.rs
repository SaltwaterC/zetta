//! The dialog that names a tab group: a new one, from the sidebar's Main row
//! or **zetta: new tab group**, or an existing one being renamed.
//!
//! What a named group is, and what creating, renaming and removing one does
//! to its tabs, belongs to `tab_groups`; this module only collects the name.
//! Validation runs on submit rather than as the name is typed, so the dialog
//! never reports an error about a name the user has not finished.

use super::*;
use crate::tab_groups::validate_tab_group_name;

/// The open dialog.
#[derive(Debug)]
pub(crate) struct TabGroupPrompt {
    pub(crate) name: TextField,
    /// The group being renamed, or `None` for a new one.
    pub(crate) renaming: Option<String>,
    pub(crate) error: Option<String>,
}

impl Zetta {
    pub(crate) fn new_tab_group(
        &mut self,
        _: &NewTabGroup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.tab_groups_enabled() {
            return;
        }
        self.open_tab_group_prompt(None, TextField::default(), window, cx);
    }

    pub(crate) fn prompt_rename_tab_group(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let field = TextField::selected(name.clone());
        self.open_tab_group_prompt(Some(name), field, window, cx);
    }

    fn open_tab_group_prompt(
        &mut self,
        renaming: Option<String>,
        name: TextField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.close_tab_confirmation.is_some() || self.projects.trust_prompt.is_some() {
            return;
        }
        self.command_palette = None;
        self.multi_command = None;
        self.tab_search = None;
        self.settings_editor = None;
        self.tab_group_prompt = Some(TabGroupPrompt {
            name,
            renaming,
            error: None,
        });
        self.tab_group_prompt_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn dismiss_tab_group_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab_group_prompt.take().is_some() {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    /// Creates or renames the group, or keeps the dialog open with the reason
    /// the name was refused.
    pub(crate) fn submit_tab_group_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.tab_group_prompt.as_mut() else {
            return;
        };
        let name = match validate_tab_group_name(
            self.tab_groups.custom_groups(),
            &prompt.name.text,
            prompt.renaming.as_deref(),
        ) {
            Ok(name) => name,
            Err(error) => {
                prompt.error = Some(error);
                cx.notify();
                return;
            }
        };
        let renaming = prompt.renaming.take();
        self.tab_group_prompt = None;
        match renaming {
            Some(from) => {
                self.rename_tab_group(&from, name, cx);
                self.focus_active(window, cx);
            }
            None => self.create_tab_group(name, window, cx),
        }
    }

    pub(crate) fn tab_group_prompt_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.tab_group_prompt.is_none() {
            return false;
        }
        match event.keystroke.key.as_str() {
            "escape" => {
                self.dismiss_tab_group_prompt(window, cx);
                return true;
            }
            "enter" => {
                self.submit_tab_group_prompt(window, cx);
                return true;
            }
            _ => {}
        }
        let Some(prompt) = self.tab_group_prompt.as_mut() else {
            return true;
        };
        let edited = match apply_clipboard_shortcut(&mut prompt.name, &event.keystroke, cx) {
            ClipboardOutcome::Ignored => apply_text_field_key(&mut prompt.name, &event.keystroke),
            ClipboardOutcome::Unchanged => TextFieldEdit::CursorMoved,
            ClipboardOutcome::Edited => TextFieldEdit::Edited,
        };
        if edited == TextFieldEdit::Edited {
            prompt.error = None;
        }
        if edited != TextFieldEdit::Ignored {
            cx.notify();
        }
        true
    }

    pub(crate) fn render_tab_group_prompt_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let prompt = self.tab_group_prompt.as_ref()?;
        let theme = self.window_theme(cx);
        let colors = theme.colors().clone();
        let error_color = theme.status().error;
        let handle = cx.entity().downgrade();
        let (title, confirm) = match &prompt.renaming {
            Some(name) => (format!("Rename {name}"), "Rename"),
            None => ("New tab group".to_owned(), "Create"),
        };
        let panel = dialog_panel("tab-group-prompt", DIALOG_WIDTH_SMALL, &colors)
            .child(dialog_title(title, &colors))
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        Label::new("Name")
                            .size(LabelSize::Small)
                            .color(Color::Custom(colors.text_muted)),
                    )
                    .child(
                        boxed_text_field(
                            "tab-group-name",
                            &prompt.name,
                            true,
                            Some("scratch".into()),
                            FieldMask::Plain,
                            &colors,
                        )
                        .w_full()
                        .cursor_text(),
                    ),
            )
            .when(prompt.renaming.is_none(), |panel| {
                panel.child(
                    div()
                        .text_sm()
                        .text_color(colors.text_muted)
                        .child("Listed under Main, and opens with a new tab."),
                )
            })
            .when_some(prompt.error.clone(), |panel, error| {
                panel.child(crate::ui_messages::error_message(error, error_color))
            })
            .child(hint_line(
                key_hints(&[("Enter", confirm), ("Esc", "cancel")]),
                &colors,
            ))
            .child(tab_group_prompt_buttons(confirm, &colors, &handle));
        Some(modal(
            modal_backdrop(
                "tab-group-prompt-overlay",
                Placement::Centered,
                BackdropClick::Swallow,
            )
            .track_focus(&self.tab_group_prompt_focus),
            panel,
        ))
    }
}

fn tab_group_prompt_buttons(
    confirm: &'static str,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> gpui::Div {
    let cancel_handle = handle.clone();
    let confirm_handle = handle.clone();
    dialog_buttons()
        .child(
            DialogButton::new("cancel-tab-group-prompt", "Cancel", ButtonRole::Secondary)
                .key_tooltip("Cancel", SurfaceKey::Escape)
                .render(colors, move |_, window, cx| {
                    cancel_handle
                        .update(cx, |this, cx| this.dismiss_tab_group_prompt(window, cx))
                        .ok();
                }),
        )
        .child(
            DialogButton::new("confirm-tab-group-prompt", confirm, ButtonRole::Primary)
                .key_tooltip(confirm, SurfaceKey::Enter)
                .render(colors, move |_, window, cx| {
                    confirm_handle
                        .update(cx, |this, cx| this.submit_tab_group_prompt(window, cx))
                        .ok();
                }),
        )
}
