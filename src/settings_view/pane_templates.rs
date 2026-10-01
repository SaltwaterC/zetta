use super::*;
use crate::ui_tokens::RADIUS_CONTROL;

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
                .hover(|divider| divider.bg(colors.element_hover))
                .on_click(activate_on_click(handle, node_control.clone()));
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
                // The lines between a selected split's children, in the
                // selection colour rather than the focus ring's, which drew the
                // two identically.
                .bg(if selected {
                    colors.border_selected
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
                .on_click(activate_on_click(handle, node_control.clone()))
                .child(
                    div()
                        .min_w_0()
                        .text_xs()
                        .text_color(colors.text_muted)
                        .truncate()
                        .child(format!("Pane {current_pane}")),
                )
                .child(div().mt_1().min_w_0().text_sm().truncate().child(label))
                .when(!pane.stack.is_empty(), |preview| {
                    preview.child(
                        div()
                            .mt_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(colors.text_muted)
                            .truncate()
                            .child(format!("+{} stacked", pane.stack.len())),
                    )
                })
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
        read_only_field(format!("pane-template-axis-{path:?}"), axis.label(), colors)
            .w_full()
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
            actions.push(
                SettingsButton::new(
                    format!("pane-template-remove-{path:?}"),
                    "Remove split",
                    SettingsControl::RemovePaneTemplateNode(path),
                )
                .destructive()
                .render(editor, colors, handle),
            );
        }
    }

    v_flex()
        .mt_3()
        .child(section_heading(
            "Selected split",
            Some(format!("{pane_count} panes").into()),
            colors,
        ))
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

/// What every row builder for a selected template node needs: the form being
/// edited, the node's address inside it, and the theme and handle its widgets
/// are built from.
///
/// The six travel together through each section of the node's detail form, and
/// none of them changes between sections, so they travel as one `Copy` bundle
/// rather than as six parameters repeated per builder.
#[derive(Clone, Copy)]
struct PaneNodeContext<'a> {
    editor: &'a SettingsEditor,
    template_index: usize,
    path: PaneTemplateNodePath,
    editable: bool,
    colors: &'a ThemeColors,
    handle: &'a WeakEntity<Zetta>,
    /// The slider the overlay's opacity is set with.
    opacity_slider: &'a dyn Fn(f32, OpacityTarget) -> AnyElement,
}

impl PaneNodeContext<'_> {
    /// Every text field in a node's form addresses that same node, so its
    /// input differs only by which field it edits.
    fn node_text_input(&self, field: PaneTemplateNodeField) -> SettingsInput {
        SettingsInput::PaneTemplate(PaneTemplateTextField::Node(
            self.template_index,
            self.path,
            field,
        ))
    }

    /// [`Self::node_text_input`] as the control the keyboard focuses.
    fn node_input(&self, field: PaneTemplateNodeField) -> SettingsControl {
        SettingsControl::Input(self.node_text_input(field))
    }

    /// The field for one of this node's values, whose element id and control
    /// identity both follow from the field — read-only while the template is a
    /// built-in one.
    fn node_field(&self, id: String, value: TextField, field: PaneTemplateNodeField) -> AnyElement {
        editable_field(
            id,
            &value,
            self.node_text_input(field),
            self.editable,
            self.editor,
            self.colors,
            self.handle,
        )
    }
}

/// The detail form for a selected leaf, section by section.
fn render_pane_details(pane: &PaneTemplatePaneForm, ctx: PaneNodeContext<'_>) -> AnyElement {
    let PaneNodeContext {
        editor,
        template_index,
        path,
        editable,
        colors,
        handle,
        ..
    } = ctx;
    let mut rows = Vec::new();
    push_node_tree_rows(&mut rows, pane, ctx);
    push_node_source_rows(&mut rows, pane, ctx);
    push_node_command_rows(&mut rows, pane, ctx);
    push_node_environment_rows(&mut rows, pane, ctx);
    push_node_overlay_rows(&mut rows, pane, ctx);
    rows.extend(render_stack_commands(
        editor,
        pane,
        template_index,
        path,
        editable,
        colors,
        handle,
    ));
    v_flex().children(rows).into_any_element()
}

/// The buttons that reshape the tree around this node, and its label.
fn push_node_tree_rows(
    rows: &mut Vec<AnyElement>,
    pane: &PaneTemplatePaneForm,
    ctx: PaneNodeContext<'_>,
) {
    let PaneNodeContext {
        editor,
        path,
        editable,
        colors,
        handle,
        ..
    } = ctx;
    let label = ctx.node_field(
        format!("pane-template-label-{path:?}"),
        pane.label.clone(),
        PaneTemplateNodeField::Label,
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
        tree_actions.push(
            SettingsButton::new(
                format!("pane-template-remove-leaf-{path:?}"),
                "Remove pane",
                SettingsControl::RemovePaneTemplateNode(path),
            )
            .destructive()
            .enabled(editable)
            .render(editor, colors, handle),
        );
    }
    rows.push(
        h_flex()
            .gap_2()
            .flex_wrap()
            .children(tree_actions)
            .into_any_element(),
    );
    rows.push(described_control_row(
        editor,
        "Label",
        Some("Lowercase kebab-case; leave empty for no label"),
        &[ctx.node_input(PaneTemplateNodeField::Label)],
        label,
        colors,
    ));
}

/// What the pane runs, and the themes that override the profile's.
fn push_node_source_rows(
    rows: &mut Vec<AnyElement>,
    pane: &PaneTemplatePaneForm,
    ctx: PaneNodeContext<'_>,
) {
    let PaneNodeContext {
        editor,
        path,
        editable,
        colors,
        handle,
        ..
    } = ctx;
    let source_label = match &pane.source {
        PaneTemplateSourceForm::Inherit => crate::settings_ui::INHERIT_LABEL.to_owned(),
        PaneTemplateSourceForm::Profile(profile) => profile.clone(),
        PaneTemplateSourceForm::Command(_) => "Direct command".to_owned(),
    };
    rows.push(control_row(
        editor,
        "Profile or command",
        &[SettingsControl::Dropdown(
            SettingsDropdown::PaneTemplateSource(path),
        )],
        editable_dropdown(
            format!("pane-template-source-{path:?}"),
            source_label,
            SettingsDropdown::PaneTemplateSource(path),
            editable,
            editor,
            colors,
            handle,
        ),
        colors,
    ));
    let theme_label = pane
        .theme
        .clone()
        .unwrap_or_else(|| crate::settings_ui::INHERIT_LABEL.to_owned());
    rows.push(control_row(
        editor,
        "Light theme override",
        &[SettingsControl::Dropdown(
            SettingsDropdown::PaneTemplateTheme(path),
        )],
        editable_dropdown(
            format!("pane-template-theme-{path:?}"),
            theme_label,
            SettingsDropdown::PaneTemplateTheme(path),
            editable,
            editor,
            colors,
            handle,
        ),
        colors,
    ));
    let dark_theme_label = pane
        .dark_theme
        .clone()
        .unwrap_or_else(|| crate::settings_ui::INHERIT_LABEL.to_owned());
    rows.push(control_row(
        editor,
        "Dark theme override",
        &[SettingsControl::Dropdown(
            SettingsDropdown::PaneTemplateDarkTheme(path),
        )],
        editable_dropdown(
            format!("pane-template-dark-theme-{path:?}"),
            dark_theme_label,
            SettingsDropdown::PaneTemplateDarkTheme(path),
            editable,
            editor,
            colors,
            handle,
        ),
        colors,
    ));
}

/// The program and arguments a `Command` source runs. Nothing is emitted for a
/// pane that inherits or names a profile.
fn push_node_command_rows(
    rows: &mut Vec<AnyElement>,
    pane: &PaneTemplatePaneForm,
    ctx: PaneNodeContext<'_>,
) {
    let PaneNodeContext {
        editor,
        path,
        editable,
        colors,
        handle,
        ..
    } = ctx;
    if let PaneTemplateSourceForm::Command(command) = &pane.source {
        let program = ctx.node_field(
            format!("pane-template-command-program-{path:?}"),
            command.program.clone(),
            PaneTemplateNodeField::CommandProgram,
        );
        rows.push(control_row(
            editor,
            "Command program",
            &[ctx.node_input(PaneTemplateNodeField::CommandProgram)],
            program,
            colors,
        ));
        for (argument, value) in command.args.iter().enumerate() {
            let argument_input = ctx.node_field(
                format!("pane-template-command-arg-{path:?}-{argument}"),
                value.clone(),
                PaneTemplateNodeField::CommandArgument(argument),
            );
            let remove = settings_remove_button(
                editor,
                format!("pane-template-command-arg-remove-{path:?}-{argument}"),
                SettingsControl::RemovePaneTemplateArgument(path, argument),
                "argument",
                editable,
                colors,
                handle,
            );
            rows.push(control_row(
                editor,
                format!("Argument {}", argument + 1),
                &[
                    ctx.node_input(PaneTemplateNodeField::CommandArgument(argument)),
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
        rows.push(add_row(
            action_button(
                editor,
                format!("pane-template-command-arg-add-{path:?}"),
                "Add argument".to_owned(),
                SettingsControl::AddPaneTemplateArgument(path),
                editable,
                colors,
                handle,
            ),
            editor,
            &[SettingsControl::AddPaneTemplateArgument(path)],
        ));
    }
}

/// The pane's own environment overrides, as an ordered list of name/value pairs.
fn push_node_environment_rows(
    rows: &mut Vec<AnyElement>,
    pane: &PaneTemplatePaneForm,
    ctx: PaneNodeContext<'_>,
) {
    let PaneNodeContext {
        editor,
        path,
        editable,
        colors,
        handle,
        ..
    } = ctx;
    rows.push(section_heading(
        "Pane environment",
        Some("Variables for this pane only; they override the template's".into()),
        colors,
    ));
    for (environment, entry) in pane.environment.iter().enumerate() {
        rows.extend(environment_pair_rows(
            EnvironmentPair {
                label: format!("Variable {}", environment + 1),
                id: format!("pane-template-env-{path:?}-{environment}"),
                name: &entry.name,
                name_input: ctx
                    .node_text_input(PaneTemplateNodeField::EnvironmentName(environment)),
                value: &entry.value,
                value_input: ctx
                    .node_text_input(PaneTemplateNodeField::EnvironmentValue(environment)),
                remove: SettingsControl::RemovePaneTemplateEnvironment(path, environment),
                editable,
            },
            editor,
            colors,
            handle,
        ));
    }
    rows.push(add_row(
        action_button(
            editor,
            format!("pane-template-env-add-{path:?}"),
            "Add environment variable".to_owned(),
            SettingsControl::AddPaneTemplateEnvironment(path),
            editable,
            colors,
            handle,
        ),
        editor,
        &[SettingsControl::AddPaneTemplateEnvironment(path)],
    ));
}

/// The pane's overlay: the button that adds or removes one, and its text, size,
/// opacity and colour while it has one.
fn push_node_overlay_rows(
    rows: &mut Vec<AnyElement>,
    pane: &PaneTemplatePaneForm,
    ctx: PaneNodeContext<'_>,
) {
    let PaneNodeContext {
        editor,
        path,
        editable,
        colors,
        handle,
        ..
    } = ctx;
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
            .child(div().text_xs().text_color(colors.text).child("Overlay"))
            .child(overlay_toggle)
            .into_any_element(),
    );
    if let Some(overlay) = &pane.overlay {
        rows.push(control_row(
            editor,
            "Overlay text",
            &[ctx.node_input(PaneTemplateNodeField::OverlayText)],
            ctx.node_field(
                format!("pane-template-overlay-text-{path:?}"),
                overlay.text.clone(),
                PaneTemplateNodeField::OverlayText,
            ),
            colors,
        ));
        let size_label = overlay
            .size
            .map_or_else(|| "Default".to_owned(), |size| size.label().to_owned());
        rows.push(control_row(
            editor,
            "Overlay size",
            &[SettingsControl::Dropdown(
                SettingsDropdown::PaneTemplateOverlaySize(path),
            )],
            editable_dropdown(
                format!("pane-template-overlay-size-{path:?}"),
                size_label,
                SettingsDropdown::PaneTemplateOverlaySize(path),
                editable,
                editor,
                colors,
                handle,
            ),
            colors,
        ));
        let target = OpacityTarget::PaneTemplateOverlay(path);
        let opacity = crate::settings_ui::pane_templates::overlay_opacity(editor, path)
            .unwrap_or(crate::pane::DEFAULT_OVERLAY_OPACITY);
        rows.push(control_row(
            editor,
            "Overlay opacity",
            &[SettingsControl::Opacity(target)],
            if editable {
                (ctx.opacity_slider)(opacity, target)
            } else {
                read_only_field(
                    format!("pane-template-overlay-opacity-{path:?}"),
                    format!("{}%", (opacity * 100.).round() as u32),
                    colors,
                )
                .w_full()
                .into_any_element()
            },
            colors,
        ));
        rows.push(described_control_row(
            editor,
            "Overlay color",
            Some("A color name, or a hex value such as #ff8800"),
            &[ctx.node_input(PaneTemplateNodeField::OverlayColor)],
            ctx.node_field(
                format!("pane-template-overlay-color-{path:?}"),
                overlay.color.clone(),
                PaneTemplateNodeField::OverlayColor,
            ),
            colors,
        ));
    }
}

/// The leaf's stacked commands. Each one becomes a stacked entry sharing the
/// pane's region with its interactive shell, so they are edited as an ordered
/// list of `{program, args}` commands like the leaf's own command.
///
/// The row order here is the page's tab order and has to match
/// `add_stack_controls`.
fn render_stack_commands(
    editor: &SettingsEditor,
    pane: &PaneTemplatePaneForm,
    template_index: usize,
    path: PaneTemplateNodePath,
    editable: bool,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> Vec<AnyElement> {
    let node_control = |field| {
        SettingsControl::Input(SettingsInput::PaneTemplate(PaneTemplateTextField::Node(
            template_index,
            path,
            field,
        )))
    };
    let node_input = |field| {
        SettingsInput::PaneTemplate(PaneTemplateTextField::Node(template_index, path, field))
    };
    let mut rows = vec![section_heading(
        "Stacked commands",
        Some("Commands that run beside this pane's shell, sharing its space".into()),
        colors,
    )];
    for (entry, command) in pane.stack.iter().enumerate() {
        let program = editable_field(
            format!("pane-template-stack-program-{path:?}-{entry}"),
            &command.program,
            node_input(PaneTemplateNodeField::StackProgram(entry)),
            editable,
            editor,
            colors,
            handle,
        );
        let remove = settings_remove_button(
            editor,
            format!("pane-template-stack-remove-{path:?}-{entry}"),
            SettingsControl::RemovePaneTemplateStackEntry(path, entry),
            "stacked command",
            editable,
            colors,
            handle,
        );
        rows.push(control_row(
            editor,
            format!("Stacked command {} · program", entry + 1),
            &[
                node_control(PaneTemplateNodeField::StackProgram(entry)),
                SettingsControl::RemovePaneTemplateStackEntry(path, entry),
            ],
            h_flex()
                .gap_1()
                .child(program)
                .child(remove)
                .into_any_element(),
            colors,
        ));
        for (argument, value) in command.args.iter().enumerate() {
            let argument_input = editable_field(
                format!("pane-template-stack-arg-{path:?}-{entry}-{argument}"),
                value,
                node_input(PaneTemplateNodeField::StackArgument(entry, argument)),
                editable,
                editor,
                colors,
                handle,
            );
            let remove = settings_remove_button(
                editor,
                format!("pane-template-stack-arg-remove-{path:?}-{entry}-{argument}"),
                SettingsControl::RemovePaneTemplateStackArgument(path, entry, argument),
                "argument",
                editable,
                colors,
                handle,
            );
            rows.push(control_row(
                editor,
                format!("Stacked command {} · argument {}", entry + 1, argument + 1),
                &[
                    node_control(PaneTemplateNodeField::StackArgument(entry, argument)),
                    SettingsControl::RemovePaneTemplateStackArgument(path, entry, argument),
                ],
                h_flex()
                    .gap_1()
                    .child(argument_input)
                    .child(remove)
                    .into_any_element(),
                colors,
            ));
        }
        rows.push(add_row(
            action_button(
                editor,
                format!("pane-template-stack-arg-add-{path:?}-{entry}"),
                "Add argument".to_owned(),
                SettingsControl::AddPaneTemplateStackArgument(path, entry),
                editable,
                colors,
                handle,
            ),
            editor,
            &[SettingsControl::AddPaneTemplateStackArgument(path, entry)],
        ));
    }
    rows.push(add_row(
        action_button(
            editor,
            format!("pane-template-stack-add-{path:?}"),
            "Add stacked command".to_owned(),
            SettingsControl::AddPaneTemplateStackEntry(path),
            editable,
            colors,
            handle,
        ),
        editor,
        &[SettingsControl::AddPaneTemplateStackEntry(path)],
    ));
    rows
}

fn render_global_environment(
    editor: &SettingsEditor,
    template: &PaneTemplateForm,
    template_index: usize,
    editable: bool,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> AnyElement {
    let mut rows = vec![section_heading(
        "Environment for all panes",
        Some("Applied to every pane; a pane's own values override matching names".into()),
        colors,
    )];
    for (environment, entry) in template.environment.iter().enumerate() {
        rows.extend(environment_pair_rows(
            EnvironmentPair {
                label: format!("Variable {}", environment + 1),
                id: format!("pane-template-global-env-{environment}"),
                name: &entry.name,
                name_input: SettingsInput::PaneTemplate(
                    PaneTemplateTextField::GlobalEnvironmentName(template_index, environment),
                ),
                value: &entry.value,
                value_input: SettingsInput::PaneTemplate(
                    PaneTemplateTextField::GlobalEnvironmentValue(template_index, environment),
                ),
                remove: SettingsControl::RemovePaneTemplateGlobalEnvironment(environment),
                editable,
            },
            editor,
            colors,
            handle,
        ));
    }
    rows.push(add_row(
        action_button(
            editor,
            "pane-template-global-env-add".to_owned(),
            "Add environment variable".to_owned(),
            SettingsControl::AddPaneTemplateGlobalEnvironment,
            editable,
            colors,
            handle,
        ),
        editor,
        &[SettingsControl::AddPaneTemplateGlobalEnvironment],
    ));
    v_flex().children(rows).into_any_element()
}

pub(crate) fn render_pane_templates_page(
    editor: &SettingsEditor,
    colors: &ThemeColors,
    widgets: &super::pages::PageWidgets<'_>,
    handle: &WeakEntity<Zetta>,
) -> AnyElement {
    // Whichever form the editor is pointed at: the user configuration on the
    // Templates page, or the open project's overlay in the Projects builder.
    let pane_templates = templates(editor);
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
    let mut list = pane_template_list_rows(editor, colors, handle, inherited_label);
    list.push(
        h_flex()
            .gap_2()
            .child(action_button(
                editor,
                "pane-template-new".to_owned(),
                "Add template".to_owned(),
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
    let details = pane_template_details(
        editor,
        colors,
        handle,
        TemplateDetails {
            selected,
            editable,
            inherited_label,
            widgets,
        },
    );

    h_flex()
        .w_full()
        .items_start()
        .gap_4()
        .child(
            // A column: `gap_2` did nothing while this was a block box, so the
            // rows touched.
            v_flex()
                .w(px(210.))
                .flex_none()
                .gap_2()
                .child(section_heading("Templates", None, colors))
                .children(list),
        )
        .child(div().min_w_0().flex_1().child(details))
        .into_any_element()
}

/// One row per template, each naming whether it is a built-in preset or
/// inherited from the layer below.
fn pane_template_list_rows(
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
    inherited_label: &'static str,
) -> Vec<AnyElement> {
    let pane_templates = templates(editor);
    let selected_index = pane_templates.selected_template;
    let mut list: Vec<AnyElement> = Vec::new();
    for (index, template) in pane_templates.templates.iter().enumerate() {
        let selected_row = index == selected_index;
        let focused_row =
            editor.focused_control == Some(SettingsControl::SelectPaneTemplate(index));
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
                .rounded(RADIUS_CONTROL)
                .cursor_pointer()
                // Focus is the ring and selection the fill, so the keyboard
                // can be followed across the selected row.
                .border_1()
                .border_color(if focused_row {
                    colors.border_focused
                } else if selected_row {
                    colors.border_selected
                } else {
                    colors.border_variant
                })
                .when(selected_row, |row| row.bg(colors.element_selected))
                .when(!selected_row, |row| {
                    row.hover(|style| style.bg(colors.element_hover))
                })
                .on_click(activate_on_click(handle, control.clone()))
                .child(div().text_sm().child(label))
                .child(
                    div()
                        .text_xs()
                        .text_color(colors.text_muted)
                        .child(format!("{} panes", template.node.pane_count())),
                )
                .into_any_element(),
        );
    }
    list
}

/// The selected template's name, its layout preview, and the details of
/// whichever node is selected inside it.
/// What the details column shows: the selected template, whether it can be
/// edited, what its read-only layer is called, and the colour its validation
/// error is shown in.
struct TemplateDetails<'a> {
    selected: Option<&'a PaneTemplateForm>,
    editable: bool,
    inherited_label: &'static str,
    widgets: &'a super::pages::PageWidgets<'a>,
}

fn pane_template_details(
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
    details: TemplateDetails<'_>,
) -> AnyElement {
    let TemplateDetails {
        selected,
        editable,
        inherited_label,
        widgets,
    } = details;
    let pane_templates = templates(editor);
    let selected_index = pane_templates.selected_template;
    if let Some(template) = selected {
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
            read_only_field("pane-template-name", template.name.text.clone(), colors)
                .w_full()
                .into_any_element()
        };
        let delete = SettingsButton::new(
            "pane-template-delete".to_owned(),
            if template.inherited() {
                "Reset override"
            } else {
                "Remove template"
            },
            SettingsControl::DeletePaneTemplate,
        )
        .destructive()
        .enabled(editable)
        .render(editor, colors, handle);
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
        content.push(section_heading(
            "Layout preview",
            Some("Select a pane to edit it; select it again to edit the split around it".into()),
            colors,
        ));
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
                .rounded(RADIUS_CONTROL)
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
                        .child(section_heading("Selected pane", None, colors))
                        .child(render_pane_details(
                            pane,
                            PaneNodeContext {
                                editor,
                                template_index: selected_index,
                                path,
                                editable,
                                colors,
                                handle,
                                opacity_slider: widgets.opacity_slider,
                            },
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
                    .child(crate::ui_messages::error_message(
                        error.clone(),
                        widgets.error_color,
                    ))
                    .into_any_element(),
            );
        }
        v_flex().children(content).into_any_element()
    } else {
        empty_state("There are no pane templates yet", colors)
    }
}
