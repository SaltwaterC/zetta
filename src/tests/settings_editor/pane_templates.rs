use super::*;
use crate::settings_editor::tests::{
    configuration_form_with_empty_custom_template, settings_test_path,
};

fn pane_for_test(
    form: &mut ConfigurationForm,
    template: usize,
    path: PaneTemplateNodePath,
) -> &mut PaneTemplatePaneForm {
    match form.pane_templates.templates[template]
        .node
        .node_at_mut(path)
        .unwrap()
    {
        PaneTemplateNodeForm::Pane(pane) => pane,
        PaneTemplateNodeForm::Split { .. } => unreachable!(),
    }
}

#[test]
fn binding_form_exposes_string_action_parameters() {
    let binding = BindingForm {
        keystroke: TextField::new("alt-shift-o"),
        action: json!([
            "zetta::ApplyPaneSplitTemplate",
            { "name": "three-right" }
        ]),
    };

    assert_eq!(
        binding.action_parameter("name").as_deref(),
        Some("three-right")
    );
}

#[test]
fn pane_template_form_round_trip_covers_nested_leaf_options() {
    let root = settings_test_path("zetta-pane-template-recursive-form");
    let defaults = Config::defaults(Some(&root), None);
    let profile = defaults.profiles.first().unwrap().name.clone();
    let layout = json!({
        "vertical": [
            {
                "label": "server",
                "profile": profile,
                "theme": "One Dark",
                "dark_theme": "Solarized Dark",
                "env": { "ROLE": "server", "EMPTY": "" },
                "overlay": {
                    "text": "SERVER",
                    "size": "2xl",
                    "opacity": 85,
                    "color": "cyan"
                }
            },
            {
                "horizontal": [
                    {
                        "command": {
                            "program": "ssh",
                            "args": ["-o", "StrictHostKeyChecking=no", "host"]
                        }
                    },
                    {
                        "label": "client",
                        "env": { "ROLE": "client" },
                        "stack": [
                            { "program": "cargo", "args": ["watch", "-x", "test"] },
                            { "program": "tail" }
                        ]
                    }
                ]
            }
        ]
    });
    let template = json!({
        "env": { "PROJECT": "zetta" },
        "layout": layout
    });
    fs::write(
        &root,
        serde_json::to_string(&json!({
            "pane_split_templates": { "recursive": template.clone() }
        }))
        .unwrap(),
    )
    .unwrap();

    let config = Config::load(Some(&root), None).unwrap();
    let form = ConfigurationForm::load(&root, &config).unwrap();
    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    Config::parse(&serde_json::to_string(&output).unwrap(), Some(&root), None).unwrap();
    fs::remove_file(root).unwrap();

    assert_eq!(output["pane_split_templates"]["recursive"], template);
}

#[test]
fn pane_template_creation_and_duplication_are_immediate_and_unique() {
    let (mut form, _) = configuration_form_with_empty_custom_template();
    let second_empty = form.pane_templates.create_empty();
    assert_eq!(
        form.pane_templates.templates[second_empty].name.text,
        "custom-2"
    );
    assert_eq!(form.pane_templates.selected_template, second_empty);
    assert_eq!(
        form.pane_templates.templates[second_empty].node,
        PaneTemplateNodeForm::empty_two_pane()
    );

    for (name, pane_count) in [
        ("three-right", 3),
        ("three-left", 3),
        ("quarters", 4),
        ("four-vertical", 4),
    ] {
        let preset = form
            .pane_templates
            .templates
            .iter()
            .position(|template| template.name.text == name)
            .unwrap();
        form.pane_templates.select_template(preset);
        let expected = form.pane_templates.templates[preset].node.clone();
        let index = form.pane_templates.duplicate_selected().unwrap();
        assert_eq!(
            form.pane_templates.templates[index].node.pane_count(),
            pane_count
        );
        assert_eq!(form.pane_templates.templates[index].node, expected);
        assert!(form.pane_templates.templates[index].editable());
        assert_eq!(form.pane_templates.selected_template, index);
    }
    let duplicate = form.pane_templates.duplicate_selected().unwrap();
    assert!(
        form.pane_templates.templates[duplicate]
            .name
            .text
            .ends_with("-copy")
    );
    let mut names = form.pane_templates.names();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), form.pane_templates.templates.len());

    let root = settings_test_path("zetta-pane-template-override");
    fs::write(
        &root,
        serde_json::to_string(&json!({
            "pane_split_templates": {
                "three-right": { "layout": { "vertical": [{}, {}] } }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let config = Config::load(Some(&root), None).unwrap();
    let mut override_form = ConfigurationForm::load(&root, &config).unwrap();
    assert!(override_form.pane_templates.templates[0].editable());
    assert!(
        override_form
            .pane_templates
            .to_value()
            .unwrap()
            .get("three-right")
            .is_some()
    );
    override_form.pane_templates.select_template(0);
    override_form.pane_templates.delete_selected(false).unwrap();
    assert!(
        override_form
            .pane_templates
            .to_value()
            .unwrap()
            .get("three-right")
            .is_none()
    );
    fs::remove_file(root).unwrap();
}

#[test]
fn pane_template_global_environment_duplicates_and_validates() {
    let (mut form, index) = configuration_form_with_empty_custom_template();
    form.pane_templates.select_template(index);
    form.pane_templates.templates[index].environment = vec![PaneTemplateEnvironmentForm {
        name: TextField::new("PROJECT"),
        value: TextField::new("zetta"),
    }];

    let duplicate = form.pane_templates.duplicate_selected().unwrap();
    assert_eq!(
        form.pane_templates.templates[duplicate].environment,
        form.pane_templates.templates[index].environment
    );
    assert!(form.pane_templates.validate().is_ok());

    form.pane_templates.templates[duplicate]
        .environment
        .push(PaneTemplateEnvironmentForm {
            name: TextField::new("PROJECT"),
            value: TextField::new("duplicate"),
        });
    let error = form.pane_templates.validate().unwrap_err();
    assert!(format!("{error:#}").contains("duplicate environment key"));
}

#[test]
fn pane_template_tree_operations_preserve_bounds_and_collapse_parents() {
    let (mut form, index) = configuration_form_with_empty_custom_template();
    form.pane_templates.select_template(index);
    let left = PaneTemplateNodePath::ROOT.child(false).unwrap();
    form.pane_templates.select_node(left);
    assert!(form.pane_templates.toggle_node_selection(left));
    assert_eq!(
        form.pane_templates.selected_node,
        Some(PaneTemplateNodePath::ROOT)
    );
    assert!(
        form.pane_templates
            .split_selected_leaf(PaneSplitAxis::Horizontal)
            .is_err()
    );
    assert!(form.pane_templates.toggle_node_selection(left));
    assert_eq!(form.pane_templates.selected_node, Some(left));
    form.pane_templates
        .split_selected_leaf(PaneSplitAxis::Horizontal)
        .unwrap();
    assert_eq!(form.pane_templates.selected().unwrap().node.pane_count(), 3);

    let nested_leaf = left.child(true).unwrap();
    form.pane_templates.select_node(nested_leaf);
    form.pane_templates.remove_selected_node().unwrap();
    assert_eq!(form.pane_templates.selected().unwrap().node.pane_count(), 2);
    assert_eq!(form.pane_templates.selected_node, Some(left));

    form.pane_templates.select_node(PaneTemplateNodePath::ROOT);
    form.pane_templates
        .set_selected_axis(PaneSplitAxis::Horizontal)
        .unwrap();
    form.pane_templates.swap_selected_children().unwrap();
    assert!(matches!(
        form.pane_templates.selected_node().unwrap(),
        PaneTemplateNodeForm::Split {
            axis: PaneSplitAxis::Horizontal,
            ..
        }
    ));

    let max_index = form.pane_templates.create_empty();
    form.pane_templates.select_template(max_index);
    let mut rightmost = PaneTemplateNodePath::ROOT.child(true).unwrap();
    for _ in 0..62 {
        form.pane_templates.select_node(rightmost);
        form.pane_templates
            .split_selected_leaf(PaneSplitAxis::Vertical)
            .unwrap();
        rightmost = rightmost.child(true).unwrap();
    }
    assert_eq!(
        form.pane_templates.selected().unwrap().node.pane_count(),
        64
    );
    form.pane_templates.select_node(rightmost);
    assert!(
        form.pane_templates
            .split_selected_leaf(PaneSplitAxis::Vertical)
            .is_err()
    );

    let min_index = form.pane_templates.create_empty();
    form.pane_templates.select_template(min_index);
    form.pane_templates.select_node(left);
    form.pane_templates
        .split_selected_leaf(PaneSplitAxis::Vertical)
        .unwrap();
    form.pane_templates.select_node(left);
    assert!(form.pane_templates.remove_selected_node().is_err());
    assert_eq!(form.pane_templates.selected().unwrap().node.pane_count(), 3);
}

#[test]
fn pane_template_validation_reports_leaf_field_errors() {
    let (mut form, index) = configuration_form_with_empty_custom_template();
    form.pane_templates.select_template(index);
    let path = PaneTemplateNodePath::ROOT.child(false).unwrap();
    form.pane_templates.select_node(path);

    pane_for_test(&mut form, index, path).label = TextField::new("Bad Label");
    assert!(form.pane_templates.validate().is_err());
    {
        let pane = pane_for_test(&mut form, index, path);
        pane.label = TextField::new("good-label");
        pane.source = PaneTemplateSourceForm::Command(PaneTemplateCommandForm {
            program: TextField::default(),
            args: vec![TextField::new("--flag")],
        });
    }
    assert!(form.pane_templates.validate().is_err());
    {
        let pane = pane_for_test(&mut form, index, path);
        pane.source = PaneTemplateSourceForm::Inherit;
        pane.environment = vec![
            PaneTemplateEnvironmentForm {
                name: TextField::new("ROLE"),
                value: TextField::new("one"),
            },
            PaneTemplateEnvironmentForm {
                name: TextField::new("ROLE"),
                value: TextField::new("two"),
            },
        ];
    }
    assert!(form.pane_templates.validate().is_err());
    {
        let pane = pane_for_test(&mut form, index, path);
        pane.environment.clear();
        pane.overlay = Some(PaneTemplateOverlayForm {
            text: TextField::new("overlay"),
            size: Some(PaneSplitOverlaySize::Large),
            opacity: TextField::new("101"),
            color: TextField::new("cyan"),
        });
    }
    assert!(form.pane_templates.validate().is_err());
    {
        let pane = pane_for_test(&mut form, index, path);
        pane.overlay.as_mut().unwrap().opacity = TextField::new("80");
        pane.overlay.as_mut().unwrap().color = TextField::new("not-a-color");
    }
    assert!(form.pane_templates.validate().is_err());
    {
        let pane = pane_for_test(&mut form, index, path);
        pane.overlay = None;
        pane.stack = vec![PaneTemplateCommandForm {
            program: TextField::new("  "),
            args: vec![TextField::new("watch")],
        }];
    }
    let error = form.pane_templates.validate().unwrap_err();
    assert!(
        format!("{error:#}").contains("stacked command 1 program is required"),
        "unexpected stacked command error: {error:#}"
    );
    {
        let pane = pane_for_test(&mut form, index, path);
        pane.stack[0].program = TextField::new("cargo");
    }
    form.pane_templates.validate().unwrap();

    // Two panes plus 63 stacked commands is one terminal past what a tab holds.
    {
        let pane = pane_for_test(&mut form, index, path);
        pane.stack = vec![
            PaneTemplateCommandForm {
                program: TextField::new("true"),
                args: Vec::new(),
            };
            63
        ];
    }
    let error = form.pane_templates.validate().unwrap_err();
    assert!(
        format!("{error:#}").contains("panes and stacked commands combined"),
        "unexpected combined budget error: {error:#}"
    );
}

#[test]
fn the_root_pane_template_path_has_no_parent() {
    let root = PaneTemplateNodePath::ROOT;
    // Selecting the root split toggles its selection off, which asks the root
    // for its parent: computing `length - 1` there panicked in debug builds and
    // wrapped to a 255-deep path in release ones.
    assert_eq!(root.parent(), None);

    let left = root.child(false).unwrap();
    let left_right = left.child(true).unwrap();
    assert_eq!(left_right.parent(), Some(left));
    assert_eq!(left.parent(), Some(root));
    assert_eq!(left_right.depth(), 2);
}

#[test]
fn toggling_the_selected_root_node_clears_the_selection_without_panicking() {
    let (mut form, index) = configuration_form_with_empty_custom_template();
    form.pane_templates.select_template(index);
    assert_eq!(
        form.pane_templates.selected_node,
        Some(PaneTemplateNodePath::ROOT)
    );

    assert!(
        form.pane_templates
            .toggle_node_selection(PaneTemplateNodePath::ROOT)
    );
    assert_eq!(form.pane_templates.selected_node, None);

    // And selecting it again brings the split details back.
    assert!(
        form.pane_templates
            .toggle_node_selection(PaneTemplateNodePath::ROOT)
    );
    assert_eq!(
        form.pane_templates.selected_node,
        Some(PaneTemplateNodePath::ROOT)
    );
}
