//! Creating and managing a project's worktrees from the sidebar.
//!
//! The same workflow `zwt` drives — `new`, `done`, `abort`, `sync` — run in
//! this process through `zwt`'s library half, which returns what the command
//! line prints and deliberately does not report to an originating tab: this
//! process has none, and when Zetta was itself started from a Zetta pane the
//! variables `zwt` reads name the parent window's tab.
//!
//! Every operation runs `git`, sometimes for a long time (a submodule clone, a
//! rebase), so each is started on the background executor and lands through
//! the dialog's generation: dismissing the dialog, or opening another, makes a
//! late result a no-op instead of a surprise.
//!
//! A worktree that `done` or `abort` removes takes its tab group with it, but
//! not its tabs. Those keep running and move to the project's group: closing
//! them would end processes the user may still want, and a shell left in a
//! deleted directory is easier to recover from than one that is gone.

use super::*;
use crate::project::project_display_name;
use crate::sidebar::{ProjectWorktrees, SidebarWorktree};
use crate::tab_groups::TabGroup;

/// Which of the create dialog's fields has the keyboard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum WorktreeField {
    #[default]
    Name,
    CopyPaths,
}

impl WorktreeField {
    fn adjacent(self) -> Self {
        match self {
            Self::Name => Self::CopyPaths,
            Self::CopyPaths => Self::Name,
        }
    }
}

/// What finishing a worktree does with its branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorktreeFinish {
    /// `done`: rebase onto the source branch, fast-forward it, remove.
    Integrate,
    /// `abort`: remove, discarding every change.
    Discard,
}

#[derive(Debug)]
pub(crate) enum WorktreePromptKind {
    Create {
        name: TextField,
        copy_paths: TextField,
        field: WorktreeField,
    },
    Finish {
        worktree: SidebarWorktree,
        finish: WorktreeFinish,
    },
}

#[derive(Debug)]
pub(crate) struct WorktreePrompt {
    pub(crate) project: PathBuf,
    pub(crate) kind: WorktreePromptKind,
    pub(crate) working: bool,
    pub(crate) error: Option<String>,
    /// Matched by the operation's completion, so a result that lands after the
    /// dialog was dismissed or replaced does nothing.
    generation: u64,
}

impl WorktreePrompt {
    /// The field typed into, for the overlay clipboard's paste routing.
    pub(crate) fn text_field_mut(&mut self) -> Option<&mut TextField> {
        if self.working {
            return None;
        }
        match &mut self.kind {
            WorktreePromptKind::Create {
                name,
                copy_paths,
                field,
            } => Some(match field {
                WorktreeField::Name => name,
                WorktreeField::CopyPaths => copy_paths,
            }),
            WorktreePromptKind::Finish { .. } => None,
        }
    }
}

/// The copy list as typed: comma-separated paths relative to the project,
/// blanks dropped. `zwt` validates each one before anything is created.
pub(crate) fn parse_copy_paths(text: &str) -> Vec<PathBuf> {
    text.split(',')
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// How the sidebar labels a worktree: its `wt/NAME` name, else its branch,
/// else its directory.
pub(crate) fn sidebar_worktree(worktree: zwt::LinkedWorktree) -> SidebarWorktree {
    let managed = worktree.is_managed();
    let label = worktree
        .name
        .clone()
        .or_else(|| worktree.branch.clone())
        .unwrap_or_else(|| project_display_name(&worktree.path).to_owned());
    SidebarWorktree {
        path: worktree.path,
        label,
        branch: worktree.branch,
        source_branch: worktree.source_branch.filter(|_| managed),
    }
}

/// The sentence a finished operation is reported with.
fn finished_message(finish: WorktreeFinish, finished: &zwt::FinishedWorktree) -> String {
    match finish {
        WorktreeFinish::Integrate => format!(
            "Integrated {} into {} and removed its worktree.",
            finished.branch, finished.source_branch
        ),
        WorktreeFinish::Discard => format!(
            "Discarded {} and removed its worktree; {} is unchanged.",
            finished.branch, finished.source_branch
        ),
    }
}

impl Zetta {
    /// Lists `root`'s worktrees on the background executor.
    pub(crate) fn refresh_project_worktrees(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        let state = self.sidebar.worktrees.entry(root.clone()).or_default();
        state.generation = state.generation.wrapping_add(1);
        state.loading = true;
        let generation = state.generation;
        cx.spawn(async move |this, cx| {
            let directory = root.clone();
            let listing = cx
                .background_spawn(async move { zwt::list_worktrees(&directory) })
                .await;
            this.update(cx, |this, cx| {
                let Some(state) = this.sidebar.worktrees.get_mut(&root) else {
                    return;
                };
                if state.generation != generation {
                    return;
                }
                *state = match listing {
                    Ok(listing) => ProjectWorktrees {
                        generation,
                        loading: false,
                        worktrees: listing.linked.into_iter().map(sidebar_worktree).collect(),
                        current_branch: listing.current_branch,
                        worktree_root: listing.worktree_root,
                        unavailable: None,
                    },
                    Err(error) => ProjectWorktrees {
                        generation,
                        unavailable: Some(format!("{error:#}")),
                        ..ProjectWorktrees::default()
                    },
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn new_worktree(
        &mut self,
        _: &NewWorktree,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self
            .tab_groups_enabled()
            .then(|| {
                self.active_tab_group()
                    .project_root()
                    .map(Path::to_path_buf)
            })
            .flatten()
        else {
            self.show_notice(
                "Switch to a project's tabs in the sidebar to create one of its worktrees.",
                cx,
            );
            return;
        };
        self.prompt_new_worktree(project, window, cx);
    }

    fn open_worktree_prompt(
        &mut self,
        project: PathBuf,
        kind: WorktreePromptKind,
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
        let generation = self
            .worktree_prompt
            .as_ref()
            .map_or(0, |prompt| prompt.generation)
            .wrapping_add(1);
        self.worktree_prompt = Some(WorktreePrompt {
            project,
            kind,
            working: false,
            error: None,
            generation,
        });
        self.worktree_prompt_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn prompt_new_worktree(
        &mut self,
        project: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.sidebar.worktrees.contains_key(&project) {
            self.refresh_project_worktrees(project.clone(), cx);
        }
        self.open_worktree_prompt(
            project,
            WorktreePromptKind::Create {
                name: TextField::default(),
                copy_paths: TextField::default(),
                field: WorktreeField::Name,
            },
            window,
            cx,
        );
    }

    pub(crate) fn prompt_finish_worktree(
        &mut self,
        project: PathBuf,
        worktree: SidebarWorktree,
        finish: WorktreeFinish,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_worktree_prompt(
            project,
            WorktreePromptKind::Finish { worktree, finish },
            window,
            cx,
        );
    }

    pub(crate) fn dismiss_worktree_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.worktree_prompt.take().is_some() {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    /// Rebases a worktree onto its source branch's tip. No dialog: `sync`
    /// keeps uncommitted work with an autostash and stops on a conflict
    /// without losing anything, so there is nothing to confirm.
    pub(crate) fn sync_sidebar_worktree(
        &mut self,
        project: PathBuf,
        worktree: SidebarWorktree,
        cx: &mut Context<Self>,
    ) {
        let path = worktree.path.clone();
        self.show_notice(format!("Synchronizing {}…", worktree.label), cx);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { zwt::sync_worktree(&path) })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(synced) => this.show_notice(synced.to_string(), cx),
                    Err(error) => this.show_error_notice(
                        format!("Could not synchronize {}: {error:#}", worktree.label),
                        cx,
                    ),
                }
                this.refresh_project_worktrees(project, cx);
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn submit_worktree_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.worktree_prompt.as_mut() else {
            return;
        };
        if prompt.working {
            return;
        }
        match &prompt.kind {
            WorktreePromptKind::Create {
                name, copy_paths, ..
            } => {
                let name = name.text.trim().to_owned();
                if name.is_empty() {
                    prompt.error = Some("Name the worktree".to_owned());
                    cx.notify();
                    return;
                }
                let copy_paths = parse_copy_paths(&copy_paths.text);
                self.create_worktree_from_prompt(name, copy_paths, window, cx);
            }
            WorktreePromptKind::Finish {
                worktree, finish, ..
            } => {
                let (worktree, finish) = (worktree.clone(), *finish);
                self.finish_worktree_from_prompt(worktree, finish, cx);
            }
        }
    }

    fn create_worktree_from_prompt(
        &mut self,
        name: String,
        copy_paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(prompt) = self.worktree_prompt.as_mut() else {
            return;
        };
        prompt.working = true;
        prompt.error = None;
        let generation = prompt.generation;
        let project = prompt.project.clone();
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let directory = project.clone();
            let result = cx
                .background_spawn(
                    async move { zwt::create_worktree(&directory, &name, &copy_paths) },
                )
                .await;
            this.update_in(cx, |this, window, cx| {
                this.refresh_project_worktrees(project.clone(), cx);
                let Some(prompt) = this
                    .worktree_prompt
                    .as_mut()
                    .filter(|prompt| prompt.generation == generation)
                else {
                    return;
                };
                match result {
                    Ok(created) => {
                        this.worktree_prompt = None;
                        this.sidebar.collapsed.remove(&project);
                        this.show_notice(
                            format!(
                                "Created {} on {} from {}.",
                                created.name, created.branch, created.source_branch
                            ),
                            cx,
                        );
                        this.activate_tab_group(
                            TabGroup::Worktree {
                                project,
                                root: created.path,
                            },
                            window,
                            cx,
                        );
                    }
                    Err(error) => {
                        prompt.working = false;
                        prompt.error = Some(format!("{error:#}"));
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    fn finish_worktree_from_prompt(
        &mut self,
        worktree: SidebarWorktree,
        finish: WorktreeFinish,
        cx: &mut Context<Self>,
    ) {
        let Some(prompt) = self.worktree_prompt.as_mut() else {
            return;
        };
        prompt.working = true;
        prompt.error = None;
        let generation = prompt.generation;
        let project = prompt.project.clone();
        cx.notify();
        let path = worktree.path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    match finish {
                        WorktreeFinish::Integrate => zwt::integrate_worktree(&path),
                        WorktreeFinish::Discard => zwt::discard_worktree(&path),
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.refresh_project_worktrees(project.clone(), cx);
                let current = this
                    .worktree_prompt
                    .as_ref()
                    .is_some_and(|prompt| prompt.generation == generation);
                match result {
                    Ok(finished) => {
                        // The worktree is gone whether or not the dialog is
                        // still open, so its tabs move regardless.
                        this.regroup_tabs(
                            &TabGroup::Worktree {
                                project: project.clone(),
                                root: worktree.path.clone(),
                            },
                            TabGroup::Project(project),
                            cx,
                        );
                        if current {
                            this.worktree_prompt = None;
                        }
                        this.show_notice(finished_message(finish, &finished), cx);
                    }
                    Err(error) => {
                        if let Some(prompt) = this.worktree_prompt.as_mut().filter(|_| current) {
                            prompt.working = false;
                            prompt.error = Some(format!("{error:#}"));
                            cx.notify();
                        } else {
                            this.show_error_notice(
                                format!("Could not finish {}: {error:#}", worktree.label),
                                cx,
                            );
                        }
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn worktree_prompt_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.worktree_prompt.is_none() {
            return false;
        }
        match event.keystroke.key.as_str() {
            "escape" => {
                self.dismiss_worktree_prompt(window, cx);
                return true;
            }
            "enter" => {
                self.submit_worktree_prompt(window, cx);
                return true;
            }
            _ => {}
        }
        let Some(prompt) = self.worktree_prompt.as_mut() else {
            return true;
        };
        if prompt.working {
            return true;
        }
        if let WorktreePromptKind::Create { field, .. } = &mut prompt.kind
            && event.keystroke.key == "tab"
        {
            *field = field.adjacent();
            cx.notify();
            return true;
        }
        let Some(text) = prompt.text_field_mut() else {
            return true;
        };
        let edited = match apply_clipboard_shortcut(text, &event.keystroke, cx) {
            ClipboardOutcome::Ignored => apply_text_field_key(text, &event.keystroke),
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

    pub(crate) fn render_worktree_prompt_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let prompt = self.worktree_prompt.as_ref()?;
        let theme = self.window_theme(cx);
        let colors = theme.colors().clone();
        let error_color = theme.status().error;
        let handle = cx.entity().downgrade();
        let project_name = project_display_name(&prompt.project).to_owned();
        let listing = self.sidebar.worktrees.get(&prompt.project);
        let panel = match &prompt.kind {
            WorktreePromptKind::Create {
                name,
                copy_paths,
                field,
            } => create_worktree_panel(CreateWorktreePanel {
                project_name: &project_name,
                name,
                copy_paths,
                field: *field,
                listing,
                colors: &colors,
                handle: &handle,
            }),
            WorktreePromptKind::Finish { worktree, finish } => {
                // Counted per render rather than when the dialog opened: a tab
                // still spawning then, or opened or closed since, is counted.
                let group = TabGroup::Worktree {
                    project: prompt.project.clone(),
                    root: worktree.path.clone(),
                };
                let open_tabs = self.tabs.iter().filter(|tab| tab.group == group).count();
                finish_worktree_panel(&project_name, worktree, *finish, open_tabs, &colors)
            }
        }
        .when_some(prompt.error.clone(), |panel, error| {
            panel.child(crate::ui_messages::error_message(error, error_color))
        })
        .child(worktree_prompt_buttons(prompt, &colors, &handle));
        Some(modal(
            modal_backdrop(
                "worktree-prompt-overlay",
                Placement::Centered,
                BackdropClick::Swallow,
            )
            .track_focus(&self.worktree_prompt_focus),
            panel,
        ))
    }
}

/// What the create dialog draws from.
struct CreateWorktreePanel<'a> {
    project_name: &'a str,
    name: &'a TextField,
    copy_paths: &'a TextField,
    field: WorktreeField,
    listing: Option<&'a ProjectWorktrees>,
    colors: &'a ThemeColors,
    handle: &'a WeakEntity<Zetta>,
}

fn create_worktree_panel(panel: CreateWorktreePanel<'_>) -> gpui::Stateful<gpui::Div> {
    let CreateWorktreePanel {
        project_name,
        name,
        copy_paths,
        field,
        listing,
        colors,
        handle,
    } = panel;
    let trimmed = name.text.trim();
    let destination = match (
        listing.and_then(|listing| listing.worktree_root.as_ref()),
        trimmed,
    ) {
        (Some(root), "") => format!("Created under {}", root.display()),
        (Some(root), name) => format!("Created at {}", root.join(name).display()),
        (None, _) => "Created beside the repository".to_owned(),
    };
    let source = listing
        .and_then(|listing| listing.current_branch.as_deref())
        .map_or_else(
            || "the project's current branch".to_owned(),
            |branch| branch.to_owned(),
        );
    let branch = if trimmed.is_empty() {
        "wt/NAME".to_owned()
    } else {
        format!("wt/{trimmed}")
    };
    let field_row = |label: &'static str,
                     text: &TextField,
                     this_field: WorktreeField,
                     placeholder: &'static str| {
        let click_handle = handle.clone();
        v_flex()
            .gap_1()
            .child(
                Label::new(label)
                    .size(LabelSize::Small)
                    .color(Color::Custom(colors.text_muted)),
            )
            .child(
                boxed_text_field(
                    ("worktree-field", this_field as usize),
                    text,
                    field == this_field,
                    Some(placeholder.into()),
                    FieldMask::Plain,
                    colors,
                )
                .w_full()
                .cursor_text()
                .on_click(move |_, _, cx| {
                    click_handle
                        .update(cx, |this, cx| {
                            if let Some(WorktreePrompt {
                                kind: WorktreePromptKind::Create { field, .. },
                                ..
                            }) = this.worktree_prompt.as_mut()
                            {
                                *field = this_field;
                                cx.notify();
                            }
                        })
                        .ok();
                }),
            )
    };
    dialog_panel("worktree-prompt", DIALOG_WIDTH_MEDIUM, colors)
        .child(dialog_title(
            format!("New worktree in {project_name}"),
            colors,
        ))
        .child(field_row("Name", name, WorktreeField::Name, "feature/name"))
        .child(field_row(
            "Copy into the worktree (optional)",
            copy_paths,
            WorktreeField::CopyPaths,
            ".env, local-settings/",
        ))
        .child(
            div()
                .text_sm()
                .text_color(colors.text_muted)
                .child(format!("Branch {branch} from {source}. {destination}.")),
        )
        .child(hint_line(
            key_hints(&[
                ("Tab", "next field"),
                ("Enter", "create"),
                ("Esc", "cancel"),
            ]),
            colors,
        ))
}

fn finish_worktree_panel(
    project_name: &str,
    worktree: &SidebarWorktree,
    finish: WorktreeFinish,
    open_tabs: usize,
    colors: &ThemeColors,
) -> gpui::Stateful<gpui::Div> {
    let source = worktree.source_branch.as_deref().unwrap_or("its source");
    let branch = worktree.branch.as_deref().unwrap_or(&worktree.label);
    let (title, body) = match finish {
        WorktreeFinish::Integrate => (
            format!("Integrate {}?", worktree.label),
            format!(
                "Rebases {branch} onto {source}, fast-forwards {source} to it, and removes the \
                 worktree and its branch. Both worktrees must be clean; on a conflict, resolve \
                 it in the worktree and integrate again."
            ),
        ),
        WorktreeFinish::Discard => (
            format!("Discard {}?", worktree.label),
            format!(
                "Removes the worktree and deletes {branch}, discarding every change in it — \
                 committed or not. {source} is left as it is."
            ),
        ),
    };
    let tabs = match open_tabs {
        0 => None,
        1 => Some(format!(
            "The tab open in it keeps running and moves to {project_name}."
        )),
        count => Some(format!(
            "The {count} tabs open in it keep running and move to {project_name}."
        )),
    };
    dialog_panel("worktree-prompt", DIALOG_WIDTH_SMALL, colors)
        .child(dialog_title(title, colors))
        .child(div().text_sm().text_color(colors.text_muted).child(body))
        .when_some(tabs, |panel, tabs| {
            panel.child(div().text_sm().text_color(colors.text_muted).child(tabs))
        })
        .child(hint_line(
            key_hints(&[
                (
                    "Enter",
                    match finish {
                        WorktreeFinish::Integrate => "integrate",
                        WorktreeFinish::Discard => "discard",
                    },
                ),
                ("Esc", "cancel"),
            ]),
            colors,
        ))
}

fn worktree_prompt_buttons(
    prompt: &WorktreePrompt,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> gpui::Div {
    let (label, role, tooltip) = match &prompt.kind {
        WorktreePromptKind::Create { .. } => ("Create", ButtonRole::Primary, "Create the worktree"),
        WorktreePromptKind::Finish {
            finish: WorktreeFinish::Integrate,
            ..
        } => (
            "Integrate",
            ButtonRole::Primary,
            "Integrate the worktree into its source branch",
        ),
        WorktreePromptKind::Finish {
            finish: WorktreeFinish::Discard,
            ..
        } => (
            "Discard",
            ButtonRole::Destructive,
            "Discard the worktree and its branch",
        ),
    };
    let cancel_handle = handle.clone();
    let confirm_handle = handle.clone();
    dialog_buttons()
        .child(
            DialogButton::new("cancel-worktree-prompt", "Cancel", ButtonRole::Secondary)
                .key_tooltip("Cancel", SurfaceKey::Escape)
                .render(colors, move |_, window, cx| {
                    cancel_handle
                        .update(cx, |this, cx| this.dismiss_worktree_prompt(window, cx))
                        .ok();
                }),
        )
        .child(
            DialogButton::new("confirm-worktree-prompt", label, role)
                .loading(prompt.working)
                .enabled(!prompt.working)
                .key_tooltip(tooltip, SurfaceKey::Enter)
                .render(colors, move |_, window, cx| {
                    confirm_handle
                        .update(cx, |this, cx| this.submit_worktree_prompt(window, cx))
                        .ok();
                }),
        )
}

#[cfg(test)]
#[path = "tests/worktree_ui.rs"]
mod tests;
