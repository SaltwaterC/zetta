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
            built_in: false,
            overridden: true,
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
