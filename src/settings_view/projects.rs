//! The settings Projects tab: the registered-project list, and the typed
//! builder for one project's `.zetta/config.json`.
//!
//! The builder reuses the pane-template editor from the Templates page rather
//! than reimplementing it; that editor resolves which form it edits through
//! `settings_ui::pane_templates::templates`, which points at the open project
//! while this page is the visible surface.

use super::pane_templates::render_pane_templates_page;
use super::*;
use crate::project::{ProjectConfig, project_display_name};
use crate::project_form::{ProjectTabIcon, ProjectTextField};
use crate::settings_ui::{ProjectEditor, project_editor};

pub(crate) fn render_projects_page(
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
    opacity_slider: &impl Fn(f32, OpacityTarget) -> AnyElement,
) -> AnyElement {
    match project_editor(editor) {
        Some(project) => render_project_config(editor, project, colors, handle, opacity_slider),
        None => render_project_list(editor, colors, handle),
    }
}

fn render_project_list(
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> AnyElement {
    let mut rows = Vec::with_capacity(editor.project_roots.len());
    for (index, root) in editor.project_roots.iter().enumerate() {
        let title = project_display_name(root).to_owned();
        rows.push(
            div()
                .id(format!("settings-project-{index}"))
                .p_3()
                .rounded(px(6.))
                .border_1()
                .border_color(colors.border)
                .bg(colors.editor_background)
                .child(div().text_sm().child(title))
                .child(
                    div()
                        .mt_1()
                        .text_xs()
                        .text_color(colors.text_muted)
                        .child(root.display().to_string()),
                )
                .child(
                    div()
                        .mt_1()
                        .text_xs()
                        .text_color(colors.text_muted)
                        .child(ProjectConfig::path_for(root).display().to_string()),
                )
                .child(
                    h_flex()
                        .mt_3()
                        .gap_2()
                        .child(action_button(
                            editor,
                            format!("settings-open-project-{index}"),
                            "Open".to_owned(),
                            SettingsControl::OpenProject(index),
                            true,
                            colors,
                            handle,
                        ))
                        .child(action_button(
                            editor,
                            format!("settings-edit-project-{index}"),
                            "Edit config".to_owned(),
                            SettingsControl::EditProject(index),
                            !editor.project_loading,
                            colors,
                            handle,
                        ))
                        .child(action_button(
                            editor,
                            format!("settings-remove-project-{index}"),
                            "Remove".to_owned(),
                            SettingsControl::RemoveProject(index),
                            true,
                            colors,
                            handle,
                        )),
                ),
        );
    }

    v_flex()
        .gap_3()
        .child(
            div()
                .text_sm()
                .child("Projects apply .zetta/config.json while an active pane is inside their registered root."),
        )
        .child(
            div()
                .text_xs()
                .text_color(colors.text_muted)
                .child("Edit config opens a builder for the project's theme, working directory, profiles, tab icon, environment, commands, inactive-pane opacity, pane templates, and initial split. Register only trusted projects: templates and registered commands may execute arbitrary shell code."),
        )
        .child(
            h_flex().child(action_button(editor, "settings-add-project".to_owned(), "Add project".to_owned(), SettingsControl::AddProject, true, colors, handle)),
        )
        .when(editor.project_roots.is_empty(), |page| {
            page.child(
                div()
                    .py_4()
                    .text_sm()
                    .text_color(colors.text_muted)
                    .child("No projects are registered."),
            )
        })
        .children(rows)
        .into_any_element()
}

fn section_heading(label: &'static str, description: &'static str, colors: &ThemeColors) -> Div {
    v_flex()
        .mt_4()
        .mb_1()
        .child(div().text_sm().child(label))
        .child(
            div()
                .text_xs()
                .text_color(colors.text_muted)
                .child(description),
        )
}

fn render_project_config(
    editor: &SettingsEditor,
    project: &ProjectEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
    opacity_slider: &impl Fn(f32, OpacityTarget) -> AnyElement,
) -> AnyElement {
    let form = &project.form;
    let saving = project.save_in_progress;
    let actions = h_flex()
        .gap_2()
        .child(action_button(
            editor,
            "project-config-close".to_owned(),
            "Back to projects".to_owned(),
            SettingsControl::CloseProjectConfig,
            !saving,
            colors,
            handle,
        ))
        .child(action_button(
            editor,
            "project-config-save".to_owned(),
            if saving {
                "Saving…".to_owned()
            } else if project.dirty {
                "Save project *".to_owned()
            } else {
                "Save project".to_owned()
            },
            SettingsControl::SaveProjectConfig,
            !saving,
            colors,
            handle,
        ))
        .child(action_button(
            editor,
            "project-config-open-file".to_owned(),
            "Open in editor".to_owned(),
            SettingsControl::OpenProjectConfigFile,
            !saving,
            colors,
            handle,
        ));

    let icon_handle = handle.clone();
    let current_icon = form.default_tab_icon;
    let tab_icon_trigger = h_flex()
        .id("project-tab-icon-picker-trigger")
        .h_9()
        .min_w_0()
        .flex_1()
        .px_3()
        .justify_between()
        .rounded(px(4.))
        .border_1()
        .border_color(
            if editor.focused_control == Some(SettingsControl::ProjectTabIconPicker) {
                colors.border_focused
            } else {
                colors.border
            },
        )
        .bg(colors.editor_background)
        .cursor_pointer()
        .hover(|style| style.bg(colors.element_hover))
        .child(
            h_flex()
                .gap_2()
                .child(Icon::new(current_icon.icon().unwrap_or(IconName::Dash)))
                .child(current_icon.label()),
        )
        .child(
            svg()
                .path(IconName::ChevronDown.path())
                .size(px(14.))
                .text_color(colors.icon_muted),
        )
        .on_click(move |_, window, cx| {
            icon_handle
                .update(cx, |this, cx| {
                    this.open_project_tab_icon_picker(window, cx);
                })
                .ok();
        });

    let mut content: Vec<AnyElement> = vec![
        actions.into_any_element(),
        div()
            .mt_2()
            .text_xs()
            .text_color(colors.text_muted)
            .child("Every field left unset inherits the application configuration.")
            .into_any_element(),
        control_row(
            editor,
            "Light theme",
            &[SettingsControl::Dropdown(SettingsDropdown::ProjectTheme)],
            dropdown_field(
                "project-theme".to_owned(),
                form.theme
                    .clone()
                    .unwrap_or_else(|| crate::project_form::PROJECT_INHERIT_LABEL.to_owned()),
                SettingsDropdown::ProjectTheme,
                editor,
                colors,
                handle,
            ),
            colors,
        ),
        control_row(
            editor,
            "Dark theme",
            &[SettingsControl::Dropdown(
                SettingsDropdown::ProjectDarkTheme,
            )],
            dropdown_field(
                "project-dark-theme".to_owned(),
                form.dark_theme
                    .clone()
                    .unwrap_or_else(|| crate::project_form::PROJECT_INHERIT_LABEL.to_owned()),
                SettingsDropdown::ProjectDarkTheme,
                editor,
                colors,
                handle,
            ),
            colors,
        ),
        control_row(
            editor,
            "Working directory (project-relative; empty means the project root)",
            &[SettingsControl::Input(SettingsInput::Project(
                ProjectTextField::WorkingDirectory,
            ))],
            text_field(
                "project-working-directory".to_owned(),
                form.working_directory.clone(),
                SettingsInput::Project(ProjectTextField::WorkingDirectory),
                editor,
                colors,
                handle,
            ),
            colors,
        ),
        control_row(
            editor,
            "Default profile",
            &[SettingsControl::Dropdown(
                SettingsDropdown::ProjectDefaultProfile,
            )],
            dropdown_field(
                "project-default-profile".to_owned(),
                form.default_profile
                    .clone()
                    .unwrap_or_else(|| crate::project_form::PROJECT_INHERIT_LABEL.to_owned()),
                SettingsDropdown::ProjectDefaultProfile,
                editor,
                colors,
                handle,
            ),
            colors,
        ),
        control_row(
            editor,
            "Default tab icon",
            &[
                SettingsControl::ProjectTabIconPicker,
                SettingsControl::ClearProjectTabIcon,
            ],
            h_flex()
                .gap_2()
                .child(tab_icon_trigger)
                .child(action_button(
                    editor,
                    "project-tab-icon-clear".to_owned(),
                    "Inherit".to_owned(),
                    SettingsControl::ClearProjectTabIcon,
                    !matches!(current_icon, ProjectTabIcon::Inherit),
                    colors,
                    handle,
                ))
                .into_any_element(),
            colors,
        ),
        control_row(
            editor,
            "Override the inactive pane opacity",
            &[SettingsControl::Toggle(
                SettingsToggle::ProjectOpacityOverride,
            )],
            switch(
                "project-inactive-pane-opacity-override",
                form.inactive_pane_opacity.is_some().into(),
            )
            .label(if form.inactive_pane_opacity.is_some() {
                "On"
            } else {
                "Off"
            })
            .full_width(true)
            .aria_label("project-inactive-pane-opacity-override")
            .on_click({
                let toggle_handle = handle.clone();
                move |state, window, cx| {
                    toggle_handle
                        .update(cx, |this, cx| {
                            this.set_settings_toggle(
                                SettingsToggle::ProjectOpacityOverride,
                                state.selected(),
                                window,
                                cx,
                            );
                        })
                        .ok();
                }
            })
            .into_any_element(),
            colors,
        ),
    ];
    if let Some(opacity) = form.inactive_pane_opacity {
        content.push(control_row(
            editor,
            "Inactive pane opacity",
            &[SettingsControl::ProjectOpacity],
            opacity_slider(opacity, OpacityTarget::Project),
            colors,
        ));
    }

    push_project_environment_rows(&mut content, editor, form, colors, handle);
    push_project_command_rows(&mut content, editor, form, colors, handle);
    push_project_profile_rows(&mut content, editor, form, colors, handle);
    push_project_template_rows(&mut content, editor, form, colors, handle);
    v_flex().children(content).into_any_element()
}

/// The environment rows: the variables every terminal started inside the
/// project inherits.
fn push_project_environment_rows(
    content: &mut Vec<AnyElement>,
    editor: &SettingsEditor,
    form: &crate::project_form::ProjectForm,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) {
    content.push(
        section_heading(
            "Environment",
            "Applied to every terminal started inside the project. Template and pane values override matching keys, and reserved ZETTA_* names cannot be replaced.",
            colors,
        )
        .into_any_element(),
    );
    for (index, entry) in form.environment.iter().enumerate() {
        content.push(control_row(
            editor,
            format!("Variable {} · name", index + 1),
            &[
                SettingsControl::Input(SettingsInput::Project(ProjectTextField::EnvironmentName(
                    index,
                ))),
                SettingsControl::RemoveProjectEnvironment(index),
            ],
            h_flex()
                .gap_1()
                .child(text_field(
                    format!("project-env-name-{index}"),
                    entry.name.clone(),
                    SettingsInput::Project(ProjectTextField::EnvironmentName(index)),
                    editor,
                    colors,
                    handle,
                ))
                .child(action_button(
                    editor,
                    format!("project-env-remove-{index}"),
                    "×".to_owned(),
                    SettingsControl::RemoveProjectEnvironment(index),
                    true,
                    colors,
                    handle,
                ))
                .into_any_element(),
            colors,
        ));
        content.push(control_row(
            editor,
            format!("Variable {} · value", index + 1),
            &[SettingsControl::Input(SettingsInput::Project(
                ProjectTextField::EnvironmentValue(index),
            ))],
            text_field(
                format!("project-env-value-{index}"),
                entry.value.clone(),
                SettingsInput::Project(ProjectTextField::EnvironmentValue(index)),
                editor,
                colors,
                handle,
            ),
            colors,
        ));
    }
    content.push(
        h_flex()
            .justify_end()
            .child(action_button(
                editor,
                "project-env-add".to_owned(),
                "Add environment variable".to_owned(),
                SettingsControl::AddProjectEnvironment,
                true,
                colors,
                handle,
            ))
            .into_any_element(),
    );
}

/// The registered-command rows. A command's own environment overrides the
/// project's for that invocation only.
fn push_project_command_rows(
    content: &mut Vec<AnyElement>,
    editor: &SettingsEditor,
    form: &crate::project_form::ProjectForm,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) {
    content.push(
        section_heading(
            "Commands",
            "Registered commands run raw shell code in the active pane. Command environments override project environment values for that invocation and do not persist in the pane.",
            colors,
        )
        .into_any_element(),
    );
    for (command_index, command) in form.commands.iter().enumerate() {
        content.push(control_row(
            editor,
            format!("Command {} · name", command_index + 1),
            &[
                SettingsControl::Input(SettingsInput::Project(ProjectTextField::CommandName(
                    command_index,
                ))),
                SettingsControl::RemoveProjectCommand(command_index),
            ],
            h_flex()
                .gap_1()
                .child(text_field(
                    format!("project-command-name-{command_index}"),
                    command.name.clone(),
                    SettingsInput::Project(ProjectTextField::CommandName(command_index)),
                    editor,
                    colors,
                    handle,
                ))
                .child(action_button(
                    editor,
                    format!("project-command-remove-{command_index}"),
                    "×".to_owned(),
                    SettingsControl::RemoveProjectCommand(command_index),
                    true,
                    colors,
                    handle,
                ))
                .into_any_element(),
            colors,
        ));
        content.push(control_row(
            editor,
            format!("Command {} · shell command", command_index + 1),
            &[SettingsControl::Input(SettingsInput::Project(
                ProjectTextField::Command(command_index),
            ))],
            text_field(
                format!("project-command-command-{command_index}"),
                command.command.clone(),
                SettingsInput::Project(ProjectTextField::Command(command_index)),
                editor,
                colors,
                handle,
            ),
            colors,
        ));
        for (environment_index, entry) in command.environment.iter().enumerate() {
            content.push(control_row(
                editor,
                format!(
                    "Command {} environment {} · name",
                    command_index + 1,
                    environment_index + 1
                ),
                &[
                    SettingsControl::Input(SettingsInput::Project(
                        ProjectTextField::CommandEnvironmentName(command_index, environment_index),
                    )),
                    SettingsControl::RemoveProjectCommandEnvironment(
                        command_index,
                        environment_index,
                    ),
                ],
                h_flex()
                    .gap_1()
                    .child(text_field(
                        format!("project-command-env-name-{command_index}-{environment_index}"),
                        entry.name.clone(),
                        SettingsInput::Project(ProjectTextField::CommandEnvironmentName(
                            command_index,
                            environment_index,
                        )),
                        editor,
                        colors,
                        handle,
                    ))
                    .child(action_button(
                        editor,
                        format!("project-command-env-remove-{command_index}-{environment_index}"),
                        "×".to_owned(),
                        SettingsControl::RemoveProjectCommandEnvironment(
                            command_index,
                            environment_index,
                        ),
                        true,
                        colors,
                        handle,
                    ))
                    .into_any_element(),
                colors,
            ));
            content.push(control_row(
                editor,
                format!(
                    "Command {} environment {} · value",
                    command_index + 1,
                    environment_index + 1
                ),
                &[SettingsControl::Input(SettingsInput::Project(
                    ProjectTextField::CommandEnvironmentValue(command_index, environment_index),
                ))],
                text_field(
                    format!("project-command-env-value-{command_index}-{environment_index}"),
                    entry.value.clone(),
                    SettingsInput::Project(ProjectTextField::CommandEnvironmentValue(
                        command_index,
                        environment_index,
                    )),
                    editor,
                    colors,
                    handle,
                ),
                colors,
            ));
        }
        content.push(
            h_flex()
                .justify_end()
                .child(action_button(
                    editor,
                    format!("project-command-env-add-{command_index}"),
                    "Add command environment variable".to_owned(),
                    SettingsControl::AddProjectCommandEnvironment(command_index),
                    true,
                    colors,
                    handle,
                ))
                .into_any_element(),
        );
    }
    content.push(
        h_flex()
            .justify_end()
            .child(action_button(
                editor,
                "project-command-add".to_owned(),
                "Add command".to_owned(),
                SettingsControl::AddProjectCommand,
                true,
                colors,
                handle,
            ))
            .into_any_element(),
    );
}

/// The profile-override rows, merged over the application profiles by name.
fn push_project_profile_rows(
    content: &mut Vec<AnyElement>,
    editor: &SettingsEditor,
    form: &crate::project_form::ProjectForm,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) {
    content.push(
        section_heading(
            "Profiles",
            "Overrides merged over the application profiles by name. Leave the program empty to keep the inherited command and only change the theme, icon, or visibility.",
            colors,
        )
        .into_any_element(),
    );
    for (index, profile) in form.profiles.iter().enumerate() {
        let [
            profile_name_control,
            profile_remove_control,
            profile_program_control,
            profile_arguments_control,
            profile_visibility_control,
            profile_icon_control,
            profile_theme_control,
            profile_dark_theme_control,
        ] = project_profile_controls(index);
        let profile_name_row_controls =
            [profile_name_control.clone(), profile_remove_control.clone()];
        content.push(control_row(
            editor,
            format!("Profile {} · name", index + 1),
            &profile_name_row_controls,
            h_flex()
                .gap_1()
                .child(text_field(
                    format!("project-profile-name-{index}"),
                    profile.name.clone(),
                    SettingsInput::Project(ProjectTextField::ProfileName(index)),
                    editor,
                    colors,
                    handle,
                ))
                .child(action_button(
                    editor,
                    format!("project-profile-remove-{index}"),
                    "×".to_owned(),
                    profile_remove_control.clone(),
                    true,
                    colors,
                    handle,
                ))
                .into_any_element(),
            colors,
        ));
        content.push(control_row(
            editor,
            format!("Profile {} · program", index + 1),
            std::slice::from_ref(&profile_program_control),
            text_field(
                format!("project-profile-program-{index}"),
                profile.program.clone(),
                SettingsInput::Project(ProjectTextField::ProfileProgram(index)),
                editor,
                colors,
                handle,
            ),
            colors,
        ));
        content.push(control_row(
            editor,
            format!("Profile {} · arguments (comma separated)", index + 1),
            std::slice::from_ref(&profile_arguments_control),
            text_field(
                format!("project-profile-arguments-{index}"),
                profile.arguments.clone(),
                SettingsInput::Project(ProjectTextField::ProfileArguments(index)),
                editor,
                colors,
                handle,
            ),
            colors,
        ));
        let visibility_handle = handle.clone();
        content.push(control_row(
            editor,
            format!("Profile {} · shown in menus", index + 1),
            std::slice::from_ref(&profile_visibility_control),
            switch(
                ("project-profile-visibility", index),
                (!profile.hidden).into(),
            )
            .label(if profile.hidden { "Hidden" } else { "Visible" })
            .full_width(true)
            .aria_label("project-profile-visibility")
            .on_click(move |state, window, cx| {
                visibility_handle
                    .update(cx, |this, cx| {
                        this.set_settings_toggle(
                            SettingsToggle::ProjectProfileVisibility(index),
                            state.selected(),
                            window,
                            cx,
                        );
                    })
                    .ok();
            })
            .into_any_element(),
            colors,
        ));
        content.push(control_row(
            editor,
            format!("Profile {} · icon", index + 1),
            std::slice::from_ref(&profile_icon_control),
            dropdown_field(
                format!("project-profile-icon-{index}"),
                profile
                    .icon
                    .as_ref()
                    .map(ProfileIcon::label)
                    .unwrap_or("Automatic")
                    .to_owned(),
                SettingsDropdown::ProjectProfileIcon(index),
                editor,
                colors,
                handle,
            ),
            colors,
        ));
        content.push(control_row(
            editor,
            format!("Profile {} · light theme", index + 1),
            std::slice::from_ref(&profile_theme_control),
            dropdown_field(
                format!("project-profile-theme-{index}"),
                profile
                    .theme
                    .clone()
                    .unwrap_or_else(|| crate::project_form::PROJECT_INHERIT_LABEL.to_owned()),
                SettingsDropdown::ProjectProfileTheme(index),
                editor,
                colors,
                handle,
            ),
            colors,
        ));
        content.push(control_row(
            editor,
            format!("Profile {} · dark theme", index + 1),
            std::slice::from_ref(&profile_dark_theme_control),
            dropdown_field(
                format!("project-profile-dark-theme-{index}"),
                profile
                    .dark_theme
                    .clone()
                    .unwrap_or_else(|| crate::project_form::PROJECT_INHERIT_LABEL.to_owned()),
                SettingsDropdown::ProjectProfileDarkTheme(index),
                editor,
                colors,
                handle,
            ),
            colors,
        ));
    }
    content.push(
        h_flex()
            .justify_end()
            .child(action_button(
                editor,
                "project-profile-add".to_owned(),
                "Add profile override".to_owned(),
                SettingsControl::AddProjectProfile,
                true,
                colors,
                handle,
            ))
            .into_any_element(),
    );
}

/// The pane-template rows: the project's initial split, and the template
/// editor overlaid on the application's templates.
fn push_project_template_rows(
    content: &mut Vec<AnyElement>,
    editor: &SettingsEditor,
    form: &crate::project_form::ProjectForm,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) {
    content.push(
        section_heading(
            "Pane templates",
            "The application's templates are read-only here; overriding one or adding a new one applies only inside this project. The initial split replaces the active pane subtree the first time a tab enters the project.",
            colors,
        )
        .into_any_element(),
    );
    content.push(control_row(
        editor,
        "Initial split",
        &[SettingsControl::Dropdown(
            SettingsDropdown::ProjectInitialSplit,
        )],
        dropdown_field(
            "project-initial-split".to_owned(),
            form.initial_split
                .clone()
                .unwrap_or_else(|| "None".to_owned()),
            SettingsDropdown::ProjectInitialSplit,
            editor,
            colors,
            handle,
        ),
        colors,
    ));
    content.push(
        div()
            .mt_3()
            .child(render_pane_templates_page(editor, colors, handle))
            .into_any_element(),
    );
}
