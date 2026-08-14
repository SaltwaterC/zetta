use super::*;

use crate::settings_editor::{
    PaneTemplateForm, PaneTemplateNodeField, PaneTemplateNodeForm, PaneTemplateNodePath,
    PaneTemplatePaneForm, PaneTemplateSourceForm, PaneTemplateTextField,
};
use crate::settings_ui::pane_templates::templates;
use crate::settings_ui::project_editor;

/// The focus ring for the template list and the layout preview.
///
/// Both surfaces already use their background to show what is *selected*, which
/// is a different thing from what the keyboard is on, so focus is drawn as an
/// overlay border instead: it reads clearly over either background and, being
/// absolutely positioned, never reflows the row or the pane it highlights.
fn focus_ring(colors: &ThemeColors) -> Div {
    div()
        .absolute()
        .inset_0()
        .border_2()
        .border_color(colors.border_focused)
}

// The preview represents the physical terminal viewport, not a grid of square
// cells. The reference viewport is 198 by 51 cells, with the observed cell
// width at roughly 40% of its height.
const TERMINAL_PREVIEW_ASPECT_RATIO: f32 = (198. / 51.) * 0.4;
const TERMINAL_PREVIEW_MAX_WIDTH: Pixels = px(540.);
const TERMINAL_PREVIEW_MAX_HEIGHT: Pixels = px(348.);

fn render_tree_node(
    editor: &SettingsEditor,
    node: &PaneTemplateNodeForm,
    path: PaneTemplateNodePath,
    pane_number: &mut usize,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> AnyElement {
    let selected = templates(editor).selected_node == Some(path);
    let node_control = SettingsControl::SelectPaneTemplateNode(path);
    let focused = editor.focused_control.as_ref() == Some(&node_control);
    let click_handle = handle.clone();
    let click_control = node_control.clone();
    match node {
        PaneTemplateNodeForm::Split {
            axis,
            first,
            second,
        } => {
            let horizontal = matches!(axis, PaneSplitAxis::Horizontal);
            let first_node = render_tree_node(
                editor,
                first,
                path.child(false).unwrap_or(path),
                pane_number,
                colors,
                handle,
            );
            let second_node = render_tree_node(
                editor,
                second,
                path.child(true).unwrap_or(path),
                pane_number,
                colors,
                handle,
            );
            let first_child = div()
                .min_w_0()
                .min_h_0()
                .flex_grow_1()
                .flex_basis(gpui::relative(0.))
                .overflow_hidden()
                .child(first_node);
            let second_child = div()
                .min_w_0()
                .min_h_0()
                .flex_grow_1()
                .flex_basis(gpui::relative(0.))
                .overflow_hidden()
                .child(second_node);
            let divider = div()
                .id(format!("pane-template-divider-{path:?}"))
                .absolute()
                .when(horizontal, |divider| {
                    divider
                        .top(gpui::relative(0.5))
                        .mt(px(-4.))
                        .h(px(8.))
                        .w_full()
                })
                .when(!horizontal, |divider| {
                    divider
                        .left(gpui::relative(0.5))
                        .ml(px(-4.))
                        .w(px(8.))
                        .h_full()
                })
                .cursor_pointer()
                .hover(|divider| divider.bg(colors.element_hover.opacity(0.35)))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    click_handle
                        .update(cx, |this, cx| {
                            this.focus_settings_control_without_scroll(
                                click_control.clone(),
                                window,
                                cx,
                            );
                            this.activate_settings_control(click_control.clone(), window, cx);
                        })
                        .ok();
                });
            track_focus_scroll(div(), editor, std::slice::from_ref(&node_control))
                .id(format!("pane-template-node-{path:?}"))
                .relative()
                .size_full()
                .flex()
                .min_h_0()
                .min_w_0()
                .flex_grow_1()
                .flex_basis(gpui::relative(0.))
                .overflow_hidden()
                .gap_px()
                .when(horizontal, |split| split.flex_col())
                .bg(if selected {
                    colors.border_focused
                } else {
                    colors.border
                })
                .child(first_child)
                .child(second_child)
                .child(divider)
                .when(focused, |split| split.child(focus_ring(colors)))
                .into_any_element()
        }
        PaneTemplateNodeForm::Pane(pane) => {
            let current_pane = *pane_number;
            *pane_number += 1;
            let label = if pane.label.text.is_empty() {
                "Unlabeled".to_owned()
            } else {
                pane.label.text.clone()
            };
            track_focus_scroll(div(), editor, std::slice::from_ref(&node_control))
                .id(format!("pane-template-node-{path:?}"))
                .relative()
                .size_full()
                .min_w_0()
                .min_h_0()
                .overflow_hidden()
                .flex()
                .flex_col()
                .p_2()
                .bg(if selected {
                    colors.element_selected
                } else {
                    colors.editor_background
                })
                .cursor_pointer()
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    click_handle
                        .update(cx, |this, cx| {
                            this.focus_settings_control_without_scroll(
                                click_control.clone(),
                                window,
                                cx,
                            );
                            this.activate_settings_control(click_control.clone(), window, cx);
                        })
                        .ok();
                })
                .child(
                    div()
                        .min_w_0()
                        .text_xs()
                        .text_color(colors.text_muted)
                        .truncate()
                        .child(format!("Pane {current_pane}")),
                )
                .child(div().mt_1().min_w_0().text_sm().truncate().child(label))
                .when(focused, |pane| pane.child(focus_ring(colors)))
                .into_any_element()
        }
    }
}

fn render_split_details(
    editor: &SettingsEditor,
    axis: PaneSplitAxis,
    pane_count: usize,
    path: PaneTemplateNodePath,
    editable: bool,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> AnyElement {
    let orientation = if editable {
        dropdown_field(
            format!("pane-template-axis-{path:?}"),
            axis.label().to_owned(),
            SettingsDropdown::PaneTemplateAxis(path),
            editor,
            colors,
            handle,
        )
    } else {
        div()
            .h_9()
            .flex()
            .items_center()
            .px_2()
            .child(axis.label())
            .into_any_element()
    };
    let mut actions = Vec::new();
    if editable {
        actions.push(action_button(
            editor,
            format!("pane-template-swap-{path:?}"),
            "Swap children".to_owned(),
            SettingsControl::SwapPaneTemplateChildren(path),
            true,
            colors,
            handle,
        ));
        if !path.is_root() {
            actions.push(action_button(
                editor,
                format!("pane-template-remove-{path:?}"),
                "Remove split".to_owned(),
                SettingsControl::RemovePaneTemplateNode(path),
                true,
                colors,
                handle,
            ));
        }
    }

    v_flex()
        .mt_3()
        .child(
            h_flex()
                .mb_2()
                .justify_between()
                .child(div().text_sm().child("Selected split"))
                .child(
                    div()
                        .text_xs()
                        .text_color(colors.text_muted)
                        .child(format!("{pane_count} panes")),
                ),
        )
        .child(control_row(
            editor,
            "Orientation",
            &[SettingsControl::Dropdown(
                SettingsDropdown::PaneTemplateAxis(path),
            )],
            orientation,
            colors,
        ))
        .when(!actions.is_empty(), |details| {
            details.child(h_flex().mt_2().gap_2().flex_wrap().children(actions))
        })
        .into_any_element()
}

fn render_pane_details(
    editor: &SettingsEditor,
    pane: &PaneTemplatePaneForm,
    template_index: usize,
    path: PaneTemplateNodePath,
    editable: bool,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> AnyElement {
    let mut rows = Vec::new();
    // Every text field in this section addresses the same node, so its control
    // identity differs only by which field it edits.
    let node_input = |field| {
        SettingsControl::Input(SettingsInput::PaneTemplate(PaneTemplateTextField::Node(
            template_index,
            path,
            field,
        )))
    };
    let label = text_field(
        format!("pane-template-label-{path:?}"),
        pane.label.clone(),
        SettingsInput::PaneTemplate(PaneTemplateTextField::Node(
            template_index,
            path,
            PaneTemplateNodeField::Label,
        )),
        editor,
        colors,
        handle,
    );
    let mut tree_actions = vec![
        action_button(
            editor,
            format!("pane-template-split-horizontal-{path:?}"),
            "Split horizontally".to_owned(),
            SettingsControl::SplitPaneTemplate(path, PaneSplitAxis::Horizontal),
            editable,
            colors,
            handle,
        ),
        action_button(
            editor,
            format!("pane-template-split-vertical-{path:?}"),
            "Split vertically".to_owned(),
            SettingsControl::SplitPaneTemplate(path, PaneSplitAxis::Vertical),
            editable,
            colors,
            handle,
        ),
    ];
    if !path.is_root() {
        tree_actions.push(action_button(
            editor,
            format!("pane-template-remove-leaf-{path:?}"),
            "Remove pane".to_owned(),
            SettingsControl::RemovePaneTemplateNode(path),
            editable,
            colors,
            handle,
        ));
    }
    rows.push(
        h_flex()
            .gap_2()
            .flex_wrap()
            .children(tree_actions)
            .into_any_element(),
    );
    rows.push(control_row(
        editor,
        "Label (lowercase kebab-case; empty means none)",
        &[node_input(PaneTemplateNodeField::Label)],
        label,
        colors,
    ));
    let source_label = match &pane.source {
        PaneTemplateSourceForm::Inherit => "Inherited".to_owned(),
        PaneTemplateSourceForm::Profile(profile) => profile.clone(),
        PaneTemplateSourceForm::Command(_) => "Direct command".to_owned(),
    };
    rows.push(control_row(
        editor,
        "Profile or command",
        &[SettingsControl::Dropdown(
            SettingsDropdown::PaneTemplateSource(path),
        )],
        dropdown_field(
            format!("pane-template-source-{path:?}"),
            source_label,
            SettingsDropdown::PaneTemplateSource(path),
            editor,
            colors,
            handle,
        ),
        colors,
    ));
    let theme_label = pane
        .theme
        .clone()
        .unwrap_or_else(|| "Use profile/application theme".to_owned());
    rows.push(control_row(
        editor,
        "Theme override",
        &[SettingsControl::Dropdown(
            SettingsDropdown::PaneTemplateTheme(path),
        )],
        dropdown_field(
            format!("pane-template-theme-{path:?}"),
            theme_label,
            SettingsDropdown::PaneTemplateTheme(path),
            editor,
            colors,
            handle,
        ),
        colors,
    ));

    if let PaneTemplateSourceForm::Command(command) = &pane.source {
        let program = text_field(
            format!("pane-template-command-program-{path:?}"),
            command.program.clone(),
            SettingsInput::PaneTemplate(PaneTemplateTextField::Node(
                template_index,
                path,
                PaneTemplateNodeField::CommandProgram,
            )),
            editor,
            colors,
            handle,
        );
        rows.push(control_row(
            editor,
            "Command program",
            &[node_input(PaneTemplateNodeField::CommandProgram)],
            program,
            colors,
        ));
        for (argument, value) in command.args.iter().enumerate() {
            let argument_input = text_field(
                format!("pane-template-command-arg-{path:?}-{argument}"),
                value.clone(),
                SettingsInput::PaneTemplate(PaneTemplateTextField::Node(
                    template_index,
                    path,
                    PaneTemplateNodeField::CommandArgument(argument),
                )),
                editor,
                colors,
                handle,
            );
            let remove = action_button(
                editor,
                format!("pane-template-command-arg-remove-{path:?}-{argument}"),
                "×".to_owned(),
                SettingsControl::RemovePaneTemplateArgument(path, argument),
                editable,
                colors,
                handle,
            );
            rows.push(control_row(
                editor,
                format!("Argument {}", argument + 1),
                &[
                    node_input(PaneTemplateNodeField::CommandArgument(argument)),
                    SettingsControl::RemovePaneTemplateArgument(path, argument),
                ],
                h_flex()
                    .gap_1()
                    .child(argument_input)
                    .child(remove)
                    .into_any_element(),
                colors,
            ));
        }
        rows.push(
            h_flex()
                .justify_end()
                .child(action_button(
                    editor,
                    format!("pane-template-command-arg-add-{path:?}"),
                    "Add argument".to_owned(),
                    SettingsControl::AddPaneTemplateArgument(path),
                    editable,
                    colors,
                    handle,
                ))
                .into_any_element(),
        );
    }

    rows.push(
        div()
            .pt_3()
            .text_xs()
            .text_color(colors.text_muted)
            .child("Pane environment overrides")
            .into_any_element(),
    );
    for (environment, entry) in pane.environment.iter().enumerate() {
        let name = text_field(
            format!("pane-template-env-name-{path:?}-{environment}"),
            entry.name.clone(),
            SettingsInput::PaneTemplate(PaneTemplateTextField::Node(
                template_index,
                path,
                PaneTemplateNodeField::EnvironmentName(environment),
            )),
            editor,
            colors,
            handle,
        );
        let value = text_field(
            format!("pane-template-env-value-{path:?}-{environment}"),
            entry.value.clone(),
            SettingsInput::PaneTemplate(PaneTemplateTextField::Node(
                template_index,
                path,
                PaneTemplateNodeField::EnvironmentValue(environment),
            )),
            editor,
            colors,
            handle,
        );
        let remove = action_button(
            editor,
            format!("pane-template-env-remove-{path:?}-{environment}"),
            "×".to_owned(),
            SettingsControl::RemovePaneTemplateEnvironment(path, environment),
            editable,
            colors,
            handle,
        );
        rows.push(control_row(
            editor,
            format!("Environment {} · name", environment + 1),
            &[
                node_input(PaneTemplateNodeField::EnvironmentName(environment)),
                SettingsControl::RemovePaneTemplateEnvironment(path, environment),
            ],
            h_flex()
                .gap_1()
                .child(name)
                .child(remove)
                .into_any_element(),
            colors,
        ));
        rows.push(control_row(
            editor,
            format!("Environment {} · value", environment + 1),
            &[node_input(PaneTemplateNodeField::EnvironmentValue(
                environment,
            ))],
            value,
            colors,
        ));
    }
    rows.push(
        h_flex()
            .justify_end()
            .child(action_button(
                editor,
                format!("pane-template-env-add-{path:?}"),
                "Add environment variable".to_owned(),
                SettingsControl::AddPaneTemplateEnvironment(path),
                editable,
                colors,
                handle,
            ))
            .into_any_element(),
    );

    let overlay_toggle = action_button(
        editor,
        format!("pane-template-overlay-toggle-{path:?}"),
        if pane.overlay.is_some() {
            "Remove overlay".to_owned()
        } else {
            "Add overlay".to_owned()
        },
        SettingsControl::TogglePaneTemplateOverlay(path),
        editable,
        colors,
        handle,
    );
    rows.push(
        h_flex()
            .justify_between()
            .py_2()
            .child(div().text_xs().child("Overlay"))
            .child(overlay_toggle)
            .into_any_element(),
    );
    if let Some(overlay) = &pane.overlay {
        rows.push(control_row(
            editor,
            "Overlay text",
            &[node_input(PaneTemplateNodeField::OverlayText)],
            text_field(
                format!("pane-template-overlay-text-{path:?}"),
                overlay.text.clone(),
                SettingsInput::PaneTemplate(PaneTemplateTextField::Node(
                    template_index,
                    path,
                    PaneTemplateNodeField::OverlayText,
                )),
                editor,
                colors,
                handle,
            ),
            colors,
        ));
        let size_label = overlay
            .size
            .map(|size| size.label().to_owned())
            .unwrap_or_else(|| "Default".to_owned());
        rows.push(control_row(
            editor,
            "Overlay size",
            &[SettingsControl::Dropdown(
                SettingsDropdown::PaneTemplateOverlaySize(path),
            )],
            dropdown_field(
                format!("pane-template-overlay-size-{path:?}"),
                size_label,
                SettingsDropdown::PaneTemplateOverlaySize(path),
                editor,
                colors,
                handle,
            ),
            colors,
        ));
        rows.push(control_row(
            editor,
            "Overlay opacity (0–100)",
            &[node_input(PaneTemplateNodeField::OverlayOpacity)],
            text_field(
                format!("pane-template-overlay-opacity-{path:?}"),
                overlay.opacity.clone(),
                SettingsInput::PaneTemplate(PaneTemplateTextField::Node(
                    template_index,
                    path,
                    PaneTemplateNodeField::OverlayOpacity,
                )),
                editor,
                colors,
                handle,
            ),
            colors,
        ));
        rows.push(control_row(
            editor,
            "Overlay color (name or hex)",
            &[node_input(PaneTemplateNodeField::OverlayColor)],
            text_field(
                format!("pane-template-overlay-color-{path:?}"),
                overlay.color.clone(),
                SettingsInput::PaneTemplate(PaneTemplateTextField::Node(
                    template_index,
                    path,
                    PaneTemplateNodeField::OverlayColor,
                )),
                editor,
                colors,
                handle,
            ),
            colors,
        ));
    }
    div().flex_col().children(rows).into_any_element()
}

fn render_global_environment(
    editor: &SettingsEditor,
    template: &PaneTemplateForm,
    template_index: usize,
    editable: bool,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> AnyElement {
    let mut rows = vec![
        div()
            .mt_3()
            .mb_1()
            .text_sm()
            .child("All panes environment")
            .into_any_element(),
        div()
            .mb_2()
            .text_xs()
            .text_color(colors.text_muted)
            .child("These variables are applied to every pane. Pane-specific values override matching keys.")
            .into_any_element(),
    ];
    for (environment, entry) in template.environment.iter().enumerate() {
        let name = text_field(
            format!("pane-template-global-env-name-{environment}"),
            entry.name.clone(),
            SettingsInput::PaneTemplate(PaneTemplateTextField::GlobalEnvironmentName(
                template_index,
                environment,
            )),
            editor,
            colors,
            handle,
        );
        let value = text_field(
            format!("pane-template-global-env-value-{environment}"),
            entry.value.clone(),
            SettingsInput::PaneTemplate(PaneTemplateTextField::GlobalEnvironmentValue(
                template_index,
                environment,
            )),
            editor,
            colors,
            handle,
        );
        let remove = action_button(
            editor,
            format!("pane-template-global-env-remove-{environment}"),
            "×".to_owned(),
            SettingsControl::RemovePaneTemplateGlobalEnvironment(environment),
            editable,
            colors,
            handle,
        );
        rows.push(control_row(
            editor,
            format!("Variable {} · name", environment + 1),
            &[
                SettingsControl::Input(SettingsInput::PaneTemplate(
                    PaneTemplateTextField::GlobalEnvironmentName(template_index, environment),
                )),
                SettingsControl::RemovePaneTemplateGlobalEnvironment(environment),
            ],
            h_flex()
                .gap_1()
                .child(name)
                .child(remove)
                .into_any_element(),
            colors,
        ));
        rows.push(control_row(
            editor,
            format!("Variable {} · value", environment + 1),
            &[SettingsControl::Input(SettingsInput::PaneTemplate(
                PaneTemplateTextField::GlobalEnvironmentValue(template_index, environment),
            ))],
            value,
            colors,
        ));
    }
    rows.push(
        h_flex()
            .justify_end()
            .child(action_button(
                editor,
                "pane-template-global-env-add".to_owned(),
                "Add environment variable".to_owned(),
                SettingsControl::AddPaneTemplateGlobalEnvironment,
                editable,
                colors,
                handle,
            ))
            .into_any_element(),
    );
    div().flex_col().children(rows).into_any_element()
}

pub(crate) fn render_pane_templates_page(
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> AnyElement {
    // Whichever form the editor is pointed at: the user configuration on the
    // Templates page, or the open project's overlay in the Projects builder.
    let pane_templates = templates(editor);
    let selected_index = pane_templates.selected_template;
    let selected = pane_templates.selected();
    let editable = selected.is_some_and(|template| template.editable());

    // A project's inherited layer is the whole user configuration, not just the
    // four built-in presets, so the read-only rows are labelled for the layer
    // the form actually overlays.
    let inherited_label = if project_editor(editor).is_some() {
        "inherited"
    } else {
        "built-in"
    };
    let mut list: Vec<AnyElement> = Vec::new();
    for (index, template) in pane_templates.templates.iter().enumerate() {
        let selected_row = index == selected_index;
        let focused_row =
            editor.focused_control == Some(SettingsControl::SelectPaneTemplate(index));
        let select_handle = handle.clone();
        let control = SettingsControl::SelectPaneTemplate(index);
        let name = template.name.text.clone();
        let label = if template.is_pristine_inherited() {
            format!("{name} · {inherited_label}")
        } else if template.inherited() {
            format!("{name} · override")
        } else {
            name.clone()
        };
        list.push(
            track_focus_scroll(div(), editor, std::slice::from_ref(&control))
                .id(format!("pane-template-list-{index}"))
                .w_full()
                .px_2()
                .py_2()
                .rounded(px(4.))
                .cursor_pointer()
                .border_1()
                .border_color(if focused_row {
                    colors.border_focused
                } else if selected_row {
                    colors.border_selected
                } else {
                    colors.border_variant
                })
                .when(selected_row || focused_row, |row| {
                    row.bg(colors.element_selected)
                })
                .on_click(move |_, window, cx| {
                    select_handle
                        .update(cx, |this, cx| {
                            this.focus_settings_control_without_scroll(control.clone(), window, cx);
                            this.activate_settings_control(control.clone(), window, cx);
                        })
                        .ok();
                })
                .child(div().text_xs().child(label))
                .child(
                    div()
                        .text_xs()
                        .text_color(colors.text_muted)
                        .child(format!("{} panes", template.node.pane_count())),
                )
                .into_any_element(),
        );
    }
    list.push(
        h_flex()
            .gap_2()
            .child(action_button(
                editor,
                "pane-template-new".to_owned(),
                "New".to_owned(),
                SettingsControl::NewPaneTemplate,
                true,
                colors,
                handle,
            ))
            .child(action_button(
                editor,
                "pane-template-duplicate".to_owned(),
                "Duplicate".to_owned(),
                SettingsControl::DuplicatePaneTemplate,
                selected.is_some(),
                colors,
                handle,
            ))
            .into_any_element(),
    );

    let details = if let Some(template) = selected {
        let name = if editable {
            text_field(
                "pane-template-name".to_owned(),
                template.name.clone(),
                SettingsInput::PaneTemplate(PaneTemplateTextField::Name(selected_index)),
                editor,
                colors,
                handle,
            )
        } else {
            div()
                .h_9()
                .flex()
                .items_center()
                .px_2()
                .child(template.name.text.clone())
                .into_any_element()
        };
        let delete = action_button(
            editor,
            "pane-template-delete".to_owned(),
            if template.inherited() {
                "Reset override"
            } else {
                "Delete"
            }
            .to_owned(),
            SettingsControl::DeletePaneTemplate,
            editable,
            colors,
            handle,
        );
        let mut content = vec![control_row(
            editor,
            "Template name",
            &[
                SettingsControl::Input(SettingsInput::PaneTemplate(PaneTemplateTextField::Name(
                    selected_index,
                ))),
                SettingsControl::DeletePaneTemplate,
            ],
            h_flex()
                .gap_2()
                .child(name)
                .child(delete)
                .into_any_element(),
            colors,
        )];
        if template.is_pristine_inherited() {
            content.push(
                div()
                    .py_3()
                    .text_xs()
                    .text_color(colors.text_muted)
                    .child(format!(
                        "This {inherited_label} layout is read-only here. Duplicate it to edit a copy."
                    ))
                    .into_any_element(),
            );
        }
        content.push(render_global_environment(
            editor,
            template,
            selected_index,
            editable,
            colors,
            handle,
        ));
        content.push(
            div()
                .mt_3()
                .mb_2()
                .text_sm()
                .child("Layout preview")
                .into_any_element(),
        );
        content.push(
            div()
                .mb_2()
                .text_xs()
                .text_color(colors.text_muted)
                .child(
                    "Select a pane to edit it. Select it again to return to its containing split.",
                )
                .into_any_element(),
        );
        let mut pane_number = 1;
        let tree = render_tree_node(
            editor,
            &template.node,
            PaneTemplateNodePath::ROOT,
            &mut pane_number,
            colors,
            handle,
        );
        content.push(
            div()
                .w_full()
                .aspect_ratio(TERMINAL_PREVIEW_ASPECT_RATIO)
                .max_w(TERMINAL_PREVIEW_MAX_WIDTH)
                .max_h(TERMINAL_PREVIEW_MAX_HEIGHT)
                .mx_auto()
                .overflow_hidden()
                .rounded(px(4.))
                .border_1()
                .border_color(colors.border)
                .child(tree)
                .into_any_element(),
        );
        if let Some(path) = pane_templates.selected_node
            && let Some(node) = pane_templates.selected_node()
        {
            match node {
                PaneTemplateNodeForm::Pane(pane) => content.push(
                    div()
                        .mt_3()
                        .child(div().mb_2().text_sm().child("Selected pane details"))
                        .child(render_pane_details(
                            editor,
                            pane,
                            selected_index,
                            path,
                            editable,
                            colors,
                            handle,
                        ))
                        .into_any_element(),
                ),
                PaneTemplateNodeForm::Split {
                    axis,
                    first,
                    second,
                } => content.push(render_split_details(
                    editor,
                    *axis,
                    first.pane_count() + second.pane_count(),
                    path,
                    editable,
                    colors,
                    handle,
                )),
            }
        }
        if let Some(error) = editor.pane_template_validation_error.as_ref() {
            content.push(
                div()
                    .mt_3()
                    .p_2()
                    .rounded(px(4.))
                    .text_xs()
                    .text_color(colors.text)
                    .bg(colors.editor_background)
                    .child(format!("Validation: {error}"))
                    .into_any_element(),
            );
        }
        div().flex_col().children(content).into_any_element()
    } else {
        div()
            .text_sm()
            .text_color(colors.text_muted)
            .child("No pane templates available.")
            .into_any_element()
    };

    h_flex()
        .w_full()
        .items_start()
        .gap_4()
        .child(
            div()
                .w(px(210.))
                .flex_none()
                .flex_col()
                .gap_2()
                .child(div().text_sm().child("Templates"))
                .children(list),
        )
        .child(div().min_w_0().flex_1().child(details))
        .into_any_element()
}
