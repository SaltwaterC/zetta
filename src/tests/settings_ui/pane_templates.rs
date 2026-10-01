use super::*;
use crate::settings_editor::PaneTemplatesForm;

/// A tree's tab order as the page builds it: every preview node, then the
/// selected node's form.
fn node_then_detail_controls(
    node: &PaneTemplateNodeForm,
    root: PaneTemplateNodePath,
    editable: bool,
    template_index: usize,
    selected: Option<PaneTemplateNodePath>,
) -> Vec<SettingsControl> {
    let mut nodes = Vec::new();
    let mut details = Vec::new();
    add_node_controls_with_template(
        &mut nodes,
        &mut details,
        node,
        root,
        editable,
        template_index,
        selected,
    );
    nodes.extend(details);
    nodes
}

/// The page draws the whole preview and then the selected node's form beneath
/// it, so Tab reaches every preview node before the first field of the form.
#[test]
fn the_preview_is_tabbed_through_before_the_selected_nodes_form() {
    let node = PaneTemplateNodeForm::empty_two_pane();
    let root = PaneTemplateNodePath::ROOT;
    let left = root.child(false).unwrap();
    let right = root.child(true).unwrap();

    let controls = node_then_detail_controls(&node, root, true, 0, Some(left));

    let last_node = controls
        .iter()
        .position(|control| *control == SettingsControl::SelectPaneTemplateNode(right))
        .unwrap();
    let first_detail = controls
        .iter()
        .position(|control| {
            *control == SettingsControl::SplitPaneTemplate(left, PaneSplitAxis::Horizontal)
        })
        .unwrap();
    assert!(last_node < first_detail);
}

#[test]
fn only_the_selected_split_exposes_split_editor_controls() {
    let node = PaneTemplateNodeForm::Split {
        axis: PaneSplitAxis::Vertical,
        first: Box::new(PaneTemplateNodeForm::empty_two_pane()),
        second: Box::new(PaneTemplateNodeForm::Pane(PaneTemplatePaneForm::default())),
    };
    let root = PaneTemplateNodePath::ROOT;
    let selected_split = root.child(false).unwrap();
    let controls = node_then_detail_controls(&node, root, true, 0, Some(selected_split));

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

    let controls = node_then_detail_controls(&node, root, true, 0, templates.selected_node);
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
    let controls = node_then_detail_controls(&node, root, true, 0, Some(left));

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
    editor.message = Some((Tone::Info, "Saved".to_owned()));

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
        Some((Tone::Info, "Saved".to_owned())),
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

/// The overlay's opacity is a slider now, as the inactive-pane opacity is, and
/// sliding it back to the default stores nothing rather than an explicit value
/// the template never chose.
#[test]
fn the_overlay_opacity_slider_stores_the_default_as_unset() {
    let source = r#"{"pane_split_templates":{"custom":{"layout":{"vertical":[{"label":"left","overlay":{"text":"Prod"}},{"label":"right"}]}}}}"#;
    let config = Config::parse(source, None, None).unwrap();
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), source).unwrap();
    let mut editor = crate::settings_ui::controls::tests::configuration_editor(&config);
    editor.configuration =
        crate::settings_editor::ConfigurationForm::load(file.path(), &config).unwrap();
    editor.page = SettingsPage::PaneTemplates;
    let index = templates(&editor)
        .templates
        .iter()
        .position(|template| template.name.text == "custom")
        .unwrap();
    templates_mut(&mut editor).selected_template = index;
    let root = PaneTemplateNodePath::ROOT.child(false).unwrap();

    assert_eq!(
        overlay_opacity(&editor, root),
        Some(crate::pane::DEFAULT_OVERLAY_OPACITY)
    );
    assert!(set_overlay_opacity(&mut editor, root, 0.4));
    assert_eq!(overlay_opacity(&editor, root), Some(0.4));
    assert!(editor.configuration_dirty);

    assert!(set_overlay_opacity(
        &mut editor,
        root,
        crate::pane::DEFAULT_OVERLAY_OPACITY
    ));
    let Some(PaneTemplateNodeForm::Pane(pane)) =
        templates(&editor).selected().unwrap().node.node_at(root)
    else {
        panic!("the left pane");
    };
    assert_eq!(pane.overlay.as_ref().unwrap().opacity.text, "");
}
