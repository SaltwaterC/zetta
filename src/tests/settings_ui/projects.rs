use super::*;

use crate::settings_editor::{ConfigurationForm, KeymapForm};
use crate::settings_ui::pane_templates::templates;
use std::collections::HashMap;

fn base_config() -> Config {
    Config::parse(
        r#"{
            "profiles": [{ "name": "Toolbox", "program": "/bin/sh" }],
            "pane_split_templates": {
                "user-pair": { "layout": { "vertical": [{ "label": "left" }, { "label": "right" }] } }
            }
        }"#,
        None,
        None,
    )
    .unwrap()
}

/// A settings editor with no files behind it: both forms fall back to their
/// bundled defaults when the paths do not exist, which is all these tests need.
fn test_editor(config: &Config, project: Option<ProjectEditor>) -> SettingsEditor {
    let missing = Path::new("zetta-settings-ui-projects-tests-nonexistent.json");
    SettingsEditor {
        page: SettingsPage::Projects,
        configuration: ConfigurationForm::load(missing, config).unwrap(),
        keymap: KeymapForm::load(missing).unwrap(),
        profile_names: Arc::from([]),
        themes: Arc::from(["One Dark".to_owned()]),
        theme_extension_query: TextField::default(),
        theme_extensions: Vec::new(),
        installed_theme_extensions: Vec::new(),
        theme_extensions_loading: false,
        theme_extensions_searched: false,
        theme_extension_downloading: None,
        actions: Arc::from([]),
        pane_template_names: Arc::from([]),
        project_roots: Arc::from([PathBuf::from("/projects/demo")]),
        project,
        project_loading: false,
        fonts: Arc::from([]),
        normalized_fonts: Arc::from([]),
        font_query: None,
        profile_draft: None,
        keymap_search: TextField::default(),
        settings_scroll: ScrollHandle::new(),
        profile_draft_scroll: ScrollHandle::new(),
        dropdown_scroll: UniformListScrollHandle::new(),
        font_scroll: UniformListScrollHandle::new(),
        keymap_scroll: UniformListScrollHandle::new(),
        numeric_repeat_generation: 0,
        scroll_geometry_initialized: true,
        focused_input: None,
        focused_control: None,
        focus_scroll_request: None,
        keymap_capture: None,
        open_dropdown: None,
        dropdown_index: 0,
        dropdown_query: String::new(),
        dropdown_anchor: Point::default(),
        configuration_dirty: false,
        keymap_dirty: false,
        message: None,
        pane_template_validation_error: None,
        pane_template_validation_generation: 0,
        settings_save_in_progress: false,
        keymap_filtered_sections: None,
        keymap_search_query_cache: String::new(),
        keymap_filtered_bindings: HashMap::new(),
        keymap_rows_cache: None,
        keymap_row_data_cache: None,
        open_dropdown_options: Arc::from([]),
        open_dropdown_rows: Arc::from([]),
        open_dropdown_widest_row: None,
        font_filtered_indices: None,
        font_search_query_cache: String::new(),
        controls_cache: None,
        controls_generation: 0,
    }
}

fn test_project(config: &Config, source: &str) -> ProjectEditor {
    ProjectEditor {
        root: PathBuf::from("/projects/demo"),
        index: 0,
        form: ProjectForm::parse(
            source,
            Path::new("/projects/demo/.zetta/config.json"),
            config,
        )
        .unwrap(),
        dirty: false,
        save_in_progress: false,
    }
}

#[test]
fn the_project_picker_starts_at_the_active_panes_directory() {
    let pane_directory = PathBuf::from("/projects/demo");

    assert_eq!(
        prompt_start_directory(Some(pane_directory.clone())),
        Some(pane_directory)
    );
    assert_eq!(
        prompt_start_directory(None),
        std::env::current_dir().ok(),
        "a pane that reported no directory falls back to Zetta's own"
    );
}

#[test]
fn the_project_list_exposes_one_control_group_per_registered_project() {
    let config = base_config();
    let editor = test_editor(&config, None);

    assert_eq!(
        project_controls(&editor),
        vec![
            SettingsControl::AddProject,
            SettingsControl::OpenProject(0),
            SettingsControl::EditProject(0),
            SettingsControl::RemoveProject(0),
        ]
    );
}

#[test]
fn the_builder_replaces_the_list_controls_and_reaches_every_row() {
    let config = base_config();
    let project = test_project(
        &config,
        r#"{
            "env": { "RUST_LOG": "debug" },
            "profiles": [{ "name": "Toolbox", "theme": "One Dark" }]
        }"#,
    );
    let editor = test_editor(&config, Some(project));
    let controls = project_controls(&editor);

    assert!(!controls.contains(&SettingsControl::AddProject));
    assert!(controls.contains(&SettingsControl::CloseProjectConfig));
    assert!(controls.contains(&SettingsControl::SaveProjectConfig));
    assert!(controls.contains(&SettingsControl::OpenProjectConfigFile));
    for control in [
        SettingsControl::Input(SettingsInput::Project(ProjectTextField::EnvironmentName(0))),
        SettingsControl::Input(SettingsInput::Project(ProjectTextField::EnvironmentValue(
            0,
        ))),
        SettingsControl::RemoveProjectEnvironment(0),
        SettingsControl::AddProjectEnvironment,
        SettingsControl::Input(SettingsInput::Project(ProjectTextField::ProfileName(0))),
        SettingsControl::Dropdown(SettingsDropdown::ProjectProfileTheme(0)),
        SettingsControl::Toggle(SettingsToggle::ProjectProfileVisibility(0)),
        SettingsControl::RemoveProjectProfile(0),
        SettingsControl::AddProjectProfile,
        SettingsControl::Dropdown(SettingsDropdown::ProjectInitialSplit),
    ] {
        assert!(controls.contains(&control), "{control:?} is unreachable");
    }
    // The builder hosts the pane-template editor, so its controls join the same
    // tab order.
    assert!(controls.contains(&SettingsControl::SelectPaneTemplate(0)));
    assert!(controls.contains(&SettingsControl::NewPaneTemplate));
}

#[test]
fn project_profile_controls_follow_the_visible_selection_order() {
    let config = base_config();
    let project = test_project(
        &config,
        r#"{
            "profiles": [{ "name": "Toolbox" }]
        }"#,
    );
    let editor = test_editor(&config, Some(project));
    let controls = project_controls(&editor);
    let profile = project_profile_controls(0);
    assert_eq!(
        profile,
        [
            SettingsControl::Input(SettingsInput::Project(ProjectTextField::ProfileName(0))),
            SettingsControl::RemoveProjectProfile(0),
            SettingsControl::Input(SettingsInput::Project(ProjectTextField::ProfileProgram(0))),
            SettingsControl::Input(SettingsInput::Project(ProjectTextField::ProfileArguments(
                0
            ),)),
            SettingsControl::Toggle(SettingsToggle::ProjectProfileVisibility(0)),
            SettingsControl::Dropdown(SettingsDropdown::ProjectProfileIcon(0)),
            SettingsControl::Dropdown(SettingsDropdown::ProjectProfileTheme(0)),
            SettingsControl::Dropdown(SettingsDropdown::ProjectProfileDarkTheme(0)),
        ]
    );
    let start = controls
        .iter()
        .position(|control| control == &profile[0])
        .expect("the project profile starts in the builder tab order");

    assert_eq!(&controls[start..start + profile.len()], profile.as_slice());
}

#[test]
fn the_opacity_slider_is_only_reachable_while_the_project_overrides_it() {
    let config = base_config();
    let editor = test_editor(&config, Some(test_project(&config, "{}")));
    assert!(!project_controls(&editor).contains(&SettingsControl::ProjectOpacity));

    let editor = test_editor(
        &config,
        Some(test_project(&config, r#"{"inactive_pane_opacity": 0.5}"#)),
    );
    assert!(project_controls(&editor).contains(&SettingsControl::ProjectOpacity));
}

#[test]
fn the_pane_template_editor_follows_the_visible_page() {
    let config = base_config();
    let project = test_project(
        &config,
        r#"{
            "pane_split_templates": {
                "project-only": { "layout": { "vertical": [{}, {}] } }
            }
        }"#,
    );
    let mut editor = test_editor(&config, Some(project));

    // On the Projects page the editor edits the open project's overlay.
    assert!(
        templates(&editor)
            .names()
            .contains(&"project-only".to_owned())
    );
    assert!(templates(&editor).names().contains(&"user-pair".to_owned()));

    // Switching to the Templates page must go back to the user configuration
    // even though the project builder is still open behind it, otherwise the
    // Templates page would silently edit the project.
    editor.page = SettingsPage::PaneTemplates;
    assert!(
        !templates(&editor)
            .names()
            .contains(&"project-only".to_owned())
    );
    assert!(project_editor(&editor).is_none());
}

#[test]
fn a_dropdown_selection_of_inherit_clears_the_field_and_marks_the_form_dirty() {
    let config = base_config();
    let mut editor = test_editor(
        &config,
        Some(test_project(&config, r#"{"theme":"One Dark"}"#)),
    );

    assert!(set_project_dropdown(
        &mut editor,
        SettingsDropdown::ProjectTheme,
        PROJECT_INHERIT_LABEL,
    ));

    let project = editor.project.as_ref().unwrap();
    assert_eq!(project.form.theme, None);
    assert!(project.dirty);
}

#[test]
fn closing_the_builder_returns_focus_to_the_row_it_was_opened_from() {
    let config = base_config();
    let mut editor = test_editor(&config, None);
    let project = test_project(&config, "{}");

    assert_eq!(
        project_row_control(&editor, Some(&project)),
        SettingsControl::EditProject(0)
    );

    // The project can be unregistered from another window while the builder is
    // open, which leaves no row to focus.
    editor.project_roots = Arc::from([]);
    assert_eq!(
        project_row_control(&editor, Some(&project)),
        SettingsControl::Tab(SettingsPage::Projects)
    );
}
