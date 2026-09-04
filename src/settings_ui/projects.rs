use super::*;
use crate::project::{
    ProjectConfig, ProjectRegistry, canonical_project_root, ensure_project_config, paths_equal,
    resolve_registered_project,
};
use crate::project_form::{
    self, PROJECT_INHERIT_LABEL, ProjectCommandForm, ProjectEnvironmentForm, ProjectForm,
    ProjectProfileForm, ProjectTabIcon, ProjectTextField,
};

/// The Projects tab's configuration builder for one registered project.
#[derive(Clone)]
pub(crate) struct ProjectEditor {
    /// Registered project identity. Managed worktrees keep the main repository
    /// here so the builder still belongs to the row that opened it.
    pub(crate) root: PathBuf,
    /// The configuration root currently being edited. This can be a managed
    /// worktree when the active pane is inside one with its own project file.
    pub(crate) config_root: PathBuf,
    /// Position in `SettingsEditor::project_roots`, so closing the builder can
    /// return focus to the row it was opened from.
    pub(crate) index: usize,
    pub(crate) form: ProjectForm,
    pub(crate) dirty: bool,
    pub(crate) save_in_progress: bool,
}

/// The open project builder, but only while the Projects page is the visible
/// surface.
///
/// The builder hosts the same pane-template editor as the Templates page, and
/// that editor resolves which form it edits through here. Gating on the page as
/// well as on `project` is what lets the Templates page keep editing the user
/// configuration while a project builder stays open behind it.
pub(crate) fn project_editor(editor: &SettingsEditor) -> Option<&ProjectEditor> {
    editor
        .project
        .as_ref()
        .filter(|_| editor.page == SettingsPage::Projects)
}

pub(crate) fn editing_project(editor: &SettingsEditor) -> bool {
    editor.page == SettingsPage::Projects && editor.project.is_some()
}

/// Where focus lands when the builder closes: the row it was opened from, or
/// the tab itself when that row is gone (the project was unregistered while the
/// builder was open).
fn project_row_control(editor: &SettingsEditor, closed: Option<&ProjectEditor>) -> SettingsControl {
    match closed.filter(|project| {
        editor
            .project_roots
            .get(project.index)
            .is_some_and(|root| *root == project.root)
    }) {
        Some(project) => SettingsControl::EditProject(project.index),
        None => SettingsControl::Tab(SettingsPage::Projects),
    }
}

pub(crate) fn mark_project_dirty(editor: &mut SettingsEditor) {
    if let Some(project) = editor.project.as_mut() {
        project.dirty = true;
    }
    editor.message = None;
}

pub(crate) fn project_controls(editor: &SettingsEditor) -> Vec<SettingsControl> {
    let Some(project) = project_editor(editor) else {
        let mut controls = vec![SettingsControl::AddProject];
        for index in 0..editor.project_roots.len() {
            controls.extend([
                SettingsControl::OpenProject(index),
                SettingsControl::EditProject(index),
                SettingsControl::RemoveProject(index),
            ]);
        }
        return controls;
    };
    let form = &project.form;
    let mut controls = vec![
        SettingsControl::CloseProjectConfig,
        SettingsControl::SaveProjectConfig,
        SettingsControl::OpenProjectConfigFile,
        SettingsControl::Dropdown(SettingsDropdown::ProjectTheme),
        SettingsControl::Dropdown(SettingsDropdown::ProjectDarkTheme),
        SettingsControl::Input(SettingsInput::Project(ProjectTextField::WorkingDirectory)),
        SettingsControl::Dropdown(SettingsDropdown::ProjectDefaultProfile),
        SettingsControl::ProjectTabIconPicker,
        SettingsControl::ClearProjectTabIcon,
        SettingsControl::Toggle(SettingsToggle::ProjectOpacityOverride),
    ];
    if form.inactive_pane_opacity.is_some() {
        controls.push(SettingsControl::ProjectOpacity);
    }
    for index in 0..form.environment.len() {
        controls.extend([
            SettingsControl::Input(SettingsInput::Project(ProjectTextField::EnvironmentName(
                index,
            ))),
            SettingsControl::Input(SettingsInput::Project(ProjectTextField::EnvironmentValue(
                index,
            ))),
            SettingsControl::RemoveProjectEnvironment(index),
        ]);
    }
    controls.push(SettingsControl::AddProjectEnvironment);
    for (command_index, command) in form.commands.iter().enumerate() {
        controls.extend([
            SettingsControl::Input(SettingsInput::Project(ProjectTextField::CommandName(
                command_index,
            ))),
            SettingsControl::RemoveProjectCommand(command_index),
            SettingsControl::Input(SettingsInput::Project(ProjectTextField::Command(
                command_index,
            ))),
        ]);
        for (environment_index, _) in command.environment.iter().enumerate() {
            controls.extend([
                SettingsControl::Input(SettingsInput::Project(
                    ProjectTextField::CommandEnvironmentName(command_index, environment_index),
                )),
                SettingsControl::Input(SettingsInput::Project(
                    ProjectTextField::CommandEnvironmentValue(command_index, environment_index),
                )),
                SettingsControl::RemoveProjectCommandEnvironment(command_index, environment_index),
            ]);
        }
        controls.push(SettingsControl::AddProjectCommandEnvironment(command_index));
    }
    controls.push(SettingsControl::AddProjectCommand);
    for index in 0..form.profiles.len() {
        controls.extend(project_profile_controls(index));
    }
    controls.push(SettingsControl::AddProjectProfile);
    controls.push(SettingsControl::Dropdown(
        SettingsDropdown::ProjectInitialSplit,
    ));
    controls.extend(pane_templates::pane_template_controls(editor));
    controls
}

pub(crate) fn project_dropdown_options(
    editor: &SettingsEditor,
    dropdown: SettingsDropdown,
) -> (String, Arc<[String]>) {
    let Some(project) = project_editor(editor) else {
        return (String::new(), Arc::from([]));
    };
    let form = &project.form;
    let inherit = || PROJECT_INHERIT_LABEL.to_owned();
    match dropdown {
        SettingsDropdown::ProjectTheme => (
            form.theme.clone().unwrap_or_else(inherit),
            std::iter::once(inherit())
                .chain(editor.themes.iter().cloned())
                .collect(),
        ),
        SettingsDropdown::ProjectDarkTheme => (
            form.dark_theme.clone().unwrap_or_else(inherit),
            std::iter::once(inherit())
                .chain(editor.themes.iter().cloned())
                .collect(),
        ),
        SettingsDropdown::ProjectDefaultProfile => (
            form.default_profile.clone().unwrap_or_else(inherit),
            std::iter::once(inherit())
                .chain(form.profile_options())
                .collect(),
        ),
        SettingsDropdown::ProjectInitialSplit => (
            form.initial_split
                .clone()
                .unwrap_or_else(|| "None".to_owned()),
            std::iter::once("None".to_owned())
                .chain(form.template_names())
                .collect(),
        ),
        SettingsDropdown::ProjectProfileTheme(index) => (
            form.profiles
                .get(index)
                .and_then(|profile| profile.theme.clone())
                .unwrap_or_else(inherit),
            std::iter::once(inherit())
                .chain(editor.themes.iter().cloned())
                .collect(),
        ),
        SettingsDropdown::ProjectProfileDarkTheme(index) => (
            form.profiles
                .get(index)
                .and_then(|profile| profile.dark_theme.clone())
                .unwrap_or_else(inherit),
            std::iter::once(inherit())
                .chain(editor.themes.iter().cloned())
                .collect(),
        ),
        SettingsDropdown::ProjectProfileIcon(index) => (
            form.profiles
                .get(index)
                .and_then(|profile| profile.icon.as_ref())
                .map_or("Automatic", ProfileIcon::label)
                .to_owned(),
            Arc::from(["Automatic", "Zetta", "Bash", "Zsh", "Fish"].map(str::to_owned)),
        ),
        _ => (String::new(), Arc::from([])),
    }
}

pub(crate) fn set_project_dropdown(
    editor: &mut SettingsEditor,
    dropdown: SettingsDropdown,
    value: &str,
) -> bool {
    let Some(project) = editor.project.as_mut() else {
        return false;
    };
    let form = &mut project.form;
    let optional = |value: &str| (value != PROJECT_INHERIT_LABEL).then(|| value.to_owned());
    match dropdown {
        SettingsDropdown::ProjectTheme => form.theme = optional(value),
        SettingsDropdown::ProjectDarkTheme => form.dark_theme = optional(value),
        SettingsDropdown::ProjectDefaultProfile => form.default_profile = optional(value),
        SettingsDropdown::ProjectInitialSplit => {
            form.initial_split = (value != "None").then(|| value.to_owned());
        }
        SettingsDropdown::ProjectProfileTheme(index) => {
            let Some(profile) = form.profiles.get_mut(index) else {
                return false;
            };
            profile.theme = optional(value);
        }
        SettingsDropdown::ProjectProfileDarkTheme(index) => {
            let Some(profile) = form.profiles.get_mut(index) else {
                return false;
            };
            profile.dark_theme = optional(value);
        }
        SettingsDropdown::ProjectProfileIcon(index) => {
            let Some(profile) = form.profiles.get_mut(index) else {
                return false;
            };
            profile.icon = (value != "Automatic")
                .then(|| {
                    ProfileIcon::parse_name(&value.to_ascii_lowercase())
                        .ok()
                        .flatten()
                })
                .flatten();
        }
        _ => return false,
    }
    mark_project_dirty(editor);
    invalidate_controls_cache(editor);
    true
}

/// Where the project picker opens: the directory the active pane reported, so a
/// shell already sitting in the project root needs no navigation, and Zetta's own
/// working directory when the pane reported none.
pub(crate) fn prompt_start_directory(pane_directory: Option<PathBuf>) -> Option<PathBuf> {
    pane_directory.or_else(|| env::current_dir().ok())
}

fn settings_project_config_root(
    registered_root: &Path,
    current_directory: Option<&Path>,
    registry: &ProjectRegistry,
) -> PathBuf {
    let Some(current_directory) = current_directory else {
        return registered_root.to_path_buf();
    };
    let resolution = resolve_registered_project(current_directory, registry);
    resolution
        .root
        .as_ref()
        .filter(|root| paths_equal(root, registered_root))
        .and(resolution.config_root)
        .unwrap_or_else(|| registered_root.to_path_buf())
}

impl Zetta {
    fn refresh_settings_project_roots(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = self.settings_editor.as_mut() {
            editor.project_roots = self.projects.registry.roots().to_vec().into();
            editor.message = None;
            editor.focused_control = Some(SettingsControl::Tab(SettingsPage::Projects));
            invalidate_controls_cache(editor);
        }
        cx.notify();
    }

    /// The directory the project picker opens at, taken from the active pane. A
    /// WSL pane reports a path inside the distribution, which the host picker
    /// cannot open, so it counts as having reported nothing.
    fn project_prompt_directory(&self, cx: &App) -> Option<PathBuf> {
        prompt_start_directory(
            self.tabs
                .get(self.active_tab)
                .and_then(Tab::active_pane)
                .filter(|pane| !is_wsl_shell(&pane.profile.command))
                .and_then(|pane| pane.working_directory(cx)),
        )
    }

    pub(crate) fn add_project_from_settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selection = gpui_platform::prompt_for_paths_in(
            cx,
            self.project_prompt_directory(cx),
            PathPromptOptions {
                files: false,
                directories: true,
                multiple: false,
                prompt: Some("Select a Zetta project root".into()),
            },
        );
        let base = self.launch_config.clone();
        let registry_path = self.projects.registry.path().to_path_buf();
        let executor = cx.background_executor().clone();
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let Some(root) = selection
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .flatten()
                    .and_then(|mut paths| paths.pop())
                else {
                    return;
                };
                let result = executor
                    .spawn(async move {
                        let root = canonical_project_root(&root)?;
                        let mut registry = ProjectRegistry::load_from(registry_path)?;
                        let resolution = resolve_registered_project(&root, &registry);
                        let root = if resolution.managed_worktree.is_some() {
                            resolution
                                .root
                                .context("managed worktree has no registered main project")?
                        } else {
                            root
                        };
                        ensure_project_config(&root)?;
                        let config = ProjectConfig::load(&root, &base)?;
                        if registry.add(&root)? {
                            registry.save()?;
                        }
                        Ok::<_, anyhow::Error>((registry, config))
                    })
                    .await;
                this.update_in(cx, |this, window, cx| match result {
                    Ok((registry, config)) => {
                        this.projects.registry = registry;
                        this.projects.insert_config(config);
                        this.refresh_settings_project_roots(cx);
                        this.reschedule_project_detection(window, cx);
                        reload_projects_in_other_windows(window.window_handle().window_id(), cx);
                    }
                    Err(error) => {
                        if let Some(editor) = this.settings_editor.as_mut() {
                            editor.message =
                                Some((true, format!("Could not add project: {error:#}")));
                        }
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
    }

    pub(crate) fn open_project_from_settings(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self
            .settings_editor
            .as_ref()
            .and_then(|editor| editor.project_roots.get(index))
            .cloned()
        else {
            return;
        };
        self.dismiss_settings(window, cx);
        self.open_project_tab(root, window, cx);
    }

    /// Opens the typed builder for a project's `.zetta/config.json`. The file is
    /// read and resolved against the user configuration on the background
    /// executor, because both are I/O and a full configuration parse.
    pub(crate) fn edit_project_from_settings(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_ref() else {
            return;
        };
        if editor.project_loading {
            return;
        }
        let Some(root) = editor.project_roots.get(index).cloned() else {
            return;
        };
        let config_root = settings_project_config_root(
            &root,
            self.project_prompt_directory(cx).as_deref(),
            &self.projects.registry,
        );
        if let Some(editor) = self.settings_editor.as_mut() {
            editor.project_loading = true;
            editor.message = Some((false, "Loading the project configuration…".to_owned()));
            invalidate_controls_cache(editor);
        }
        let base = self.launch_config.clone();
        let executor = cx.background_executor().clone();
        let this = cx.entity().downgrade();
        let load_root = config_root.clone();
        window
            .spawn(cx, async move |cx| {
                let loaded = executor
                    .spawn(async move { ProjectForm::load(&load_root, &base) })
                    .await;
                this.update_in(cx, |this, window, cx| {
                    let Some(editor) = this.settings_editor.as_mut() else {
                        return;
                    };
                    editor.project_loading = false;
                    match loaded {
                        Ok(form) => {
                            editor.project = Some(ProjectEditor {
                                root,
                                config_root,
                                index,
                                form,
                                dirty: false,
                                save_in_progress: false,
                            });
                            editor.page = SettingsPage::Projects;
                            // The builder replaces the whole page, so a scroll
                            // offset carried over from the list would open it
                            // part-way down.
                            editor.settings_scroll.set_offset(Point::default());
                            editor.message = None;
                            editor.pane_template_validation_error = None;
                            editor.focused_input = None;
                            editor.focused_control = Some(SettingsControl::CloseProjectConfig);
                            invalidate_controls_cache(editor);
                            this.settings_focus.focus(window, cx);
                        }
                        Err(error) => {
                            editor.message = Some((
                                true,
                                format!("Could not open the project configuration: {error:#}"),
                            ));
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        cx.notify();
    }

    /// The raw-JSON escape hatch: opens the project's configuration file in the
    /// same editor flow the application configuration uses, for anything the
    /// builder deliberately does not model.
    pub(crate) fn open_project_config_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self
            .settings_editor
            .as_ref()
            .and_then(project_editor)
            .map(|project| ProjectConfig::path_for(&project.config_root))
        else {
            return;
        };
        self.dismiss_settings(window, cx);
        self.edit_settings_file_in_active_pane(path, window, cx);
    }

    pub(crate) fn close_project_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.settings_editor.as_mut() {
            let closed = editor.project.take();
            let discarded = closed
                .as_ref()
                .is_some_and(|project| project.dirty && !project.save_in_progress);
            editor.clear_dropdown();
            editor.pane_template_validation_error = None;
            editor.settings_scroll.set_offset(Point::default());
            editor.focused_input = None;
            editor.focused_control = Some(project_row_control(editor, closed.as_ref()));
            editor.message = discarded.then(|| {
                (
                    false,
                    "Closed the project configuration without saving.".to_owned(),
                )
            });
            invalidate_controls_cache(editor);
        }
        self.settings_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn save_project_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self
            .settings_editor
            .as_ref()
            .and_then(project_editor)
            .filter(|project| !project.save_in_progress)
        else {
            return;
        };
        if !project.dirty {
            self.close_project_config(window, cx);
            return;
        }
        let form = project.form.clone();
        let config_root = project.config_root.clone();
        if let Some(editor) = self.settings_editor.as_mut() {
            if let Some(project) = editor.project.as_mut() {
                project.save_in_progress = true;
            }
            editor.message = Some((false, "Saving the project configuration…".to_owned()));
        }
        let base = self.launch_config.clone();
        let executor = cx.background_executor().clone();
        let this = cx.entity().downgrade();
        let save_root = config_root.clone();
        window
            .spawn(cx, async move |cx| {
                let result = executor
                    .spawn(async move {
                        let text = form.to_json()?;
                        let path = project_form::save(&save_root, &base, &text)?;
                        let config = ProjectConfig::load(&save_root, &base)?;
                        Ok::<_, anyhow::Error>((path, config))
                    })
                    .await;
                this.update_in(cx, |this, window, cx| {
                    match result {
                        Ok((path, config)) => {
                            this.projects.insert_config(config);
                            this.projects.invalidate_active_context();
                            if let Some(editor) = this.settings_editor.as_mut() {
                                let saved = editor.project.take();
                                editor.clear_dropdown();
                                editor.pane_template_validation_error = None;
                                editor.focused_input = None;
                                editor.focused_control =
                                    Some(project_row_control(editor, saved.as_ref()));
                                editor.message = Some((false, format!("Saved {}", path.display())));
                                invalidate_controls_cache(editor);
                            }
                            this.activate_current_project(window, cx);
                            this.reschedule_project_detection(window, cx);
                            reload_projects_in_other_windows(
                                window.window_handle().window_id(),
                                cx,
                            );
                            this.settings_focus.focus(window, cx);
                        }
                        Err(error) => {
                            if let Some(editor) = this.settings_editor.as_mut() {
                                if let Some(project) = editor.project.as_mut() {
                                    project.save_in_progress = false;
                                }
                                editor.message = Some((true, format!("Not saved: {error:#}")));
                            }
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        cx.notify();
    }

    pub(crate) fn remove_project_from_settings(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self
            .settings_editor
            .as_ref()
            .and_then(|editor| editor.project_roots.get(index))
            .cloned()
        else {
            return;
        };
        let registry_path = self.projects.registry.path().to_path_buf();
        let executor = cx.background_executor().clone();
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let removed_root = root.clone();
                let result = executor
                    .spawn(async move {
                        let mut registry = ProjectRegistry::load_from(registry_path)?;
                        registry.remove(&removed_root).with_context(|| {
                            format!("{} is not a registered project", removed_root.display())
                        })?;
                        registry.save()?;
                        Ok::<_, anyhow::Error>(registry)
                    })
                    .await;
                this.update_in(cx, |this, window, cx| match result {
                    Ok(registry) => {
                        this.projects.registry = registry;
                        this.projects.suppress_offer_for(&root);
                        this.projects.invalidate_active_context();
                        this.projects.clear_removed_roots();
                        if let Some(editor) = this.settings_editor.as_mut()
                            && editor
                                .project
                                .as_ref()
                                .is_some_and(|project| project.root == root)
                        {
                            editor.project = None;
                            editor.clear_dropdown();
                        }
                        this.refresh_settings_project_roots(cx);
                        this.activate_current_project(window, cx);
                        this.reschedule_project_detection(window, cx);
                        reload_projects_in_other_windows(window.window_handle().window_id(), cx);
                    }
                    Err(error) => {
                        if let Some(editor) = this.settings_editor.as_mut() {
                            editor.message = Some((
                                true,
                                format!("Could not remove project {}: {error:#}", root.display()),
                            ));
                        }
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
    }

    pub(crate) fn activate_project_config_control(
        &mut self,
        control: SettingsControl,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match control {
            SettingsControl::CloseProjectConfig => {
                self.close_project_config(window, cx);
                return;
            }
            SettingsControl::SaveProjectConfig => {
                self.save_project_config(window, cx);
                return;
            }
            SettingsControl::OpenProjectConfigFile => {
                self.open_project_config_file(window, cx);
                return;
            }
            SettingsControl::ProjectTabIconPicker => {
                self.open_project_tab_icon_picker(window, cx);
                return;
            }
            _ => {}
        }
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        let Some(project) = editor.project.as_mut() else {
            return;
        };
        let form = &mut project.form;
        // Removing a row leaves nothing at its index, so focus moves to the row
        // list's Add button rather than to a control that no longer exists.
        let mut focus = control.clone();
        match control {
            SettingsControl::ClearProjectTabIcon => {
                form.default_tab_icon = ProjectTabIcon::Inherit;
            }
            SettingsControl::AddProjectEnvironment => {
                form.environment.push(ProjectEnvironmentForm {
                    name: TextField::default(),
                    value: TextField::default(),
                });
            }
            SettingsControl::RemoveProjectEnvironment(index) => {
                if index >= form.environment.len() {
                    return;
                }
                form.environment.remove(index);
                focus = SettingsControl::AddProjectEnvironment;
            }
            SettingsControl::AddProjectCommand => {
                form.commands.push(ProjectCommandForm {
                    name: TextField::default(),
                    command: TextField::default(),
                    environment: Vec::new(),
                    object: false,
                });
            }
            SettingsControl::RemoveProjectCommand(index) => {
                if index >= form.commands.len() {
                    return;
                }
                form.commands.remove(index);
                focus = SettingsControl::AddProjectCommand;
            }
            SettingsControl::AddProjectCommandEnvironment(command_index) => {
                let Some(command) = form.commands.get_mut(command_index) else {
                    return;
                };
                command.environment.push(ProjectEnvironmentForm {
                    name: TextField::default(),
                    value: TextField::default(),
                });
            }
            SettingsControl::RemoveProjectCommandEnvironment(command_index, environment_index) => {
                let Some(command) = form.commands.get_mut(command_index) else {
                    return;
                };
                if environment_index >= command.environment.len() {
                    return;
                }
                command.environment.remove(environment_index);
                focus = SettingsControl::AddProjectCommandEnvironment(command_index);
            }
            SettingsControl::AddProjectProfile => {
                form.profiles.push(ProjectProfileForm {
                    name: TextField::default(),
                    program: TextField::default(),
                    arguments: TextField::default(),
                    theme: None,
                    dark_theme: None,
                    icon: None,
                    hidden: false,
                });
            }
            SettingsControl::RemoveProjectProfile(index) => {
                if index >= form.profiles.len() {
                    return;
                }
                form.profiles.remove(index);
                focus = SettingsControl::AddProjectProfile;
            }
            _ => return,
        }
        mark_project_dirty(editor);
        editor.focused_control = Some(focus);
        invalidate_controls_cache(editor);
        cx.notify();
    }
}

#[cfg(test)]
#[path = "../tests/settings_ui/projects.rs"]
mod tests;
