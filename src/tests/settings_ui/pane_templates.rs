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
