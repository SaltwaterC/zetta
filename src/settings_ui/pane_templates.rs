use super::*;

use crate::config::{PaneSplitAxis, PaneSplitOverlaySize};
use crate::settings_editor::{
    PaneTemplateCommandForm, PaneTemplateEnvironmentForm, PaneTemplateForm, PaneTemplateNodeField,
    PaneTemplateNodeForm, PaneTemplateNodePath, PaneTemplateOverlayForm, PaneTemplatePaneForm,
    PaneTemplateSourceForm, PaneTemplateTextField, rename_pane_template_bindings,
};
use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

const VALIDATION_DEBOUNCE: Duration = Duration::from_millis(75);

pub(crate) fn schedule_pane_template_validation(zetta: &mut Zetta, cx: &mut Context<Zetta>) {
    let Some(editor) = zetta.settings_editor.as_mut() else {
        return;
    };
    editor.pane_template_validation_generation =
        editor.pane_template_validation_generation.wrapping_add(1);
    let generation = editor.pane_template_validation_generation;
    editor.pane_template_validation_error = None;

    let executor = cx.background_executor().clone();
    cx.spawn(async move |this, cx| {
        executor.timer(VALIDATION_DEBOUNCE).await;
        let templates = this
            .update(cx, |this, _| {
                let editor = this.settings_editor.as_ref()?;
                (editor.pane_template_validation_generation == generation)
                    .then(|| editor.configuration.pane_templates.clone())
            })
            .ok()
            .flatten();
        let Some(templates) = templates else {
            return;
        };
        let error = executor
            .spawn(async move { templates.validate().err().map(|error| format!("{error:#}")) })
            .await;
        this.update(cx, |this, cx| {
            let Some(editor) = this.settings_editor.as_mut() else {
                return;
            };
            if editor.pane_template_validation_generation == generation {
                editor.pane_template_validation_error = error;
                cx.notify();
            }
        })
        .ok();
    })
    .detach();
    cx.notify();
}

pub(crate) fn pane_template_text_mut(
    editor: &mut SettingsEditor,
    field: PaneTemplateTextField,
) -> Option<&mut TextField> {
    match field {
        PaneTemplateTextField::Name(template) => editor
            .configuration
            .pane_templates
            .templates
            .get_mut(template)
            .filter(|template| template.editable())
            .map(|template| &mut template.name),
        PaneTemplateTextField::GlobalEnvironmentName(template, environment) => editor
            .configuration
            .pane_templates
            .templates
            .get_mut(template)
            .filter(|template| template.editable())?
            .environment
            .get_mut(environment)
            .map(|entry| &mut entry.name),
        PaneTemplateTextField::GlobalEnvironmentValue(template, environment) => editor
            .configuration
            .pane_templates
            .templates
            .get_mut(template)
            .filter(|template| template.editable())?
            .environment
            .get_mut(environment)
            .map(|entry| &mut entry.value),
        PaneTemplateTextField::Node(template, path, field) => {
            let template = editor
                .configuration
                .pane_templates
                .templates
                .get_mut(template)
                .filter(|template| template.editable())?;
            let PaneTemplateNodeForm::Pane(pane) = template.node.node_at_mut(path)? else {
                return None;
            };
            match field {
                PaneTemplateNodeField::Label => Some(&mut pane.label),
                PaneTemplateNodeField::CommandProgram => match &mut pane.source {
                    PaneTemplateSourceForm::Command(command) => Some(&mut command.program),
                    _ => None,
                },
                PaneTemplateNodeField::CommandArgument(argument) => match &mut pane.source {
                    PaneTemplateSourceForm::Command(command) => command.args.get_mut(argument),
                    _ => None,
                },
                PaneTemplateNodeField::EnvironmentName(environment) => pane
                    .environment
                    .get_mut(environment)
                    .map(|entry| &mut entry.name),
                PaneTemplateNodeField::EnvironmentValue(environment) => pane
                    .environment
                    .get_mut(environment)
                    .map(|entry| &mut entry.value),
                PaneTemplateNodeField::OverlayText => {
                    pane.overlay.as_mut().map(|overlay| &mut overlay.text)
                }
                PaneTemplateNodeField::OverlayOpacity => {
                    pane.overlay.as_mut().map(|overlay| &mut overlay.opacity)
                }
                PaneTemplateNodeField::OverlayColor => {
                    pane.overlay.as_mut().map(|overlay| &mut overlay.color)
                }
            }
        }
    }
}

fn selected_node(
    editor: &SettingsEditor,
    path: PaneTemplateNodePath,
) -> Option<&PaneTemplateNodeForm> {
    editor
        .configuration
        .pane_templates
        .selected()?
        .node
        .node_at(path)
}

fn selected_pane(
    editor: &SettingsEditor,
    path: PaneTemplateNodePath,
) -> Option<&PaneTemplatePaneForm> {
    match selected_node(editor, path)? {
        PaneTemplateNodeForm::Pane(pane) => Some(pane),
        PaneTemplateNodeForm::Split { .. } => None,
    }
}

fn selected_pane_mut(
    editor: &mut SettingsEditor,
    path: PaneTemplateNodePath,
) -> Option<&mut PaneTemplatePaneForm> {
    match editor
        .configuration
        .pane_templates
        .selected_mut()?
        .node
        .node_at_mut(path)?
    {
        PaneTemplateNodeForm::Pane(pane) => Some(pane),
        PaneTemplateNodeForm::Split { .. } => None,
    }
}

fn overlay_size_options() -> Arc<[String]> {
    [
        "Default".to_owned(),
        PaneSplitOverlaySize::Small.label().to_owned(),
        PaneSplitOverlaySize::Base.label().to_owned(),
        PaneSplitOverlaySize::Large.label().to_owned(),
        PaneSplitOverlaySize::ExtraLarge.label().to_owned(),
        PaneSplitOverlaySize::ExtraExtraLarge.label().to_owned(),
        PaneSplitOverlaySize::ExtraExtraExtraLarge
            .label()
            .to_owned(),
    ]
    .into()
}

fn overlay_size_label(size: PaneSplitOverlaySize) -> String {
    size.label().to_owned()
}

fn overlay_size_from_label(value: &str) -> Option<PaneSplitOverlaySize> {
    [
        PaneSplitOverlaySize::Small,
        PaneSplitOverlaySize::Base,
        PaneSplitOverlaySize::Large,
        PaneSplitOverlaySize::ExtraLarge,
        PaneSplitOverlaySize::ExtraExtraLarge,
        PaneSplitOverlaySize::ExtraExtraExtraLarge,
    ]
    .into_iter()
    .find(|size| size.label() == value)
}

pub(crate) fn pane_template_dropdown_options(
    editor: &SettingsEditor,
    dropdown: SettingsDropdown,
) -> (String, Arc<[String]>) {
    let path = match dropdown {
        SettingsDropdown::PaneTemplateAxis(path)
        | SettingsDropdown::PaneTemplateSource(path)
        | SettingsDropdown::PaneTemplateTheme(path)
        | SettingsDropdown::PaneTemplateOverlaySize(path) => path,
        _ => return (String::new(), Arc::from([])),
    };
    match dropdown {
        SettingsDropdown::PaneTemplateAxis(_) => {
            let selected = match selected_node(editor, path) {
                Some(PaneTemplateNodeForm::Split { axis, .. }) => axis.label().to_owned(),
                _ => String::new(),
            };
            (
                selected,
                Arc::from(["Vertical".to_owned(), "Horizontal".to_owned()]),
            )
        }
        SettingsDropdown::PaneTemplateSource(_) => {
            let selected = match selected_pane(editor, path).map(|pane| &pane.source) {
                Some(PaneTemplateSourceForm::Inherit) | None => "Inherited".to_owned(),
                Some(PaneTemplateSourceForm::Profile(profile)) => profile.clone(),
                Some(PaneTemplateSourceForm::Command(_)) => "Direct command".to_owned(),
            };
            let mut options = vec!["Inherited".to_owned()];
            options.extend(editor.profile_names.iter().cloned());
            options.push("Direct command".to_owned());
            options.sort_by_key(|option| option.to_ascii_lowercase());
            options.dedup();
            (selected, options.into())
        }
        SettingsDropdown::PaneTemplateTheme(_) => {
            let selected = selected_pane(editor, path)
                .and_then(|pane| pane.theme.clone())
                .unwrap_or_else(|| "Use profile/application theme".to_owned());
            let options = std::iter::once("Use profile/application theme".to_owned())
                .chain(editor.themes.iter().cloned())
                .collect::<Vec<_>>();
            (selected, options.into())
        }
        SettingsDropdown::PaneTemplateOverlaySize(_) => {
            let selected = selected_pane(editor, path)
                .and_then(|pane| pane.overlay.as_ref())
                .and_then(|overlay| overlay.size)
                .map(overlay_size_label)
                .unwrap_or_else(|| "Default".to_owned());
            (selected, overlay_size_options())
        }
        _ => unreachable!(),
    }
}

pub(crate) fn set_pane_template_dropdown(
    editor: &mut SettingsEditor,
    dropdown: SettingsDropdown,
    value: &str,
) -> bool {
    let path = match dropdown {
        SettingsDropdown::PaneTemplateAxis(path)
        | SettingsDropdown::PaneTemplateSource(path)
        | SettingsDropdown::PaneTemplateTheme(path)
        | SettingsDropdown::PaneTemplateOverlaySize(path) => path,
        _ => return false,
    };
    if !editor.configuration.pane_templates.selected_is_editable() {
        return false;
    }
    let changed = match dropdown {
        SettingsDropdown::PaneTemplateAxis(_) => editor
            .configuration
            .pane_templates
            .set_selected_axis(if value == "Horizontal" {
                PaneSplitAxis::Horizontal
            } else {
                PaneSplitAxis::Vertical
            })
            .is_ok(),
        SettingsDropdown::PaneTemplateSource(_) => {
            let Some(pane) = selected_pane_mut(editor, path) else {
                return false;
            };
            pane.source = if value == "Inherited" {
                PaneTemplateSourceForm::Inherit
            } else if value == "Direct command" {
                PaneTemplateSourceForm::Command(PaneTemplateCommandForm {
                    program: TextField::default(),
                    args: Vec::new(),
                })
            } else {
                PaneTemplateSourceForm::Profile(value.to_owned())
            };
            true
        }
        SettingsDropdown::PaneTemplateTheme(_) => {
            let Some(pane) = selected_pane_mut(editor, path) else {
                return false;
            };
            pane.theme = (value != "Use profile/application theme").then(|| value.to_owned());
            true
        }
        SettingsDropdown::PaneTemplateOverlaySize(_) => {
            let Some(pane) = selected_pane_mut(editor, path) else {
                return false;
            };
            let Some(overlay) = pane.overlay.as_mut() else {
                return false;
            };
            overlay.size = (value != "Default")
                .then(|| overlay_size_from_label(value))
                .flatten();
            true
        }
        _ => unreachable!(),
    };
    if changed {
        editor.configuration_dirty = true;
        editor.message = None;
        invalidate_controls_cache(editor);
    }
    changed
}

fn template_is_referenced(editor: &SettingsEditor, name: &str) -> bool {
    editor.keymap.sections.iter().any(|section| {
        section.bindings.iter().any(|binding| {
            binding.action.as_array().is_some_and(|action| {
                action.first().and_then(Value::as_str)
                    == Some(ApplyPaneSplitTemplate::name_for_type())
                    && action
                        .get(1)
                        .and_then(Value::as_object)
                        .and_then(|arguments| arguments.get("name"))
                        .and_then(Value::as_str)
                        == Some(name)
            })
        })
    })
}

fn template_has_pending_reference(editor: &SettingsEditor, template: &PaneTemplateForm) -> bool {
    if template.built_in {
        template.name.text != template.original_name
            && template_is_referenced(editor, &template.name.text)
    } else {
        template_is_referenced(editor, &template.name.text)
            || (template.name.text != template.original_name
                && template_is_referenced(editor, &template.original_name))
    }
}

pub(crate) fn refresh_template_names(editor: &mut SettingsEditor) {
    let mut names = editor.configuration.pane_templates.names();
    names.sort();
    names.dedup();
    editor.pane_template_names = names.into();
    invalidate_controls_cache(editor);
}

pub(crate) fn synchronize_pane_template_keybindings(editor: &mut SettingsEditor) {
    let mut renamed = Vec::new();
    for template in &editor.configuration.pane_templates.templates {
        if template.editable()
            && template.original_name != template.name.text
            && !template.name.text.trim().is_empty()
        {
            renamed.push((template.original_name.clone(), template.name.text.clone()));
        }
    }
    if renamed.is_empty() {
        return;
    }
    let changed = rename_pane_template_bindings(&mut editor.keymap, &renamed);
    for (old_name, new_name) in renamed {
        if let Some(template) = editor
            .configuration
            .pane_templates
            .templates
            .iter_mut()
            .find(|template| template.original_name == old_name)
        {
            template.original_name = new_name;
        }
    }
    if changed {
        editor.keymap_dirty = true;
        invalidate_keymap_cache(editor);
    }
    refresh_template_names(editor);
}

fn add_pane_template_environment(editor: &mut SettingsEditor, path: PaneTemplateNodePath) -> bool {
    if !editor.configuration.pane_templates.selected_is_editable() {
        return false;
    }
    let Some(pane) = selected_pane_mut(editor, path) else {
        return false;
    };
    pane.environment.push(PaneTemplateEnvironmentForm {
        name: TextField::default(),
        value: TextField::default(),
    });
    true
}

pub(crate) fn pane_template_controls(editor: &SettingsEditor) -> Vec<SettingsControl> {
    let mut controls = Vec::new();
    let pane_templates = &editor.configuration.pane_templates;
    controls.extend(
        pane_templates
            .templates
            .iter()
            .enumerate()
            .map(|(index, _)| SettingsControl::SelectPaneTemplate(index)),
    );
    controls.extend([
        SettingsControl::NewPaneTemplate,
        SettingsControl::DuplicatePaneTemplate,
    ]);
    let Some(template) = pane_templates.selected() else {
        return controls;
    };
    let editable = template.editable();
    if editable {
        controls.extend([
            SettingsControl::Input(SettingsInput::PaneTemplate(PaneTemplateTextField::Name(
                pane_templates.selected_template,
            ))),
            SettingsControl::DeletePaneTemplate,
        ]);
        add_global_environment_controls(
            &mut controls,
            pane_templates.selected_template,
            template.environment.len(),
        );
    }
    // The selected template index is captured once so each recursive control has
    // a stable identity even while the tree is being edited.
    let template_index = pane_templates.selected_template;
    let selected_path = pane_templates.selected_node;
    let mut tree_controls = Vec::new();
    add_node_controls_with_template(
        &mut tree_controls,
        &template.node,
        PaneTemplateNodePath::ROOT,
        editable,
        template_index,
        selected_path,
    );
    controls.extend(tree_controls);
    controls
}

fn add_global_environment_controls(
    controls: &mut Vec<SettingsControl>,
    template_index: usize,
    environment_count: usize,
) {
    for environment in 0..environment_count {
        controls.extend([
            SettingsControl::Input(SettingsInput::PaneTemplate(
                PaneTemplateTextField::GlobalEnvironmentName(template_index, environment),
            )),
            SettingsControl::Input(SettingsInput::PaneTemplate(
                PaneTemplateTextField::GlobalEnvironmentValue(template_index, environment),
            )),
            SettingsControl::RemovePaneTemplateGlobalEnvironment(environment),
        ]);
    }
    controls.push(SettingsControl::AddPaneTemplateGlobalEnvironment);
}

fn add_node_controls_with_template(
    controls: &mut Vec<SettingsControl>,
    node: &PaneTemplateNodeForm,
    path: PaneTemplateNodePath,
    editable: bool,
    template_index: usize,
    selected_path: Option<PaneTemplateNodePath>,
) {
    controls.push(SettingsControl::SelectPaneTemplateNode(path));
    match node {
        PaneTemplateNodeForm::Split { first, second, .. } => {
            if editable && selected_path == Some(path) {
                controls.extend([
                    SettingsControl::Dropdown(SettingsDropdown::PaneTemplateAxis(path)),
                    SettingsControl::SwapPaneTemplateChildren(path),
                ]);
                if !path.is_root() {
                    controls.push(SettingsControl::RemovePaneTemplateNode(path));
                }
            }
            add_node_controls_with_template(
                controls,
                first,
                path.child(false).unwrap_or(path),
                editable,
                template_index,
                selected_path,
            );
            add_node_controls_with_template(
                controls,
                second,
                path.child(true).unwrap_or(path),
                editable,
                template_index,
                selected_path,
            );
        }
        PaneTemplateNodeForm::Pane(pane) => {
            if !editable || selected_path != Some(path) {
                return;
            }
            controls.extend([
                SettingsControl::SplitPaneTemplate(path, PaneSplitAxis::Horizontal),
                SettingsControl::SplitPaneTemplate(path, PaneSplitAxis::Vertical),
            ]);
            if !path.is_root() {
                controls.push(SettingsControl::RemovePaneTemplateNode(path));
            }
            controls.extend([
                SettingsControl::Input(SettingsInput::PaneTemplate(PaneTemplateTextField::Node(
                    template_index,
                    path,
                    PaneTemplateNodeField::Label,
                ))),
                SettingsControl::Dropdown(SettingsDropdown::PaneTemplateSource(path)),
                SettingsControl::Dropdown(SettingsDropdown::PaneTemplateTheme(path)),
            ]);
            if let PaneTemplateSourceForm::Command(command) = &pane.source {
                controls.push(SettingsControl::Input(SettingsInput::PaneTemplate(
                    PaneTemplateTextField::Node(
                        template_index,
                        path,
                        PaneTemplateNodeField::CommandProgram,
                    ),
                )));
                for argument in 0..command.args.len() {
                    controls.extend([
                        SettingsControl::Input(SettingsInput::PaneTemplate(
                            PaneTemplateTextField::Node(
                                template_index,
                                path,
                                PaneTemplateNodeField::CommandArgument(argument),
                            ),
                        )),
                        SettingsControl::RemovePaneTemplateArgument(path, argument),
                    ]);
                }
                controls.push(SettingsControl::AddPaneTemplateArgument(path));
            }
            for environment in 0..pane.environment.len() {
                controls.extend([
                    SettingsControl::Input(SettingsInput::PaneTemplate(
                        PaneTemplateTextField::Node(
                            template_index,
                            path,
                            PaneTemplateNodeField::EnvironmentName(environment),
                        ),
                    )),
                    SettingsControl::Input(SettingsInput::PaneTemplate(
                        PaneTemplateTextField::Node(
                            template_index,
                            path,
                            PaneTemplateNodeField::EnvironmentValue(environment),
                        ),
                    )),
                    SettingsControl::RemovePaneTemplateEnvironment(path, environment),
                ]);
            }
            controls.push(SettingsControl::AddPaneTemplateEnvironment(path));
            controls.push(SettingsControl::TogglePaneTemplateOverlay(path));
            if pane.overlay.is_some() {
                controls.extend([
                    SettingsControl::Input(SettingsInput::PaneTemplate(
                        PaneTemplateTextField::Node(
                            template_index,
                            path,
                            PaneTemplateNodeField::OverlayText,
                        ),
                    )),
                    SettingsControl::Dropdown(SettingsDropdown::PaneTemplateOverlaySize(path)),
                    SettingsControl::Input(SettingsInput::PaneTemplate(
                        PaneTemplateTextField::Node(
                            template_index,
                            path,
                            PaneTemplateNodeField::OverlayOpacity,
                        ),
                    )),
                    SettingsControl::Input(SettingsInput::PaneTemplate(
                        PaneTemplateTextField::Node(
                            template_index,
                            path,
                            PaneTemplateNodeField::OverlayColor,
                        ),
                    )),
                ]);
            }
        }
    }
}

pub(crate) fn activate_pane_template_control(
    zetta: &mut Zetta,
    control: SettingsControl,
    _window: &mut Window,
    cx: &mut Context<Zetta>,
) -> Result<()> {
    let Some(editor) = zetta.settings_editor.as_mut() else {
        return Ok(());
    };
    let result = match control {
        SettingsControl::SelectPaneTemplate(index) => {
            editor.configuration.pane_templates.select_template(index);
            editor.focused_control = Some(SettingsControl::SelectPaneTemplate(index));
            Ok(())
        }
        SettingsControl::SelectPaneTemplateNode(path) => {
            if editor
                .configuration
                .pane_templates
                .toggle_node_selection(path)
            {
                editor.focused_control = Some(SettingsControl::SelectPaneTemplateNode(path));
                Ok(())
            } else {
                Err(anyhow::anyhow!("pane-template node no longer exists"))
            }
        }
        SettingsControl::NewPaneTemplate => {
            editor.configuration.pane_templates.create_empty();
            Ok(())
        }
        SettingsControl::DuplicatePaneTemplate => editor
            .configuration
            .pane_templates
            .duplicate_selected()
            .map(|_| ()),
        SettingsControl::DeletePaneTemplate => {
            let referenced = editor
                .configuration
                .pane_templates
                .selected()
                .is_some_and(|template| template_has_pending_reference(editor, template));
            editor
                .configuration
                .pane_templates
                .delete_selected(referenced)
        }
        SettingsControl::SplitPaneTemplate(path, axis) => {
            editor.configuration.pane_templates.select_node(path);
            editor
                .configuration
                .pane_templates
                .split_selected_leaf(axis)
        }
        SettingsControl::RemovePaneTemplateNode(path) => {
            editor.configuration.pane_templates.select_node(path);
            editor.configuration.pane_templates.remove_selected_node()
        }
        SettingsControl::SwapPaneTemplateChildren(path) => {
            editor.configuration.pane_templates.select_node(path);
            editor.configuration.pane_templates.swap_selected_children()
        }
        SettingsControl::AddPaneTemplateArgument(path) => {
            let Some(pane) = selected_pane_mut(editor, path) else {
                return Ok(());
            };
            let PaneTemplateSourceForm::Command(command) = &mut pane.source else {
                return Ok(());
            };
            command.args.push(TextField::default());
            Ok(())
        }
        SettingsControl::RemovePaneTemplateArgument(path, argument) => {
            let Some(pane) = selected_pane_mut(editor, path) else {
                return Ok(());
            };
            let PaneTemplateSourceForm::Command(command) = &mut pane.source else {
                return Ok(());
            };
            anyhow::ensure!(
                argument < command.args.len(),
                "command argument no longer exists"
            );
            command.args.remove(argument);
            Ok(())
        }
        SettingsControl::AddPaneTemplateGlobalEnvironment => {
            let Some(template) = editor
                .configuration
                .pane_templates
                .selected_mut()
                .filter(|template| template.editable())
            else {
                return Ok(());
            };
            template.environment.push(PaneTemplateEnvironmentForm {
                name: TextField::default(),
                value: TextField::default(),
            });
            Ok(())
        }
        SettingsControl::RemovePaneTemplateGlobalEnvironment(environment) => {
            let Some(template) = editor
                .configuration
                .pane_templates
                .selected_mut()
                .filter(|template| template.editable())
            else {
                return Ok(());
            };
            anyhow::ensure!(
                environment < template.environment.len(),
                "template environment row no longer exists"
            );
            template.environment.remove(environment);
            Ok(())
        }
        SettingsControl::AddPaneTemplateEnvironment(path) => {
            anyhow::ensure!(
                add_pane_template_environment(editor, path),
                "pane is not editable"
            );
            Ok(())
        }
        SettingsControl::RemovePaneTemplateEnvironment(path, environment) => {
            let Some(pane) = selected_pane_mut(editor, path) else {
                return Ok(());
            };
            anyhow::ensure!(
                environment < pane.environment.len(),
                "environment row no longer exists"
            );
            pane.environment.remove(environment);
            Ok(())
        }
        SettingsControl::TogglePaneTemplateOverlay(path) => {
            let Some(pane) = selected_pane_mut(editor, path) else {
                return Ok(());
            };
            pane.overlay = if pane.overlay.is_some() {
                None
            } else {
                Some(PaneTemplateOverlayForm {
                    text: TextField::default(),
                    size: None,
                    opacity: TextField::default(),
                    color: TextField::default(),
                })
            };
            Ok(())
        }
        _ => return Ok(()),
    };
    let mut schedule_validation = false;
    match result {
        Ok(()) => {
            if !matches!(
                control,
                SettingsControl::SelectPaneTemplate(_) | SettingsControl::SelectPaneTemplateNode(_)
            ) {
                editor.configuration_dirty = true;
                editor.message = None;
                refresh_template_names(editor);
                schedule_validation = true;
            }
            invalidate_controls_cache(editor);
        }
        Err(error) => {
            editor.message = Some((true, format!("Not changed: {error:#}")));
        }
    }
    if schedule_validation {
        schedule_pane_template_validation(zetta, cx);
    } else {
        cx.notify();
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/settings_ui/pane_templates.rs"]
mod tests;
