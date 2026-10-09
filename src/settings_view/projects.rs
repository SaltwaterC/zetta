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
    widgets: &super::pages::PageWidgets<'_>,
) -> AnyElement {
    match project_editor(editor) {
        Some(project) => render_project_config(editor, project, colors, handle, widgets),
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
        let controls = [
            SettingsControl::OpenProject(index),
            SettingsControl::EditProject(index),
            SettingsControl::RemoveProject(index),
        ];
        let focused = controls
            .iter()
            .any(|control| editor.focused_control.as_ref() == Some(control));
        rows.push(
            track_focus_scroll(card_frame(focused, colors), editor, &controls)
                .child(card_text(
                    project_display_name(root).to_owned(),
                    [
                        root.display().to_string().into(),
                        ProjectConfig::path_for(root).display().to_string().into(),
                    ],
                    colors,
                ))
                .child(
                    h_flex()
                        .mt_3()
                        .gap_2()
                        .child(
                            SettingsButton::new(
                                format!("settings-open-project-{index}"),
                                "Open",
                                SettingsControl::OpenProject(index),
                            )
                            .render(editor, colors, handle),
                        )
                        .child(
                            SettingsButton::new(
                                format!("settings-edit-project-{index}"),
                                "Edit config",
                                SettingsControl::EditProject(index),
                            )
                            .loading(editor.project_loading)
                            .render(editor, colors, handle),
                        )
                        .child(
                            SettingsButton::new(
                                format!("settings-remove-project-{index}"),
                                "Remove",
                                SettingsControl::RemoveProject(index),
                            )
                            .destructive()
                            .confirm_with("Confirm remove")
                            .render(editor, colors, handle),
                        ),
                )
                .into_any_element(),
        );
    }

    v_flex()
        .child(section_heading(
            "Projects",
            Some(
                "A project's .zetta/config.json applies while the active pane is inside its root"
                    .into(),
            ),
            colors,
        ))
        .child(div().mb_2().text_xs().text_color(colors.text_muted).child(
            "Edit config opens a builder for everything a project can override. Commands edited outside Zetta require \
                 approval through Review and trust. Commands saved here are approved when you save.",
        ))
        .children(rows)
        .when(editor.project_roots.is_empty(), |page| {
            page.child(empty_state("No projects are registered yet", colors))
        })
        .child(add_row(
            action_button(
                editor,
                "settings-add-project".to_owned(),
                "Add project".to_owned(),
                SettingsControl::AddProject,
                true,
                colors,
                handle,
            ),
            editor,
            &[SettingsControl::AddProject],
        ))
        .into_any_element()
}

fn render_project_config(
    editor: &SettingsEditor,
    project: &ProjectEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
    widgets: &super::pages::PageWidgets<'_>,
) -> AnyElement {
    let opacity_slider = widgets.opacity_slider;
    let form = &project.form;
    let current_icon = form.default_tab_icon;
    let actions = project_config_actions(editor, project, colors, handle);
    let tab_icon_trigger = project_tab_icon_trigger(editor, project, colors, handle);
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
                    .unwrap_or_else(|| crate::settings_ui::INHERIT_LABEL.to_owned()),
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
                    .unwrap_or_else(|| crate::settings_ui::INHERIT_LABEL.to_owned()),
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
                    .unwrap_or_else(|| crate::settings_ui::INHERIT_LABEL.to_owned()),
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
                    "Reset to inherited".to_owned(),
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
            toggle_switch(
                "project-inactive-pane-opacity-override",
                "Override the inactive pane opacity",
                form.inactive_pane_opacity.is_some(),
                SettingsToggle::ProjectOpacityOverride,
                handle,
            ),
            colors,
        ),
    ];
    if let Some(opacity) = form.inactive_pane_opacity {
        content.push(control_row(
            editor,
            "Inactive pane opacity",
            &[SettingsControl::Opacity(OpacityTarget::Project)],
            opacity_slider(opacity, OpacityTarget::Project),
            colors,
        ));
    }

    push_project_environment_rows(&mut content, editor, form, colors, handle);
    push_project_command_rows(&mut content, editor, form, colors, handle);
    push_project_profile_rows(&mut content, editor, form, colors, widgets, handle);
    push_project_template_rows(&mut content, editor, form, colors, widgets, handle);
    v_flex().children(content).into_any_element()
}

/// The builder's Back and Open-file buttons. Saving is the dialog header's
/// Save, which writes the open project's file while the builder is up; the
/// builder used to carry a second Save of its own that did the same thing.
fn project_config_actions(
    editor: &SettingsEditor,
    project: &ProjectEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> AnyElement {
    let saving = project.save_in_progress;
    h_flex()
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
            "project-config-open-file".to_owned(),
            "Open in editor".to_owned(),
            SettingsControl::OpenProjectConfigFile,
            !saving,
            colors,
            handle,
        ))
        .into_any_element()
}

/// The row that opens the tab-icon picker for the project's default icon.
fn project_tab_icon_trigger(
    editor: &SettingsEditor,
    project: &ProjectEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> AnyElement {
    let current_icon = project.form.default_tab_icon;
    picker_trigger(
        "project-tab-icon-picker-trigger",
        SettingsControl::ProjectTabIconPicker,
        h_flex()
            .gap_2()
            .child(Icon::new(current_icon.icon().unwrap_or(IconName::Dash)))
            .child(current_icon.label()),
        editor,
        colors,
        handle,
    )
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
        section_heading("Environment", Some("Applied to every terminal started inside the project. Template and pane values override matching keys, and reserved ZETTA_* names cannot be replaced.".into()), colors),
    );
    for (index, entry) in form.environment.iter().enumerate() {
        content.extend(environment_pair_rows(
            EnvironmentPair {
                label: format!("Variable {}", index + 1),
                id: format!("project-env-{index}"),
                name: &entry.name,
                name_input: SettingsInput::Project(ProjectTextField::EnvironmentName(index)),
                value: &entry.value,
                value_input: SettingsInput::Project(ProjectTextField::EnvironmentValue(index)),
                remove: SettingsControl::RemoveProjectEnvironment(index),
                editable: true,
            },
            editor,
            colors,
            handle,
        ));
    }
    content.push(add_row(
        action_button(
            editor,
            "project-env-add".to_owned(),
            "Add environment variable".to_owned(),
            SettingsControl::AddProjectEnvironment,
            true,
            colors,
            handle,
        ),
        editor,
        &[SettingsControl::AddProjectEnvironment],
    ));
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
        section_heading("Commands", Some("Registered commands run raw shell code in the active pane. Command environments override project environment values for that invocation and do not persist in the pane.".into()), colors),
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
                .child(settings_remove_button(
                    editor,
                    format!("project-command-remove-{command_index}"),
                    SettingsControl::RemoveProjectCommand(command_index),
                    "command",
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
            content.extend(environment_pair_rows(
                EnvironmentPair {
                    label: format!(
                        "Command {} variable {}",
                        command_index + 1,
                        environment_index + 1
                    ),
                    id: format!("project-command-env-{command_index}-{environment_index}"),
                    name: &entry.name,
                    name_input: SettingsInput::Project(ProjectTextField::CommandEnvironmentName(
                        command_index,
                        environment_index,
                    )),
                    value: &entry.value,
                    value_input: SettingsInput::Project(ProjectTextField::CommandEnvironmentValue(
                        command_index,
                        environment_index,
                    )),
                    remove: SettingsControl::RemoveProjectCommandEnvironment(
                        command_index,
                        environment_index,
                    ),
                    editable: true,
                },
                editor,
                colors,
                handle,
            ));
        }
        content.push(add_row(
            action_button(
                editor,
                format!("project-command-env-add-{command_index}"),
                "Add command environment variable".to_owned(),
                SettingsControl::AddProjectCommandEnvironment(command_index),
                true,
                colors,
                handle,
            ),
            editor,
            &[SettingsControl::AddProjectCommandEnvironment(command_index)],
        ));
    }
    content.push(add_row(
        action_button(
            editor,
            "project-command-add".to_owned(),
            "Add command".to_owned(),
            SettingsControl::AddProjectCommand,
            true,
            colors,
            handle,
        ),
        editor,
        &[SettingsControl::AddProjectCommand],
    ));
}

/// The profile overrides, merged over the application profiles by name: one
/// card each, the same card the Configuration page draws a profile with.
fn push_project_profile_rows(
    content: &mut Vec<AnyElement>,
    editor: &SettingsEditor,
    form: &crate::project_form::ProjectForm,
    colors: &ThemeColors,
    widgets: &super::pages::PageWidgets<'_>,
    handle: &WeakEntity<Zetta>,
) {
    content.push(section_heading(
        "Profiles",
        Some(
            "Overrides merged over the application profiles by name. Leave the program empty \
             to keep the inherited command and change only the theme, icon or visibility."
                .into(),
        ),
        colors,
    ));
    for (index, profile) in form.profiles.iter().enumerate() {
        let controls = project_profile_controls(index, profile.arguments.len());
        content.push(super::pages::render_profile_card(
            super::pages::ProfileCard {
                profile,
                identity: Some(super::pages::ProfileIdentity {
                    id_prefix: format!("project-profile-{index}"),
                    name: SettingsInput::Project(ProjectTextField::ProfileName(index)),
                    program: SettingsInput::Project(ProjectTextField::ProfileProgram(index)),
                    program_hint: Some("Leave empty to keep the inherited command"),
                    remove: SettingsControl::RemoveProjectProfile(index),
                    remove_label: "profile override",
                    arguments: ProfileTarget::Project(index),
                }),
                overrides: super::pages::ProfileOverrides {
                    id_prefix: format!("project-profile-{index}"),
                    visibility: SettingsToggle::ProjectProfileVisibility(index),
                    icon: SettingsDropdown::ProjectProfileIcon(index),
                    theme: SettingsDropdown::ProjectProfileTheme(index),
                    dark_theme: SettingsDropdown::ProjectProfileDarkTheme(index),
                    profile,
                    automatic_icon: &profile.automatic_icon,
                },
                controls: &controls,
            },
            editor,
            colors,
            handle,
            widgets,
        ));
    }
    content.push(add_row(
        action_button(
            editor,
            "project-profile-add".to_owned(),
            "Add profile override".to_owned(),
            SettingsControl::AddProjectProfile,
            true,
            colors,
            handle,
        ),
        editor,
        &[SettingsControl::AddProjectProfile],
    ));
}

/// The pane-template rows: the project's initial split, and the template
/// editor overlaid on the application's templates.
fn push_project_template_rows(
    content: &mut Vec<AnyElement>,
    editor: &SettingsEditor,
    form: &crate::project_form::ProjectForm,
    colors: &ThemeColors,
    widgets: &super::pages::PageWidgets<'_>,
    handle: &WeakEntity<Zetta>,
) {
    content.push(
        section_heading("Pane templates", Some("The application's templates are read-only here; overriding one or adding a new one applies only inside this project. The initial split replaces the active pane subtree the first time a tab enters the project.".into()), colors),
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
            .child(render_pane_templates_page(editor, colors, widgets, handle))
            .into_any_element(),
    );
}
