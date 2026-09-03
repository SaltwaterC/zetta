//! Turning a session that has been handed back into the tab a window shows.
//!
//! A restored pane's working directory, project binding, profile and theme all
//! come from the durable state rather than from the current configuration, so
//! reconnecting cannot silently re-point a pane at a different project or
//! theme. `RestoredPaneMetadata` is what carries that across.

use super::*;

impl Zetta {
    pub(crate) fn take_background_session_by_id(
        &mut self,
        session_id: u64,
        authorization: Option<&VerifiedSession>,
        cx: &mut Context<Self>,
    ) -> Option<Tab> {
        let index = self
            .background_sessions
            .iter()
            .position(|tab| tab.id == session_id)?;
        match (
            self.background_sessions.authentication_at(index),
            authorization,
        ) {
            (None, None) => {}
            (Some(expected), Some(supplied)) if expected.authorizes(supplied) => {}
            _ => return None,
        }
        let tab = self.background_sessions.reconnect_at(index)?;
        self.publish_background_session_catalog(cx);
        Some(tab)
    }

    /// Gives a tab arriving from elsewhere the project of the pane this window is
    /// on, the way every other route to a new pane does.
    ///
    /// A pane's theme is resolved once, when its view is built, and it resolves
    /// through the pane's project. A tab attached from the multiplexer or handed
    /// over by another window had no project recorded for its panes at all, so
    /// the theme fell back to the default and only corrected itself when
    /// detection eventually reported a root — which is the next time the pane's
    /// title changes, so a joined session showing a full-screen program kept the
    /// wrong theme until that program quit.
    ///
    /// Inheriting is the same answer `split_pane` and the command panes use: the
    /// window's own context is the best guess, and detection still corrects it
    /// if the session's shell turns out to be somewhere else.
    fn inherit_project_for_incoming_panes(&mut self, tab: &Tab) {
        let Some(source) = self
            .tabs
            .get(self.active_tab)
            .map(|active| active.active_pane)
        else {
            return;
        };
        inherit_project_for_panes(&mut self.projects, source, tab);
    }

    /// Resolves saved pane directories before any terminal view is built.
    /// Each distinct project config root is read once, so all panes in a
    /// restore see the same latest project configuration without multiplying
    /// I/O by pane count.
    pub(super) fn prepare_restored_panes(
        &mut self,
        panes: Vec<(u64, String, Option<PathBuf>)>,
    ) -> RestoredPaneMetadata {
        let destination_root = self
            .active_project_config()
            .map(|project| project.root.clone());
        let mut metadata = RestoredPaneMetadata::default();
        for (routing_id, profile_name, working_directory) in panes {
            let profile = self
                .launch_config
                .profiles
                .iter()
                .find(|profile| profile.name.eq_ignore_ascii_case(&profile_name))
                .cloned()
                .unwrap_or_else(|| Profile {
                    name: profile_name,
                    command: task::Shell::System,
                    theme: None,
                    dark_theme: None,
                    icon: ProfileIcon::default(),
                });
            let project_root = match working_directory.as_deref() {
                Some(directory) => resolve_registered_project_config_root(
                    &restored_project_directory(&profile, directory),
                    &self.projects.registry,
                ),
                None => destination_root.clone(),
            };
            metadata.panes.insert(
                routing_id,
                RestoredPaneInfo {
                    working_directory,
                    project_root,
                },
            );
        }

        let roots = metadata
            .panes
            .values()
            .filter_map(|pane| pane.project_root.clone())
            .collect::<HashSet<_>>();
        let mut unavailable = HashSet::new();
        for root in roots {
            match ProjectConfig::load(&root, &self.launch_config) {
                Ok(project) => {
                    self.projects.insert_config(project);
                }
                Err(error) => {
                    unavailable.insert(root.clone());
                    self.projects.configs.remove(&root);
                    self.configuration_error = Some(format!(
                        "Could not restore project configuration {}: {error:#}",
                        ProjectConfig::path_for(&root).display()
                    ));
                }
            }
        }
        for pane in metadata.panes.values_mut() {
            if pane
                .project_root
                .as_ref()
                .is_some_and(|root| unavailable.contains(root))
            {
                pane.project_root = None;
            }
        }
        metadata
    }

    pub(super) fn restored_profiles(
        &self,
        panes: &[(u64, String, Option<PathBuf>)],
        metadata: &RestoredPaneMetadata,
    ) -> HashMap<u64, Profile> {
        panes
            .iter()
            .map(|(routing_id, name, _)| {
                let profiles = metadata
                    .project_root(*routing_id)
                    .and_then(|root| self.projects.configs.get(root))
                    .map(|project| &project.effective.profiles)
                    .unwrap_or(&self.launch_config.profiles);
                let mut profile = profiles
                    .iter()
                    .find(|profile| profile.name.eq_ignore_ascii_case(name))
                    .cloned()
                    .unwrap_or_else(|| Profile {
                        name: name.clone(),
                        command: task::Shell::System,
                        theme: None,
                        dark_theme: None,
                        icon: ProfileIcon::default(),
                    });
                crate::app::apply_launch_theme_override(
                    &mut profile,
                    self.launch_theme_override.as_ref(),
                );
                (*routing_id, profile)
            })
            .collect()
    }

    pub(super) fn bind_restored_projects(&mut self, tab: &Tab, metadata: &RestoredPaneMetadata) {
        let mut roots = HashSet::new();
        for pane in &tab.panes {
            if let Some(root) = metadata.project_root(pane.routing_id) {
                self.projects.pane_roots.insert(pane.id, root.clone());
                roots.insert(root.clone());
            }
            for entry in &pane.stack.entries {
                if let Some(root) = metadata.project_root(entry.routing_id) {
                    self.projects.pane_roots.insert(entry.id, root.clone());
                    roots.insert(root.clone());
                }
            }
        }
        for root in roots {
            self.projects.mark_entered(tab.id, &root);
        }
    }

    pub(super) fn restored_terminal_theme(
        &mut self,
        pane_theme_override: Option<&str>,
        tab_theme_override: Option<&str>,
        profile: &Profile,
        project: Option<&ProjectConfig>,
        cx: &App,
    ) -> Option<Arc<Theme>> {
        if let Some(name) = pane_theme_override {
            match ThemeRegistry::global(cx).get(name) {
                Ok(theme) => return Some(theme),
                Err(error) => {
                    self.configuration_error =
                        Some(format!("Could not restore pane theme {name:?}: {error:#}"));
                }
            }
        }
        if let Some(name) = tab_theme_override {
            match ThemeRegistry::global(cx).get(name) {
                Ok(theme) => return Some(theme),
                Err(error) => {
                    self.configuration_error =
                        Some(format!("Could not restore tab theme {name:?}: {error:#}"));
                }
            }
        }
        match resolve_project_profile_theme(profile, project, cx) {
            Ok(theme) => theme.or_else(|| Some(self.application_theme(cx))),
            Err(error) => {
                self.configuration_error = Some(format!(
                    "Could not restore configured terminal theme: {error:#}"
                ));
                resolve_profile_theme(profile, cx)
                    .ok()
                    .flatten()
                    .or_else(|| Some(self.application_theme(cx)))
            }
        }
    }

    pub(crate) fn attach_reconnected_tab(
        &mut self,
        mut tab: Tab,
        transferred: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let restored_panes = tab
            .panes
            .iter()
            .flat_map(|pane| {
                let base = std::iter::once((
                    pane.routing_id,
                    pane.profile.name.clone(),
                    pane.working_directory(cx),
                ));
                let stacked = pane.stack.entries.iter().map(|entry| {
                    (
                        entry.routing_id,
                        entry.profile.name.clone(),
                        entry
                            .terminal
                            .as_ref()
                            .and_then(|terminal| terminal.read(cx).working_directory())
                            .or_else(|| entry.working_directory.clone())
                            .or_else(|| entry.wsl_directory.as_deref().map(PathBuf::from)),
                    )
                });
                base.chain(stacked)
            })
            .collect::<Vec<_>>();
        let restored_metadata = self.prepare_restored_panes(restored_panes.clone());
        let profiles = self.restored_profiles(&restored_panes, &restored_metadata);
        for pane in &mut tab.panes {
            if let Some(profile) = profiles.get(&pane.routing_id) {
                pane.profile = profile.clone();
            }
            for entry in &mut pane.stack.entries {
                if let Some(profile) = profiles.get(&entry.routing_id) {
                    entry.profile = profile.clone();
                }
            }
        }
        self.attach_reconnected_tab_with_metadata(
            tab,
            transferred,
            Some(restored_metadata),
            window,
            cx,
        );
    }

    fn attach_reconnected_tab_with_metadata(
        &mut self,
        mut tab: Tab,
        transferred: bool,
        restored: Option<RestoredPaneMetadata>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if transferred {
            let tab_id = self.next_tab_id;
            self.next_tab_id += 1;
            tab.reassign_ids(tab_id, &mut self.next_pane_id);
        }
        self.next_attention_id = self
            .next_attention_id
            .max(tab.attention_id.saturating_add(1));
        if cx.has_global::<ZettaProcessState>() {
            let process = cx.global_mut::<ZettaProcessState>();
            process.next_attention_id = process
                .next_attention_id
                .max(tab.attention_id.saturating_add(1));
        }
        let tab_id = tab.id;
        if let Some(restored) = restored.as_ref() {
            self.bind_restored_projects(&tab, restored);
        } else {
            self.inherit_project_for_incoming_panes(&tab);
        }
        let tab_theme_override = tab.theme_override.clone();
        let panes = tab
            .panes
            .iter()
            .flat_map(|pane| {
                let project = self.projects.config_for_pane(pane.id).cloned();
                let base = pane
                    .terminal
                    .clone()
                    .filter(|_| pane.exit.is_none())
                    .map(|terminal| {
                        (
                            pane.id,
                            None,
                            terminal,
                            pane.theme_override.clone(),
                            tab_theme_override.clone(),
                            pane.profile.clone(),
                            project.clone(),
                        )
                    });
                let stacked = pane
                    .stack
                    .entries
                    .iter()
                    .filter_map(|entry| {
                        let project = self.projects.config_for_pane(entry.id).cloned();
                        Some((
                            pane.id,
                            Some(entry.id),
                            entry.terminal.clone()?,
                            entry.theme_override.clone(),
                            tab_theme_override.clone(),
                            entry.profile.clone(),
                            project,
                        ))
                    })
                    .collect::<Vec<_>>();
                base.into_iter().chain(stacked)
            })
            .collect::<Vec<_>>();
        self.active_tab = insert_tab_in_pin_order(&mut self.tabs, tab);

        for (pane_id, stack_id, terminal, theme_override, tab_theme_override, profile, project) in
            panes
        {
            let theme = self.restored_terminal_theme(
                theme_override.as_deref(),
                tab_theme_override.as_deref(),
                &profile,
                project.as_deref(),
                cx,
            );
            let display_only = !terminal.read(cx).is_pty();
            let view =
                cx.new(|cx| TerminalView::new_with_theme(terminal.clone(), theme, window, cx));
            if display_only {
                view.update(cx, |view, cx| view.set_input_enabled(false, cx));
            }
            if let Some(entry_id) = stack_id {
                self.connect_stacked_terminal_view(tab_id, pane_id, entry_id, view, window, cx);
            } else {
                self.connect_terminal_view(tab_id, pane_id, view, window, cx);
            }
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(crate) fn process_background_session_picker_entries(
        &self,
        cx: &App,
    ) -> Arc<[ProcessBackgroundSessionEntry]> {
        let own_runner = self.background_sessions.runner_id();
        if cx.has_global::<ZettaProcessState>() {
            let entries = cx
                .global::<ZettaProcessState>()
                .background_session_entries
                .clone();
            let shown_here = |entry: &ProcessBackgroundSessionEntry| {
                session_is_already_shown_here(&self.mux_panes, entry, own_runner)
            };
            if !entries.iter().any(shown_here) {
                return entries;
            }
            return entries
                .iter()
                .filter(|entry| !shown_here(entry))
                .cloned()
                .collect::<Vec<_>>()
                .into();
        }
        self.background_session_picker_entries
            .iter()
            .map(|(session_id, title, details)| {
                (own_runner, *session_id, title.clone(), details.clone())
            })
            .collect::<Vec<_>>()
            .into()
    }

    pub(super) fn picker_entries_from_summaries(
        sessions: &[BackgroundSessionSummary],
    ) -> Vec<(u64, String, String)> {
        sessions
            .iter()
            .rev()
            .map(|session| {
                if session.authentication_required {
                    return (
                        session.id,
                        "Protected session".to_owned(),
                        format!("Session {} · protected", session.id),
                    );
                }
                let mut applications = Vec::new();
                for pane in &session.panes {
                    if !applications.contains(&pane.application) {
                        applications.push(pane.application.clone());
                    }
                }
                let pane_count = session.panes.len();
                let mut details = format!(
                    "Session {} · {pane_count} pane{}",
                    session.id,
                    if pane_count == 1 { "" } else { "s" }
                );
                if !applications.is_empty() {
                    details.push_str(" · ");
                    details.push_str(&applications.join(", "));
                }
                // A session another window is showing does not reconnect, it is
                // *joined*: the multiplexer asks that window to hand its
                // terminals over and both then see the same panes. Listing it
                // identically to a detached session made that look like an
                // ordinary reconnect right up until the other window changed.
                if session.held {
                    details.push_str(" · in use elsewhere");
                }
                if let Some(exit) = session.panes.iter().find_map(|pane| pane.exit.as_ref()) {
                    details.push_str(" · failed: ");
                    details.push_str(&exit.reason_text());
                }
                (session.id, session.title.clone(), details)
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "../tests/background_session_ui/restore.rs"]
mod tests;
