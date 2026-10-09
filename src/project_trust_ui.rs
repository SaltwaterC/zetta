//! Review and acceptance of project commands using Zetta's shared dialog UI.
//!
//! A prompt owns the exact fingerprint and text it displayed. Acceptance persists
//! that fingerprint, then reloads through the normal approval gate; a file changed
//! while the prompt was open cannot acquire approval for its replacement contents.

use super::*;
use crate::project::{ProjectConfig, ProjectRegistry};
use crate::ui_tokens::RADIUS_CONTROL;

#[derive(Clone, Debug)]
pub(crate) struct ProjectTrustPrompt {
    root: PathBuf,
    fingerprint: String,
    review: SharedString,
    pub(crate) focus: gpui::FocusHandle,
    trust_selected: bool,
    saving: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum ReviewAction {
    ContinueWithout,
    Choose,
    Approve,
    Ignore,
}

fn review_action(key: &str, trust_selected: bool, saving: bool) -> ReviewAction {
    if saving {
        return ReviewAction::Ignore;
    }
    match key {
        "escape" => ReviewAction::ContinueWithout,
        "tab" | "left" | "right" => ReviewAction::Choose,
        "enter" if trust_selected => ReviewAction::Approve,
        "enter" => ReviewAction::ContinueWithout,
        _ => ReviewAction::Ignore,
    }
}

impl Zetta {
    pub(crate) fn project_needing_approval(&self) -> Option<&Arc<ProjectConfig>> {
        self.active_project_config()
            .filter(|project| project.pending_approval.is_some())
    }

    pub(crate) fn open_project_trust_prompt(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project_needing_approval() else {
            return;
        };
        let Some(approval) = &project.pending_approval else {
            return;
        };
        let prompt = ProjectTrustPrompt {
            root: project.root.clone(),
            fingerprint: approval.fingerprint.clone(),
            review: approval.review.clone().into(),
            focus: cx.focus_handle(),
            trust_selected: false,
            saving: false,
        };
        prompt.focus.focus(window, cx);
        self.projects.trust_prompt = Some(prompt);
        cx.notify();
    }

    pub(crate) fn dismiss_project_trust(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .projects
            .trust_prompt
            .as_ref()
            .is_some_and(|prompt| prompt.saving)
        {
            return;
        }
        self.projects.trust_prompt = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    fn approve_project_execution(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self
            .projects
            .trust_prompt
            .as_mut()
            .filter(|prompt| !prompt.saving)
        else {
            return;
        };
        prompt.saving = true;
        let root = prompt.root.clone();
        let fingerprint = prompt.fingerprint.clone();
        let registry_path = self.projects.registry.path().to_path_buf();
        let base = self.launch_config.clone();
        let executor = cx.background_executor().clone();
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let result = executor
                    .spawn(async move {
                        let mut registry = ProjectRegistry::load_from(registry_path)?;
                        registry.approve(&root, &fingerprint)?;
                        registry.save()?;
                        let config = ProjectConfig::load_in_registry(&root, &base, &registry)?;
                        Ok::<_, anyhow::Error>((registry, config))
                    })
                    .await;
                this.update_in(cx, |this, window, cx| {
                    match result {
                        Ok((registry, config)) => {
                            this.projects.registry = registry;
                            this.projects.insert_config(config);
                            this.projects.trust_prompt = None;
                            this.projects.invalidate_active_context();
                            this.activate_current_project(window, cx);
                            reload_projects_in_other_windows(
                                window.window_handle().window_id(),
                                cx,
                            );
                            this.focus_active(window, cx);
                        }
                        Err(error) => {
                            if let Some(prompt) = this.projects.trust_prompt.as_mut() {
                                prompt.saving = false;
                            }
                            this.show_error_notice(
                                format!("Could not approve project commands: {error:#}"),
                                cx,
                            );
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        cx.notify();
    }

    pub(crate) fn project_trust_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(prompt) = self.projects.trust_prompt.as_mut() else {
            return false;
        };
        match review_action(
            event.keystroke.key.as_str(),
            prompt.trust_selected,
            prompt.saving,
        ) {
            ReviewAction::ContinueWithout => self.dismiss_project_trust(window, cx),
            ReviewAction::Choose => {
                prompt.trust_selected = !prompt.trust_selected;
                cx.notify();
            }
            ReviewAction::Approve => self.approve_project_execution(window, cx),
            ReviewAction::Ignore => {}
        }
        true
    }

    pub(crate) fn render_project_trust_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let prompt = self.projects.trust_prompt.as_ref()?;
        let colors = self.window_theme(cx).colors().clone();
        let cancel_handle = cx.entity().downgrade();
        let trust_handle = cancel_handle.clone();
        Some(project_trust_dialog(
            prompt,
            &colors,
            move |_, window, cx| {
                cancel_handle
                    .update(cx, |this, cx| this.dismiss_project_trust(window, cx))
                    .ok();
            },
            move |_, window, cx| {
                trust_handle
                    .update(cx, |this, cx| this.approve_project_execution(window, cx))
                    .ok();
            },
        ))
    }
}

fn project_trust_dialog(
    prompt: &ProjectTrustPrompt,
    colors: &ThemeColors,
    cancel: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    approve: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let panel = dialog_panel("project-trust-dialog", DIALOG_WIDTH_LARGE, colors)
            .h(px(540.))
            .debug_selector(|| "project-trust-panel".to_owned())
            .child(dialog_title("Trust project commands?", colors))
            .child(div().text_sm().text_color(colors.text_muted).child(
                "These project commands were loaded from disk. Review them before allowing Zetta to run them."
            ))
            .child(div().flex_none().truncate().text_sm().child(ProjectConfig::path_for(&prompt.root).display().to_string()))
            .child(div().id("project-trust-settings").debug_selector(|| "project-trust-review".to_owned()).min_h_0().flex_1().overflow_y_scroll()
                .p_3().rounded(RADIUS_CONTROL).bg(colors.editor_background)
                .text_sm().child(prompt.review.clone()))
            .child(hint_line(key_hints(&[("Tab", "choose"), ("Enter", "activate"), ("Esc", "continue without")]), colors))
            .child(dialog_buttons().flex_none().flex_wrap().debug_selector(|| "project-trust-actions".to_owned())
                .child(DialogButton::new("cancel-project-trust", "Continue without", ButtonRole::Secondary)
                    .focused(!prompt.trust_selected).enabled(!prompt.saving)
                    .key_tooltip("Continue without project commands", SurfaceKey::Escape)
                    .render(colors, cancel))
                .child(DialogButton::new("approve-project-trust", "Trust commands", ButtonRole::Primary)
                    .focused(prompt.trust_selected).loading(prompt.saving)
                    .render(colors, approve)));
    modal(
        modal_backdrop(
            "project-trust-overlay",
            Placement::Centered,
            BackdropClick::Swallow,
        )
        .track_focus(&prompt.focus),
        panel,
    )
}

#[cfg(test)]
#[path = "tests/project_trust_ui.rs"]
mod tests;
