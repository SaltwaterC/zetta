//! Confirmation before running a shell script macOS asked Zetta to open.
//!
//! Setting Zetta as the default terminal makes it the handler for `.command`,
//! `.tool`, `.zsh`, `.csh`, `.sh` and `.pl` files, and opening one runs it in a
//! new tab. Without a prompt, double-clicking a downloaded `.sh` in Finder would
//! execute it where an editor used to open it, so every open is held here until
//! the user chooses to run it. **Cancel** is the default choice for both the
//! keyboard and a stray Enter.
//!
//! Each open request is confirmed separately. A file that arrives while a prompt
//! is showing waits for its own prompt, so it can never join a list the user is
//! already about to accept.

use std::collections::VecDeque;

use super::*;
use crate::command_panes::PaneCommand;
use crate::ui_tokens::RADIUS_CONTROL;

/// The open requests still waiting for an answer, oldest first. The front one
/// is the one on screen.
#[derive(Debug, Default)]
pub(crate) struct ScriptOpenQueue {
    requests: VecDeque<Vec<PathBuf>>,
    run_selected: bool,
}

impl ScriptOpenQueue {
    pub(crate) fn is_open(&self) -> bool {
        !self.requests.is_empty()
    }

    fn showing(&self) -> Option<&[PathBuf]> {
        self.requests.front().map(Vec::as_slice)
    }

    /// Queues one open request; an empty one is dropped rather than prompted.
    fn push(&mut self, scripts: Vec<PathBuf>) {
        if !scripts.is_empty() {
            self.requests.push_back(scripts);
        }
    }

    /// Answers the request on screen. The next one, if any, starts again with
    /// **Cancel** selected.
    fn answer(&mut self) -> Option<Vec<PathBuf>> {
        self.run_selected = false;
        self.requests.pop_front()
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ScriptOpenAction {
    Cancel,
    Choose,
    Run,
    Ignore,
}

fn script_open_action(key: &str, run_selected: bool) -> ScriptOpenAction {
    match key {
        "escape" => ScriptOpenAction::Cancel,
        "tab" | "left" | "right" => ScriptOpenAction::Choose,
        "enter" if run_selected => ScriptOpenAction::Run,
        "enter" => ScriptOpenAction::Cancel,
        _ => ScriptOpenAction::Ignore,
    }
}

impl Zetta {
    /// Holds the scripts named by a macOS open request for confirmation. Nothing
    /// is started until [`Self::run_confirmed_scripts`].
    pub(crate) fn open_script_urls(
        &mut self,
        urls: &[String],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let scripts = crate::default_terminal::script_paths_from_urls(urls);
        if scripts.is_empty() {
            return;
        }
        self.script_open.push(scripts);
        self.script_open_focus.focus(window, cx);
        cx.notify();
    }

    fn cancel_script_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.script_open.answer();
        self.after_script_open_answer(window, cx);
    }

    fn run_confirmed_scripts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(scripts) = self.script_open.answer() else {
            return;
        };
        let mut rejected = 0;
        for path in scripts {
            let request = PaneCommand {
                direction: None,
                label: None,
                pane: None,
                overlay: None,
                stack: false,
                list: false,
                command: vec![path.to_string_lossy().into_owned()],
            };
            if self
                .open_command_in_new_tab(request, path.parent().map(Path::to_path_buf), window, cx)
                .is_err()
            {
                rejected += 1;
            }
        }
        if rejected != 0 {
            self.show_error_notice(
                format!("Could not open {rejected} shell-script file(s) in Zetta"),
                cx,
            );
        }
        self.after_script_open_answer(window, cx);
    }

    fn after_script_open_answer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.script_open.is_open() {
            self.script_open_focus.focus(window, cx);
        } else {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    pub(crate) fn script_open_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.script_open.is_open() {
            return false;
        }
        match script_open_action(event.keystroke.key.as_str(), self.script_open.run_selected) {
            ScriptOpenAction::Cancel => self.cancel_script_open(window, cx),
            ScriptOpenAction::Choose => {
                self.script_open.run_selected = !self.script_open.run_selected;
                cx.notify();
            }
            ScriptOpenAction::Run => self.run_confirmed_scripts(window, cx),
            ScriptOpenAction::Ignore => {}
        }
        true
    }

    pub(crate) fn render_script_open_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let scripts = self.script_open.showing()?;
        let colors = self.window_theme(cx).colors().clone();
        let cancel_handle = cx.entity().downgrade();
        let run_handle = cancel_handle.clone();
        Some(script_open_dialog(
            scripts,
            self.script_open.run_selected,
            &self.script_open_focus,
            &colors,
            move |_, window, cx| {
                cancel_handle
                    .update(cx, |this, cx| this.cancel_script_open(window, cx))
                    .ok();
            },
            move |_, window, cx| {
                run_handle
                    .update(cx, |this, cx| this.run_confirmed_scripts(window, cx))
                    .ok();
            },
        ))
    }
}

fn script_open_dialog(
    scripts: &[PathBuf],
    run_selected: bool,
    focus: &gpui::FocusHandle,
    colors: &ThemeColors,
    cancel: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    run: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let (title, explanation) = if scripts.len() == 1 {
        (
            "Run shell script?".to_owned(),
            "Opening this file runs it in a new tab. Run it only if you trust where it came from.",
        )
    } else {
        (
            format!("Run {} shell scripts?", scripts.len()),
            "Opening these files runs each of them in a new tab. Run them only if you trust where they came from.",
        )
    };
    let listing = scripts
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let panel = dialog_panel("script-open-dialog", DIALOG_WIDTH_LARGE, colors)
        .debug_selector(|| "script-open-panel".to_owned())
        .child(dialog_title(title, colors))
        .child(
            div()
                .text_sm()
                .text_color(colors.text_muted)
                .child(explanation),
        )
        .child(
            div()
                .id("script-open-paths")
                .debug_selector(|| "script-open-paths".to_owned())
                .min_h_0()
                .overflow_y_scroll()
                .p_3()
                .rounded(RADIUS_CONTROL)
                .bg(colors.editor_background)
                .text_sm()
                .child(listing),
        )
        .child(hint_line(
            key_hints(&[("Tab", "choose"), ("Enter", "activate"), ("Esc", "cancel")]),
            colors,
        ))
        .child(
            dialog_buttons()
                .flex_none()
                .flex_wrap()
                .debug_selector(|| "script-open-actions".to_owned())
                .child(
                    DialogButton::new("cancel-script-open", "Cancel", ButtonRole::Secondary)
                        .focused(!run_selected)
                        .key_tooltip("Do not run", SurfaceKey::Escape)
                        .render(colors, cancel),
                )
                .child(
                    DialogButton::new("run-script-open", "Run", ButtonRole::Destructive)
                        .focused(run_selected)
                        .render(colors, run),
                ),
        );
    modal(
        modal_backdrop(
            "script-open-overlay",
            Placement::Centered,
            BackdropClick::Swallow,
        )
        .track_focus(focus),
        panel,
    )
}

#[cfg(test)]
#[path = "tests/script_open_ui.rs"]
mod tests;
