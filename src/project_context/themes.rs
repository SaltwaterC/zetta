//! The theme and tab icon a tab or pane ends up with once its project, its
//! profile and the window's appearance have each had their say, and
//! re-applying them when any of those changes.

use super::*;

pub(crate) const fn resolve_tab_icon(
    effective_icon: Option<IconName>,
    icon_override: TabIconOverride,
    policy: ProjectContextPolicy,
    application_default: Option<IconName>,
) -> Option<IconName> {
    match icon_override {
        TabIconOverride::Icon(icon) => Some(icon),
        TabIconOverride::Hidden => None,
        TabIconOverride::None if policy.is_remote() => application_default,
        TabIconOverride::None => effective_icon,
    }
}

pub(super) fn reset_remote_tab_icon(
    tab_id: u64,
    icon_override: &mut TabIconOverride,
    inherited_tab_icons: &mut HashMap<u64, Option<IconName>>,
) {
    // `icon` is deliberately not an argument: it is shared canonical state
    // and may have been derived from another viewer's project. Each remote
    // viewer resolves its own application default at display time.
    inherited_tab_icons.remove(&tab_id);
    *icon_override = TabIconOverride::None;
}

/// Applies the active project's icon when a tab has no explicit user choice.
/// The first time a project applies, the tab's current icon is snapshotted
/// into `inherited_tab_icons` so leaving the project can restore it. Explicit
/// icon choices, including an explicit hidden icon, stay effective across
/// entering, leaving, and switching projects. Callers must never seed a new
/// tab's icon from the project it is opened directly into (see
/// `Zetta::open_tab_with_profile_context`) — doing so snapshots the project's
/// own icon instead of the tab's true default, so leaving the project later
/// restores the wrong value.
pub(crate) fn apply_project_tab_icon(
    tab_id: u64,
    tab_icon: &mut Option<IconName>,
    tab_icon_override: TabIconOverride,
    project: Option<&ProjectConfig>,
    inherited_tab_icons: &mut HashMap<u64, Option<IconName>>,
) {
    if !matches!(tab_icon_override, TabIconOverride::None) {
        return;
    }

    match project {
        Some(project) => {
            inherited_tab_icons.entry(tab_id).or_insert(*tab_icon);
            *tab_icon = project.effective.default_tab_icon;
        }
        None => {
            if let Some(icon) = inherited_tab_icons.remove(&tab_id) {
                *tab_icon = icon;
            }
        }
    }
}

/// Clears a tab's explicit icon choice and reapplies the project fallback.
/// Removing the inherited value first is important when a previous project
/// application left stale state behind: the application default is the value
/// that leaving the current project must restore.
pub(crate) fn reset_project_tab_icon(
    tab_id: u64,
    tab_icon: &mut Option<IconName>,
    tab_icon_override: &mut TabIconOverride,
    default_tab_icon: Option<IconName>,
    project: Option<&ProjectConfig>,
    inherited_tab_icons: &mut HashMap<u64, Option<IconName>>,
) {
    inherited_tab_icons.remove(&tab_id);
    *tab_icon = default_tab_icon;
    *tab_icon_override = TabIconOverride::None;
    apply_project_tab_icon(
        tab_id,
        tab_icon,
        *tab_icon_override,
        project,
        inherited_tab_icons,
    );
}

/// The theme a terminal pane should render with. Mirrors the precedence
/// [`Zetta::window_theme`]/[`Zetta::theme_for_tab`] use for the tab/window
/// chrome: an active project's own `theme` wins outright, because that is the
/// project asserting a theme for everything inside it, including profiles it
/// never mentions and that therefore still carry whatever `theme` they
/// inherited from the global configuration. Only when no project is active,
/// or the active project sets no `theme` of its own, does the profile's theme
/// apply.
pub(crate) fn resolve_project_profile_theme(
    profile: &Profile,
    project: Option<&ProjectConfig>,
    cx: &App,
) -> Result<Option<Arc<Theme>>> {
    if let Some(name) = project.and_then(|project| project_theme_name(project, cx)) {
        return ThemeRegistry::global(cx)
            .get(name)
            .with_context(|| format!("using project theme {name:?}"))
            .map(Some);
    }
    resolve_profile_theme(profile, cx)
}

/// The configured fallback below pane and tab overrides. Local panes consult
/// project/profile configuration; remote panes use this viewer's application
/// theme because their paths and launch profiles belong to another machine.
#[derive(Clone, Copy)]
pub(crate) enum TerminalThemeFallback<'a> {
    Configured {
        profile: &'a Profile,
        project: Option<&'a ProjectConfig>,
    },
    Application(&'a Arc<Theme>),
}

impl<'a> TerminalThemeFallback<'a> {
    #[cfg(test)]
    pub(crate) const fn configured(
        profile: &'a Profile,
        project: Option<&'a ProjectConfig>,
    ) -> Self {
        Self::Configured { profile, project }
    }
}

/// Resolves terminal content styling in one place. Explicit pane state wins
/// over project and profile configuration; an absent result means the
/// application theme selected by the caller/view remains in effect.
/// The theme names a pane and its tab were explicitly set to, if any.
#[derive(Clone, Copy, Default)]
pub(crate) struct ThemeOverrides<'a> {
    pub(crate) pane: Option<&'a str>,
    pub(crate) tab: Option<&'a str>,
}

pub(crate) fn resolve_terminal_theme(
    pane_theme_override: Option<&str>,
    tab_theme_override: Option<&str>,
    fallback: TerminalThemeFallback<'_>,
    cx: &App,
) -> Result<Option<Arc<Theme>>> {
    if let Some(name) = pane_theme_override {
        return ThemeRegistry::global(cx)
            .get(name)
            .with_context(|| format!("using pane theme {name:?}"))
            .map(Some);
    }
    if let Some(name) = tab_theme_override {
        return ThemeRegistry::global(cx)
            .get(name)
            .with_context(|| format!("using tab theme {name:?}"))
            .map(Some);
    }
    match fallback {
        TerminalThemeFallback::Configured { profile, project } => {
            resolve_project_profile_theme(profile, project, cx)
        }
        TerminalThemeFallback::Application(theme) => Ok(Some(theme.clone())),
    }
}

pub(super) fn project_theme_name<'a>(project: &'a ProjectConfig, cx: &App) -> Option<&'a str> {
    if SystemAppearance::global(cx).is_light() {
        project.theme.as_deref()
    } else {
        project.dark_theme.as_deref()
    }
}

pub(super) fn refresh_terminal_themes_in_tab(
    tab: &mut Tab,
    projects: &HashMap<u64, Arc<ProjectConfig>>,
    policy: ProjectContextPolicy,
    application_theme: &Arc<Theme>,
    cx: &mut Context<Zetta>,
) {
    let refresh = ThemeRefreshContext {
        policy,
        application_theme,
    };
    for pane in &mut tab.panes {
        refresh_terminal_theme_for_profile(
            pane.theme_override.as_deref(),
            tab.theme_override.as_deref(),
            &mut pane.profile,
            pane.view.clone(),
            projects.get(&pane.id).map(Arc::as_ref),
            refresh,
            cx,
        );
        for entry in &mut pane.stack.entries {
            refresh_terminal_theme_for_profile(
                entry.theme_override.as_deref(),
                tab.theme_override.as_deref(),
                &mut entry.profile,
                entry.view.clone(),
                projects.get(&entry.id).map(Arc::as_ref),
                refresh,
                cx,
            );
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct ThemeRefreshContext<'a> {
    pub(super) policy: ProjectContextPolicy,
    pub(super) application_theme: &'a Arc<Theme>,
}

pub(super) fn terminal_projects_for_tab(
    tab: &Tab,
    projects: &ProjectState,
) -> HashMap<u64, Arc<ProjectConfig>> {
    tab.panes
        .iter()
        .flat_map(|pane| {
            std::iter::once(pane.id).chain(pane.stack.entries.iter().map(|entry| entry.id))
        })
        .filter_map(|pane_id| {
            projects
                .config_for_pane(pane_id)
                .cloned()
                .map(|project| (pane_id, project))
        })
        .collect()
}

pub(super) fn refresh_terminal_theme_for_profile(
    pane_theme_override: Option<&str>,
    tab_theme_override: Option<&str>,
    profile: &mut Profile,
    view: Option<Entity<TerminalView>>,
    project: Option<&ProjectConfig>,
    refresh: ThemeRefreshContext<'_>,
    cx: &mut Context<Zetta>,
) {
    let fallback = refresh
        .policy
        .theme_fallback(profile, project, refresh.application_theme);
    let theme = resolve_terminal_theme(pane_theme_override, tab_theme_override, fallback, cx)
        .ok()
        .flatten();
    if let Some(view) = view {
        view.update(cx, |view, cx| view.set_theme(theme, cx));
    }
}

impl Zetta {
    /// Applies the window's current system appearance and refreshes every
    /// terminal view that follows configuration. This is deliberately driven
    /// by GPUI's appearance notification rather than by render or timer work.
    pub(crate) fn handle_window_appearance_change(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        *SystemAppearance::global_mut(cx) = SystemAppearance(window.appearance().into());
        // Every pane's resolved theme can flip with the system appearance, and
        // nothing about that is user-initiated.
        crate::process_control::bump_pane_theme_revision();
        let theme = self.window_theme(cx);
        GlobalTheme::update_theme(cx, theme);
        self.refresh_terminal_themes_for_appearance(cx);
        cx.notify();
    }

    /// The theme this window's *launch* configuration selects, i.e. what
    /// `apply_config_settings` installed globally at startup.
    ///
    /// This is the fallback for everything that has no project or profile theme
    /// of its own. `cx.theme()` is not usable for that any more: `Zetta::render`
    /// repoints the global theme at the rendering window's project theme so that
    /// Zed's `ui` components follow it (see `Render for Zetta`), so in a
    /// multi-window process `cx.theme()` is whichever window drew last.
    /// The theme for a pane of `tab_id` running `profile`: its own override,
    /// else the tab's, else the profile → project → application fallback.
    pub(crate) fn pane_theme(
        &self,
        tab_id: u64,
        profile: &Profile,
        project: Option<&ProjectConfig>,
        overrides: ThemeOverrides<'_>,
        cx: &App,
    ) -> Result<Option<Arc<Theme>>> {
        let application_theme = self.application_theme(cx);
        let fallback = self.project_context_policy(tab_id).theme_fallback(
            profile,
            project,
            &application_theme,
        );
        resolve_terminal_theme(overrides.pane, overrides.tab, fallback, cx)
    }

    pub(crate) fn application_theme(&self, cx: &App) -> Arc<Theme> {
        ThemeRegistry::global(cx)
            .get(selected_theme_name_for_appearance(&self.launch_config, cx))
            .unwrap_or_else(|_| cx.theme().clone())
    }

    pub(crate) fn window_theme(&self, cx: &App) -> Arc<Theme> {
        self.active_project_config()
            .and_then(|project| project_theme_name(project, cx))
            .and_then(|name| ThemeRegistry::global(cx).get(name).ok())
            .unwrap_or_else(|| self.application_theme(cx))
    }

    pub(crate) fn theme_for_tab(&self, tab: &Tab, cx: &App) -> Arc<Theme> {
        self.theme_for_tab_with_policy(tab, self.project_context_policy(tab.id), cx)
    }

    pub(crate) fn theme_for_tab_with_policy(
        &self,
        tab: &Tab,
        policy: ProjectContextPolicy,
        cx: &App,
    ) -> Arc<Theme> {
        let override_theme = tab
            .theme_override
            .as_deref()
            .and_then(|name| ThemeRegistry::global(cx).get(name).ok());
        if policy.is_remote() {
            return override_theme.unwrap_or_else(|| self.application_theme(cx));
        }
        override_theme
            .or_else(|| {
                self.projects
                    .config_for_pane(tab.active_pane)
                    .and_then(|project| project_theme_name(project, cx))
                    .and_then(|name| ThemeRegistry::global(cx).get(name).ok())
            })
            .unwrap_or_else(|| tab.theme(cx, || self.application_theme(cx)))
    }

    /// The icon this viewer displays for a tab. A remote tab's serialized
    /// effective icon may have come from another viewer's project; mask that
    /// value without mutating shared session state.
    pub(crate) fn resolved_tab_icon(&self, tab: &Tab) -> Option<IconName> {
        self.resolved_tab_icon_with_policy(tab, self.project_context_policy(tab.id))
    }

    pub(crate) fn resolved_tab_icon_with_policy(
        &self,
        tab: &Tab,
        policy: ProjectContextPolicy,
    ) -> Option<IconName> {
        resolve_tab_icon(
            tab.icon,
            tab.icon_override,
            policy,
            self.launch_config.default_tab_icon,
        )
    }

    /// Reapplies the active project's effective icon without changing project
    /// context bookkeeping. Configuration reloads use this path because they
    /// do not have a window with which to call `activate_current_project`.
    pub(crate) fn refresh_active_project_tab_icon(&mut self, project: Option<&ProjectConfig>) {
        if self
            .current_tab()
            .is_some_and(|tab| self.project_context_policy(tab.id).is_remote())
        {
            return;
        }
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            apply_project_tab_icon(
                tab.id,
                &mut tab.icon,
                tab.icon_override,
                project,
                &mut self.projects.inherited_tab_icons,
            );
        }
    }

    pub(crate) fn reset_project_tab_icon_at(&mut self, tab_index: usize) -> bool {
        let default_tab_icon = self.launch_config.default_tab_icon;
        let project = self
            .tabs
            .get(tab_index)
            .and_then(|tab| self.project_config_for_tab(tab.id))
            .cloned();
        let policy = self
            .tabs
            .get(tab_index)
            .map_or(ProjectContextPolicy::Local, |tab| {
                self.project_context_policy(tab.id)
            });
        let Some(tab) = self.tabs.get_mut(tab_index) else {
            return false;
        };
        if policy.is_remote() {
            reset_remote_tab_icon(
                tab.id,
                &mut tab.icon_override,
                &mut self.projects.inherited_tab_icons,
            );
            return true;
        }
        reset_project_tab_icon(
            tab.id,
            &mut tab.icon,
            &mut tab.icon_override,
            default_tab_icon,
            project.as_deref(),
            &mut self.projects.inherited_tab_icons,
        );
        true
    }

    pub(crate) fn apply_effective_themes_to_tab(&mut self, tab_id: u64, cx: &mut Context<Self>) {
        let Some(tab_index) = self.tab_index(tab_id) else {
            return;
        };
        let policy = self.project_context_policy(tab_id);
        let projects = self.tabs[tab_index]
            .panes
            .iter()
            .flat_map(|pane| {
                std::iter::once(pane.id).chain(pane.stack.entries.iter().map(|entry| entry.id))
            })
            .filter_map(|pane_id| {
                if policy.is_remote() {
                    return None;
                }
                self.projects
                    .config_for_pane(pane_id)
                    .cloned()
                    .map(|project| (pane_id, project))
            })
            .collect::<HashMap<_, _>>();
        let application_theme = self.application_theme(cx);
        let tab = &mut self.tabs[tab_index];
        for pane in &mut tab.panes {
            let project = projects.get(&pane.id);
            let configured_profiles = project.map_or(&self.launch_config.profiles, |project| {
                &project.effective.profiles
            });
            let configured_profile = configured_profiles
                .iter()
                .find(|profile| profile.name.eq_ignore_ascii_case(&pane.profile.name));
            if let Some(profile) = configured_profile {
                pane.profile.theme = profile.theme.clone();
                pane.profile.dark_theme = profile.dark_theme.clone();
            } else {
                pane.profile.theme = None;
                pane.profile.dark_theme = None;
            }
            crate::app::apply_launch_theme_override(
                &mut pane.profile,
                self.launch_theme_override.as_ref(),
            );
            let fallback =
                policy.theme_fallback(&pane.profile, project.map(Arc::as_ref), &application_theme);
            let theme = resolve_terminal_theme(
                pane.theme_override.as_deref(),
                tab.theme_override.as_deref(),
                fallback,
                cx,
            )
            .ok()
            .flatten();
            if let Some(view) = pane.view.clone() {
                view.update(cx, |view, cx| view.set_theme(theme.clone(), cx));
            }
            for entry in &mut pane.stack.entries {
                let project = projects.get(&entry.id);
                let configured_profiles = project.map_or(&self.launch_config.profiles, |project| {
                    &project.effective.profiles
                });
                let configured_profile = configured_profiles
                    .iter()
                    .find(|profile| profile.name.eq_ignore_ascii_case(&entry.profile.name));
                if let Some(profile) = configured_profile {
                    entry.profile.theme = profile.theme.clone();
                    entry.profile.dark_theme = profile.dark_theme.clone();
                } else {
                    entry.profile.theme = None;
                    entry.profile.dark_theme = None;
                }
                crate::app::apply_launch_theme_override(
                    &mut entry.profile,
                    self.launch_theme_override.as_ref(),
                );
                let fallback = policy.theme_fallback(
                    &entry.profile,
                    project.map(Arc::as_ref),
                    &application_theme,
                );
                let theme = resolve_terminal_theme(
                    entry.theme_override.as_deref(),
                    tab.theme_override.as_deref(),
                    fallback,
                    cx,
                )
                .ok()
                .flatten();
                if let Some(view) = entry.view.clone() {
                    view.update(cx, |view, cx| view.set_theme(theme, cx));
                }
            }
        }
    }

    /// Refreshes the live terminals for a session-scoped theme change without
    /// replacing the profiles already attached to pane-template leaves. The
    /// full effective-theme path above is for project/configuration changes;
    /// this path only needs to re-resolve the precedence chain for each view.
    pub(crate) fn refresh_terminal_themes_in_tab(&mut self, tab_id: u64, cx: &mut Context<Self>) {
        let Some(tab_index) = self.tab_index(tab_id) else {
            return;
        };
        let policy = self.project_context_policy(tab_id);
        let projects = if policy.is_remote() {
            HashMap::new()
        } else {
            terminal_projects_for_tab(&self.tabs[tab_index], &self.projects)
        };
        let application_theme = self.application_theme(cx);
        refresh_terminal_themes_in_tab(
            &mut self.tabs[tab_index],
            &projects,
            policy,
            &application_theme,
            cx,
        );
    }

    pub(crate) fn refresh_terminal_themes_for_appearance(&mut self, cx: &mut Context<Self>) {
        // The pane profiles already contain the effective configuration used
        // when each pane was opened. Keep those values intact here: pane
        // template leaf overrides live on the pane profile, while the
        // appearance change only selects which of its two fields is active.
        for index in 0..self.tabs.len() {
            let policy = self.project_context_policy(self.tabs[index].id);
            let projects = if policy.is_remote() {
                HashMap::new()
            } else {
                terminal_projects_for_tab(&self.tabs[index], &self.projects)
            };
            let application_theme = self.application_theme(cx);
            refresh_terminal_themes_in_tab(
                &mut self.tabs[index],
                &projects,
                policy,
                &application_theme,
                cx,
            );
        }

        let background_projects = self
            .background_sessions
            .iter()
            .map(|tab| terminal_projects_for_tab(tab, &self.projects))
            .collect::<Vec<_>>();
        for (tab, projects) in self.background_sessions.iter_mut().zip(background_projects) {
            let application_theme = ThemeRegistry::global(cx)
                .get(selected_theme_name_for_appearance(&self.launch_config, cx))
                .unwrap_or_else(|_| cx.theme().clone());
            refresh_terminal_themes_in_tab(
                tab,
                &projects,
                ProjectContextPolicy::Local,
                &application_theme,
                cx,
            );
        }
    }
}
