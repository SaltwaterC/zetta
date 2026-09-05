use super::*;

impl Zetta {
    pub(crate) fn fetch_theme_extensions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        if editor.theme_extensions_loading {
            return;
        }
        let query = editor.theme_extension_query.text.trim().to_owned();
        if query.is_empty() {
            editor.theme_extensions.clear();
            editor.theme_extensions_searched = false;
            editor.message = Some((false, "Enter a theme name to search.".to_owned()));
            cx.notify();
            return;
        }
        editor.theme_extensions_loading = true;
        editor.theme_extensions_searched = true;
        editor.message = None;
        let http = cx.http_client();
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let result = theme_extensions::fetch(http, &query).await;
                this.update_in(cx, |this, _, cx| {
                    let Some(editor) = this.settings_editor.as_mut() else {
                        return;
                    };
                    editor.theme_extensions_loading = false;
                    match result {
                        Ok(extensions) => editor.theme_extensions = extensions,
                        Err(error) => {
                            editor.message =
                                Some((true, format!("Could not load themes: {error:#}")));
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
    }

    pub(crate) fn download_theme_extension(
        &mut self,
        extension_id: Arc<str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        if editor.theme_extension_downloading.is_some() {
            return;
        }
        let Some(extension) = editor
            .theme_extensions
            .iter()
            .find(|extension| extension.id == extension_id)
            .cloned()
        else {
            return;
        };
        editor.theme_extension_downloading = Some(extension_id);
        editor.message = Some((false, format!("Downloading {}…", extension.name)));
        let name = extension.name.clone();
        let http = cx.http_client();
        let themes_dir = config::themes_dir();
        let executor = cx.background_executor().clone();
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let result = theme_extensions::download(
                    http,
                    &extension,
                    &themes_dir,
                    executor.clone(),
                )
                .await;
                let installed_theme_extensions = if result.is_ok() {
                    executor
                        .spawn(async move { theme_extensions::installed(&themes_dir) })
                        .await
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                this.update_in(cx, |this, window, cx| {
                    if let Some(editor) = this.settings_editor.as_mut() {
                        editor.theme_extension_downloading = None;
                    }
                    match result {
                        Ok(count) => {
                            this.reload_configuration(&ReloadConfiguration, window, cx);
                            let mut themes = ThemeRegistry::global(cx)
                                .list()
                                .into_iter()
                                .map(|theme| theme.name.to_string())
                                .collect::<Vec<_>>();
                            themes.sort();
                            themes.dedup();
                            if let Some(editor) = this.settings_editor.as_mut() {
                                editor.installed_theme_extensions = installed_theme_extensions;
                                editor.themes = themes.into();
                                Self::refresh_open_dropdown_options(editor);
                                editor.message = Some((
                                    false,
                                    format!(
                                        "Installed {name} ({count} theme file{}). Theme selectors have been reloaded.",
                                        if count == 1 { "" } else { "s" }
                                    ),
                                ));
                            }
                            this.settings_focus.focus(window, cx);
                        }
                        Err(error) => {
                            if let Some(editor) = this.settings_editor.as_mut() {
                                editor.message =
                                    Some((true, format!("Could not install {name}: {error:#}")));
                            }
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
    }

    pub(crate) fn remove_theme_extension(
        &mut self,
        extension_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(installed) = self
            .settings_editor
            .as_ref()
            .and_then(|editor| (editor.theme_extension_downloading.is_none()).then_some(editor))
            .and_then(|editor| {
                editor
                    .installed_theme_extensions
                    .iter()
                    .find(|extension| extension.id == extension_id)
            })
            .cloned()
        else {
            return;
        };
        let in_use = self.settings_editor.as_ref().is_some_and(|editor| {
            installed.theme_names.iter().any(|theme| {
                editor.configuration.theme == *theme
                    || editor.configuration.profiles.iter().any(|profile| {
                        profile
                            .theme
                            .as_ref()
                            .is_some_and(|selected| selected == theme)
                    })
            })
        });
        if in_use {
            if let Some(editor) = self.settings_editor.as_mut() {
                editor.message = Some((
                    true,
                    "Choose and save replacement application/profile themes before removing this extension."
                        .to_owned(),
                ));
            }
            cx.notify();
            return;
        }
        if let Some(editor) = self.settings_editor.as_mut() {
            editor.theme_extension_downloading = Some(Arc::from(extension_id.clone()));
            editor.message = Some((false, format!("Removing {extension_id}…")));
        }

        let themes_dir = config::themes_dir();
        let executor = cx.background_executor().clone();
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let id_for_work = extension_id.clone();
                let result = executor
                    .spawn(async move {
                        let count = theme_extensions::remove(&id_for_work, &themes_dir)?;
                        let installed = theme_extensions::installed(&themes_dir)?;
                        anyhow::Ok((count, installed))
                    })
                    .await;
                this.update_in(cx, |this, window, cx| {
                    if let Some(editor) = this.settings_editor.as_mut() {
                        editor.theme_extension_downloading = None;
                    }
                    match result {
                        Ok((count, installed_theme_extensions)) => {
                            let theme_names = installed
                                .theme_names
                                .iter()
                                .cloned()
                                .map(SharedString::from)
                                .collect::<Vec<_>>();
                            let registry = ThemeRegistry::global(cx);
                            registry.remove_user_themes(&theme_names);
                            theme_settings::load_bundled_themes(&registry);
                            // Installing or removing an extension changes what
                            // a theme name resolves to.
                            crate::process_control::bump_pane_theme_revision();
                            this.reload_configuration(&ReloadConfiguration, window, cx);

                            let mut themes = ThemeRegistry::global(cx)
                                .list()
                                .into_iter()
                                .map(|theme| theme.name.to_string())
                                .collect::<Vec<_>>();
                            themes.sort();
                            themes.dedup();
                            if let Some(editor) = this.settings_editor.as_mut() {
                                editor.themes = themes.into();
                                editor.installed_theme_extensions = installed_theme_extensions;
                                Self::refresh_open_dropdown_options(editor);
                                editor.message = Some((
                                    false,
                                    format!(
                                        "Removed {extension_id} ({count} theme file{}). Theme selectors have been reloaded.",
                                        if count == 1 { "" } else { "s" }
                                    ),
                                ));
                            }
                            this.settings_focus.focus(window, cx);
                        }
                        Err(error) => {
                            if let Some(editor) = this.settings_editor.as_mut() {
                                editor.message = Some((
                                    true,
                                    format!("Could not remove {extension_id}: {error:#}"),
                                ));
                            }
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
    }
}
