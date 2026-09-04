use super::*;
use crate::settings_editor::PaneTemplatesForm;

#[test]
fn only_the_selected_split_exposes_split_editor_controls() {
    let node = PaneTemplateNodeForm::Split {
        axis: PaneSplitAxis::Vertical,
        first: Box::new(PaneTemplateNodeForm::empty_two_pane()),
        second: Box::new(PaneTemplateNodeForm::Pane(PaneTemplatePaneForm::default())),
    };
    let root = PaneTemplateNodePath::ROOT;
    let selected_split = root.child(false).unwrap();
    let mut controls = Vec::new();

    add_node_controls_with_template(&mut controls, &node, root, true, 0, Some(selected_split));

    assert!(controls.contains(&SettingsControl::Dropdown(
        SettingsDropdown::PaneTemplateAxis(selected_split)
    )));
    assert!(controls.contains(&SettingsControl::SwapPaneTemplateChildren(selected_split)));
    assert!(controls.contains(&SettingsControl::RemovePaneTemplateNode(selected_split)));
    assert!(!controls.contains(&SettingsControl::Dropdown(
        SettingsDropdown::PaneTemplateAxis(root)
    )));
    assert!(!controls.contains(&SettingsControl::SwapPaneTemplateChildren(root)));
}

#[test]
fn returning_to_the_parent_split_restores_its_editor_controls() {
    let node = PaneTemplateNodeForm::empty_two_pane();
    let root = PaneTemplateNodePath::ROOT;
    let left = root.child(false).unwrap();
    let mut templates = PaneTemplatesForm {
        templates: vec![PaneTemplateForm {
            name: TextField::new("custom"),
            original_name: "custom".to_owned(),
            overridden: true,
            inherited_source: None,
            environment: Vec::new(),
            node: node.clone(),
        }],
        selected_template: 0,
        selected_node: Some(left),
        available_profiles: Vec::new(),
    };

    assert!(templates.toggle_node_selection(left));
    assert_eq!(templates.selected_node, Some(root));

    let mut controls = Vec::new();
    add_node_controls_with_template(&mut controls, &node, root, true, 0, templates.selected_node);
    assert!(controls.contains(&SettingsControl::Dropdown(
        SettingsDropdown::PaneTemplateAxis(root)
    )));
    assert!(controls.contains(&SettingsControl::SwapPaneTemplateChildren(root)));
}

#[test]
fn stacked_command_rows_are_keyboard_reachable_in_render_order() {
    let path = PaneTemplateNodePath::ROOT.child(true).unwrap();
    let pane = PaneTemplatePaneForm {
        stack: vec![
            PaneTemplateCommandForm {
                program: TextField::new("cargo"),
                args: vec![TextField::new("watch")],
            },
            PaneTemplateCommandForm {
                program: TextField::new("tail"),
                args: Vec::new(),
            },
        ],
        ..PaneTemplatePaneForm::default()
    };
    let node_input = |field| {
        SettingsControl::Input(SettingsInput::PaneTemplate(PaneTemplateTextField::Node(
            2, path, field,
        )))
    };
    let mut controls = Vec::new();

    add_stack_controls(&mut controls, &pane, path, 2);

    assert_eq!(
        controls,
        vec![
            node_input(PaneTemplateNodeField::StackProgram(0)),
            SettingsControl::RemovePaneTemplateStackEntry(path, 0),
            node_input(PaneTemplateNodeField::StackArgument(0, 0)),
            SettingsControl::RemovePaneTemplateStackArgument(path, 0, 0),
            SettingsControl::AddPaneTemplateStackArgument(path, 0),
            node_input(PaneTemplateNodeField::StackProgram(1)),
            SettingsControl::RemovePaneTemplateStackEntry(path, 1),
            SettingsControl::AddPaneTemplateStackArgument(path, 1),
            SettingsControl::AddPaneTemplateStackEntry(path),
        ]
    );
}

#[test]
fn stacked_commands_are_only_offered_for_the_selected_leaf() {
    let node = PaneTemplateNodeForm::empty_two_pane();
    let root = PaneTemplateNodePath::ROOT;
    let left = root.child(false).unwrap();
    let right = root.child(true).unwrap();
    let mut controls = Vec::new();

    add_node_controls_with_template(&mut controls, &node, root, true, 0, Some(left));

    assert!(controls.contains(&SettingsControl::AddPaneTemplateStackEntry(left)));
    assert!(!controls.contains(&SettingsControl::AddPaneTemplateStackEntry(right)));
}

#[test]
fn global_environment_rows_are_keyboard_reachable() {
    let mut controls = Vec::new();
    add_global_environment_controls(&mut controls, 3, 2);

    assert_eq!(
        controls,
        vec![
            SettingsControl::Input(SettingsInput::PaneTemplate(
                PaneTemplateTextField::GlobalEnvironmentName(3, 0),
            )),
            SettingsControl::Input(SettingsInput::PaneTemplate(
                PaneTemplateTextField::GlobalEnvironmentValue(3, 0),
            )),
            SettingsControl::RemovePaneTemplateGlobalEnvironment(0),
            SettingsControl::Input(SettingsInput::PaneTemplate(
                PaneTemplateTextField::GlobalEnvironmentName(3, 1),
            )),
            SettingsControl::Input(SettingsInput::PaneTemplate(
                PaneTemplateTextField::GlobalEnvironmentValue(3, 1),
            )),
            SettingsControl::RemovePaneTemplateGlobalEnvironment(1),
            SettingsControl::AddPaneTemplateGlobalEnvironment,
        ]
    );
}

/// A control this page does not own must leave the form alone: it is not a
/// change, so it must not mark the configuration dirty or clear the message the
/// last real change left. The distinction lives in
/// `apply_pane_template_control`'s `Ok(None)`, and nothing else pins it.
#[test]
fn a_control_this_page_does_not_own_leaves_the_form_untouched() {
    let config = Config::parse("{}", None, None).unwrap();
    let mut editor = crate::settings_ui::controls::tests::configuration_editor(&config);
    editor.configuration_dirty = false;
    editor.message = Some((false, "Saved".to_owned()));

    let outcome = apply_pane_template_control(&mut editor, SettingsControl::Save);

    assert!(
        matches!(outcome, Ok(None)),
        "a control from another page is neither applied nor an error"
    );
    assert!(
        !editor.configuration_dirty,
        "an unowned control must not mark the configuration dirty"
    );
    assert_eq!(
        editor.message,
        Some((false, "Saved".to_owned())),
        "an unowned control must not clear the last message"
    );
}

/// The same for a control this page *does* own whose node has since gone: the
/// early returns in those arms mean "nothing changed", not "changed
/// successfully".
#[test]
fn a_template_control_for_a_missing_node_reports_no_change() {
    let config = Config::parse("{}", None, None).unwrap();
    let mut editor = crate::settings_ui::controls::tests::configuration_editor(&config);
    editor.configuration_dirty = false;
    let missing = PaneTemplateNodePath::ROOT
        .child(false)
        .unwrap()
        .child(false)
        .unwrap();

    let outcome = apply_pane_template_control(
        &mut editor,
        SettingsControl::AddPaneTemplateArgument(missing),
    );

    assert!(matches!(outcome, Ok(None)));
    assert!(!editor.configuration_dirty);
}
