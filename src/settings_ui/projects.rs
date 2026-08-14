use super::*;
use crate::project::{
    ProjectConfig, ProjectRegistry, canonical_project_root, ensure_project_config,
};

impl Zetta {
    fn refresh_settings_project_roots(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = self.settings_editor.as_mut() {
            editor.project_roots = self.projects.registry.roots().to_vec().into();
            editor.message = None;
            editor.focused_control = Some(SettingsControl::Tab(SettingsPage::Projects));
            invalidate_controls_cache(editor);
        }
        cx.notify();
    }

    pub(crate) fn add_project_from_settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selection = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Select a Zetta project root".into()),
        });
        let base = self.launch_config.clone();
        let registry_path = self.projects.registry.path().to_path_buf();
        let executor = cx.background_executor().clone();
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let Some(root) = selection
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .flatten()
                    .and_then(|mut paths| paths.pop())
                else {
                    return;
                };
                let result = executor
                    .spawn(async move {
                        let root = canonical_project_root(&root)?;
                        ensure_project_config(&root)?;
                        let config = ProjectConfig::load(&root, &base)?;
                        let mut registry = ProjectRegistry::load_from(registry_path)?;
                        if registry.add(&root)? {
                            registry.save()?;
                        }
                        Ok::<_, anyhow::Error>((registry, config))
                    })
                    .await;
                this.update_in(cx, |this, window, cx| match result {
                    Ok((registry, config)) => {
                        this.projects.registry = registry;
                        this.projects.insert_config(config);
                        this.refresh_settings_project_roots(cx);
                        this.reschedule_project_detection(window, cx);
                        reload_projects_in_other_windows(window.window_handle().window_id(), cx);
                    }
                    Err(error) => {
                        if let Some(editor) = this.settings_editor.as_mut() {
                            editor.message =
                                Some((true, format!("Could not add project: {error:#}")));
                        }
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
    }

    pub(crate) fn open_project_from_settings(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self
            .settings_editor
            .as_ref()
            .and_then(|editor| editor.project_roots.get(index))
            .cloned()
        else {
            return;
        };
        self.dismiss_settings(window, cx);
        self.open_project_tab(root, window, cx);
    }

    pub(crate) fn edit_project_from_settings(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self
            .settings_editor
            .as_ref()
            .and_then(|editor| editor.project_roots.get(index))
            .map(|root| ProjectConfig::path_for(root))
        else {
            return;
        };
        self.dismiss_settings(window, cx);
        self.edit_settings_file_in_active_pane(path, window, cx);
    }

    pub(crate) fn remove_project_from_settings(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self
            .settings_editor
            .as_ref()
            .and_then(|editor| editor.project_roots.get(index))
            .cloned()
        else {
            return;
        };
        let registry_path = self.projects.registry.path().to_path_buf();
        let executor = cx.background_executor().clone();
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let removed_root = root.clone();
                let result = executor
                    .spawn(async move {
                        let mut registry = ProjectRegistry::load_from(registry_path)?;
                        registry.remove(&removed_root).with_context(|| {
                            format!("{} is not a registered project", removed_root.display())
                        })?;
                        registry.save()?;
                        Ok::<_, anyhow::Error>(registry)
                    })
                    .await;
                this.update_in(cx, |this, window, cx| match result {
                    Ok(registry) => {
                        this.projects.registry = registry;
                        this.projects.suppress_offer_for(&root);
                        this.projects.invalidate_active_context();
                        this.projects.clear_removed_roots();
                        this.refresh_settings_project_roots(cx);
                        this.activate_current_project(window, cx);
                        this.reschedule_project_detection(window, cx);
                        reload_projects_in_other_windows(window.window_handle().window_id(), cx);
                    }
                    Err(error) => {
                        if let Some(editor) = this.settings_editor.as_mut() {
                            editor.message = Some((
                                true,
                                format!("Could not remove project {}: {error:#}", root.display()),
                            ));
                        }
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
    }
}
