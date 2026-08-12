use super::*;

use crate::settings_editor::{
    PaneTemplateForm, PaneTemplateNodeField, PaneTemplateNodeForm, PaneTemplateNodePath,
    PaneTemplatePaneForm, PaneTemplateSourceForm, PaneTemplateTextField,
};

// The preview represents the physical terminal viewport, not a grid of square
// cells. The reference viewport is 198 by 51 cells, with the observed cell
// width at roughly 40% of its height.
const TERMINAL_PREVIEW_ASPECT_RATIO: f32 = (198. / 51.) * 0.4;
const TERMINAL_PREVIEW_MAX_WIDTH: Pixels = px(540.);
const TERMINAL_PREVIEW_MAX_HEIGHT: Pixels = px(348.);

fn action_button(
    id: String,
    label: String,
    control: SettingsControl,
    focused: bool,
    enabled: bool,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> AnyElement {
    let click_handle = handle.clone();
    let click_control = control.clone();
    div()
        .id(id)
        .h_8()
        .px_2()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .border_1()
        .border_color(if focused {
            colors.border_focused
        } else {
            colors.border
        })
        .text_xs()
        .when(enabled, |button| {
            button
                .cursor_pointer()
                .hover(|style| style.bg(colors.element_hover))
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
        })
        .when(!enabled, |button| button.opacity(0.5))
        .when(focused, |button| button.bg(colors.element_selected))
        .child(label)
        .into_any_element()
}

fn field_row(label: impl Into<String>, control: AnyElement, colors: &ThemeColors) -> AnyElement {
    h_flex()
        .w_full()
        .min_h(px(42.))
        .gap_3()
        .justify_between()
        .border_b_1()
        .border_color(colors.border_variant)
        .child(div().min_w_0().flex_1().text_xs().child(label.into()))
        .child(div().w(px(300.)).flex_none().child(control))
        .into_any_element()
}

fn text_field(
    id: String,
    field: TextField,
    input: SettingsInput,
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> AnyElement {
    Zetta::text_input_widget(
        id,
        field,
        input,
        editor.focused_input,
        colors.clone(),
        handle.clone(),
    )
}

fn dropdown_field(
    id: String,
    label: String,
    selection: SettingsDropdown,
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> AnyElement {
    Zetta::dropdown_trigger_widget(
        id,
        label,
        selection,
        editor.focused_control == Some(SettingsControl::Dropdown(selection)),
        colors.clone(),
        handle.clone(),
    )
}

fn render_tree_node(
    editor: &SettingsEditor,
    node: &PaneTemplateNodeForm,
    path: PaneTemplateNodePath,
    pane_number: &mut usize,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> AnyElement {
    let selected = editor.configuration.pane_templates.selected_node == Some(path);
    let click_handle = handle.clone();
    let click_control = SettingsControl::SelectPaneTemplateNode(path);
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
            div()
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
            div()
                .id(format!("pane-template-node-{path:?}"))
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
            format!("pane-template-swap-{path:?}"),
            "Swap children".to_owned(),
            SettingsControl::SwapPaneTemplateChildren(path),
            editor.focused_control == Some(SettingsControl::SwapPaneTemplateChildren(path)),
            true,
            colors,
            handle,
        ));
        if !path.is_root() {
            actions.push(action_button(
                format!("pane-template-remove-{path:?}"),
                "Remove split".to_owned(),
                SettingsControl::RemovePaneTemplateNode(path),
                editor.focused_control == Some(SettingsControl::RemovePaneTemplateNode(path)),
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
        .child(field_row("Orientation", orientation, colors))
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
            format!("pane-template-split-horizontal-{path:?}"),
            "Split horizontally".to_owned(),
            SettingsControl::SplitPaneTemplate(path, PaneSplitAxis::Horizontal),
            editor.focused_control
                == Some(SettingsControl::SplitPaneTemplate(
                    path,
                    PaneSplitAxis::Horizontal,
                )),
            editable,
            colors,
            handle,
        ),
        action_button(
            format!("pane-template-split-vertical-{path:?}"),
            "Split vertically".to_owned(),
            SettingsControl::SplitPaneTemplate(path, PaneSplitAxis::Vertical),
            editor.focused_control
                == Some(SettingsControl::SplitPaneTemplate(
                    path,
                    PaneSplitAxis::Vertical,
                )),
            editable,
            colors,
            handle,
        ),
    ];
    if !path.is_root() {
        tree_actions.push(action_button(
            format!("pane-template-remove-leaf-{path:?}"),
            "Remove pane".to_owned(),
            SettingsControl::RemovePaneTemplateNode(path),
            editor.focused_control == Some(SettingsControl::RemovePaneTemplateNode(path)),
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
    rows.push(field_row(
        "Label (lowercase kebab-case; empty means none)",
        label,
        colors,
    ));
    let source_label = match &pane.source {
        PaneTemplateSourceForm::Inherit => "Inherited".to_owned(),
        PaneTemplateSourceForm::Profile(profile) => profile.clone(),
        PaneTemplateSourceForm::Command(_) => "Direct command".to_owned(),
    };
    rows.push(field_row(
        "Profile or command",
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
    rows.push(field_row(
        "Theme override",
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
        rows.push(field_row("Command program", program, colors));
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
                format!("pane-template-command-arg-remove-{path:?}-{argument}"),
                "×".to_owned(),
                SettingsControl::RemovePaneTemplateArgument(path, argument),
                editor.focused_control
                    == Some(SettingsControl::RemovePaneTemplateArgument(path, argument)),
                editable,
                colors,
                handle,
            );
            rows.push(field_row(
                format!("Argument {}", argument + 1),
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
                    format!("pane-template-command-arg-add-{path:?}"),
                    "Add argument".to_owned(),
                    SettingsControl::AddPaneTemplateArgument(path),
                    editor.focused_control == Some(SettingsControl::AddPaneTemplateArgument(path)),
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
            format!("pane-template-env-remove-{path:?}-{environment}"),
            "×".to_owned(),
            SettingsControl::RemovePaneTemplateEnvironment(path, environment),
            editor.focused_control
                == Some(SettingsControl::RemovePaneTemplateEnvironment(
                    path,
                    environment,
                )),
            editable,
            colors,
            handle,
        );
        rows.push(field_row(
            format!("Environment {} · name", environment + 1),
            h_flex()
                .gap_1()
                .child(name)
                .child(remove)
                .into_any_element(),
            colors,
        ));
        rows.push(field_row(
            format!("Environment {} · value", environment + 1),
            value,
            colors,
        ));
    }
    rows.push(
        h_flex()
            .justify_end()
            .child(action_button(
                format!("pane-template-env-add-{path:?}"),
                "Add environment variable".to_owned(),
                SettingsControl::AddPaneTemplateEnvironment(path),
                editor.focused_control == Some(SettingsControl::AddPaneTemplateEnvironment(path)),
                editable,
                colors,
                handle,
            ))
            .into_any_element(),
    );

    let overlay_toggle = action_button(
        format!("pane-template-overlay-toggle-{path:?}"),
        if pane.overlay.is_some() {
            "Remove overlay".to_owned()
        } else {
            "Add overlay".to_owned()
        },
        SettingsControl::TogglePaneTemplateOverlay(path),
        editor.focused_control == Some(SettingsControl::TogglePaneTemplateOverlay(path)),
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
        rows.push(field_row(
            "Overlay text",
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
        rows.push(field_row(
            "Overlay size",
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
        rows.push(field_row(
            "Overlay opacity (0–100)",
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
        rows.push(field_row(
            "Overlay color (name or hex)",
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
            format!("pane-template-global-env-remove-{environment}"),
            "×".to_owned(),
            SettingsControl::RemovePaneTemplateGlobalEnvironment(environment),
            editor.focused_control
                == Some(SettingsControl::RemovePaneTemplateGlobalEnvironment(
                    environment,
                )),
            editable,
            colors,
            handle,
        );
        rows.push(field_row(
            format!("Variable {} · name", environment + 1),
            h_flex()
                .gap_1()
                .child(name)
                .child(remove)
                .into_any_element(),
            colors,
        ));
        rows.push(field_row(
            format!("Variable {} · value", environment + 1),
            value,
            colors,
        ));
    }
    rows.push(
        h_flex()
            .justify_end()
            .child(action_button(
                "pane-template-global-env-add".to_owned(),
                "Add environment variable".to_owned(),
                SettingsControl::AddPaneTemplateGlobalEnvironment,
                editor.focused_control == Some(SettingsControl::AddPaneTemplateGlobalEnvironment),
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
    let pane_templates = &editor.configuration.pane_templates;
    let selected_index = pane_templates.selected_template;
    let selected = pane_templates.selected();
    let editable = selected.is_some_and(|template| template.editable());

    let mut list: Vec<AnyElement> = Vec::new();
    for (index, template) in pane_templates.templates.iter().enumerate() {
        let selected_row = index == selected_index;
        let select_handle = handle.clone();
        let control = SettingsControl::SelectPaneTemplate(index);
        let name = template.name.text.clone();
        let label = if template.is_pristine_built_in() {
            format!("{name} · built-in")
        } else if template.built_in {
            format!("{name} · override")
        } else {
            name.clone()
        };
        list.push(
            div()
                .id(format!("pane-template-list-{index}"))
                .w_full()
                .px_2()
                .py_2()
                .rounded(px(4.))
                .cursor_pointer()
                .border_1()
                .border_color(if selected_row {
                    colors.border_focused
                } else {
                    colors.border_variant
                })
                .when(selected_row, |row| row.bg(colors.element_selected))
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
                "pane-template-new".to_owned(),
                "New".to_owned(),
                SettingsControl::NewPaneTemplate,
                editor.focused_control == Some(SettingsControl::NewPaneTemplate),
                true,
                colors,
                handle,
            ))
            .child(action_button(
                "pane-template-duplicate".to_owned(),
                "Duplicate".to_owned(),
                SettingsControl::DuplicatePaneTemplate,
                editor.focused_control == Some(SettingsControl::DuplicatePaneTemplate),
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
            "pane-template-delete".to_owned(),
            if template.built_in {
                "Reset override"
            } else {
                "Delete"
            }
            .to_owned(),
            SettingsControl::DeletePaneTemplate,
            editor.focused_control == Some(SettingsControl::DeletePaneTemplate),
            editable,
            colors,
            handle,
        );
        let mut content = vec![field_row(
            "Template name",
            h_flex()
                .gap_2()
                .child(name)
                .child(delete)
                .into_any_element(),
            colors,
        )];
        if template.is_pristine_built_in() {
            content.push(
                div()
                    .py_3()
                    .text_xs()
                    .text_color(colors.text_muted)
                    .child(
                        "This built-in layout is a read-only preset. Duplicate it to edit a copy.",
                    )
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
