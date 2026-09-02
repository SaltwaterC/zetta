use super::*;
use crate::project::{
    ProjectConfig, ProjectRegistry, canonical_project_root, discover_project_config,
    is_registered_project_config_root, paths_equal, resolve_registered_project,
    resolve_registered_project_config_root, resolve_registered_project_root,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectOffer {
    pub(crate) root: PathBuf,
    pane_id: u64,
}

#[derive(Clone, Debug)]
struct ProjectDetectionState {
    directory: PathBuf,
    generation: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectState {
    pub(crate) registry: ProjectRegistry,
    pub(crate) configs: HashMap<PathBuf, Arc<ProjectConfig>>,
    pub(crate) pane_roots: HashMap<u64, PathBuf>,
    detections: HashMap<u64, ProjectDetectionState>,
    next_detection_generation: u64,
    entered: HashSet<(u64, String)>,
    inherited_tab_icons: HashMap<u64, Option<IconName>>,
    active_context: Option<(u64, Option<usize>)>,
    dismissed_offers: HashSet<String>,
    pub(crate) offer: Option<ProjectOffer>,
}

impl ProjectState {
    pub(crate) fn load() -> Result<Self> {
        Ok(Self::new(ProjectRegistry::load()?))
    }

    pub(crate) fn new(registry: ProjectRegistry) -> Self {
        Self {
            registry,
            configs: HashMap::new(),
            pane_roots: HashMap::new(),
            detections: HashMap::new(),
            next_detection_generation: 0,
            entered: HashSet::new(),
            inherited_tab_icons: HashMap::new(),
            active_context: None,
            dismissed_offers: HashSet::new(),
            offer: None,
        }
    }

    pub(crate) fn root_for_pane(&self, pane_id: u64) -> Option<&PathBuf> {
        self.pane_roots.get(&pane_id)
    }

    pub(crate) fn config_for_pane(&self, pane_id: u64) -> Option<&Arc<ProjectConfig>> {
        self.root_for_pane(pane_id).and_then(|root| {
            self.configs
                .iter()
                .find_map(|(config_root, config)| paths_equal(config_root, root).then_some(config))
        })
    }

    pub(crate) fn insert_config(&mut self, config: ProjectConfig) -> Arc<ProjectConfig> {
        let config = Arc::new(config);
        self.configs.insert(config.root.clone(), config.clone());
        config
    }

    pub(crate) fn mark_entered(&mut self, tab_id: u64, root: &Path) -> bool {
        self.entered.insert((tab_id, project_key(root)))
    }

    pub(crate) fn dismiss_offer(&mut self) {
        if let Some(offer) = self.offer.take() {
            self.dismissed_offers.insert(project_key(&offer.root));
        }
    }

    pub(crate) fn suppress_offer_for(&mut self, root: &Path) {
        self.dismissed_offers.insert(project_key(root));
        if let Some(registered_root) = resolve_registered_project_root(root, &self.registry) {
            self.dismissed_offers.insert(project_key(&registered_root));
        }
        let related_offer = self.offer.as_ref().is_some_and(|offer| {
            paths_equal(&offer.root, root)
                || resolve_registered_project_root(&offer.root, &self.registry)
                    .is_some_and(|registered_root| paths_equal(&registered_root, root))
        });
        if related_offer {
            self.offer = None;
        }
    }

    fn offer_is_dismissed(&self, root: &Path) -> bool {
        self.dismissed_offers.contains(&project_key(root))
    }

    pub(crate) fn clear_removed_roots(&mut self) {
        self.pane_roots
            .retain(|_, root| is_registered_project_config_root(root, &self.registry));
        self.configs
            .retain(|root, _| is_registered_project_config_root(root, &self.registry));
        if self.offer.as_ref().is_some_and(|offer| {
            self.registry.contains(&offer.root)
                || resolve_registered_project_root(&offer.root, &self.registry).is_some()
        }) {
            self.offer = None;
        }
    }

    pub(crate) fn invalidate_active_context(&mut self) {
        self.active_context = None;
    }

    pub(crate) fn invalidate_detections(&mut self) {
        self.detections.clear();
    }

    fn begin_detection(&mut self, pane_id: u64, directory: PathBuf) -> Option<u64> {
        if self
            .detections
            .get(&pane_id)
            .is_some_and(|state| paths_equal(&state.directory, &directory))
        {
            return None;
        }
        self.next_detection_generation = self.next_detection_generation.wrapping_add(1);
        let generation = self.next_detection_generation;
        self.detections.insert(
            pane_id,
            ProjectDetectionState {
                directory,
                generation,
            },
        );
        Some(generation)
    }

    pub(crate) fn inherit_pane_root(&mut self, source_pane_id: u64, pane_id: u64) {
        if let Some(root) = self.pane_roots.get(&source_pane_id).cloned() {
            self.pane_roots.insert(pane_id, root);
        }
    }

    fn clear_pane_root(&mut self, pane_id: u64) {
        if let Some(root) = self.pane_roots.remove(&pane_id)
            && !self
                .pane_roots
                .values()
                .any(|candidate| paths_equal(candidate, &root))
        {
            self.configs.remove(&root);
        }
    }

    pub(crate) fn forget_pane(&mut self, pane_id: u64) {
        self.clear_pane_root(pane_id);
        self.detections.remove(&pane_id);
        if self
            .offer
            .as_ref()
            .is_some_and(|offer| offer.pane_id == pane_id)
        {
            self.offer = None;
        }
        self.active_context = None;
    }

    pub(crate) fn forget_tab(&mut self, tab_id: u64, pane_ids: impl IntoIterator<Item = u64>) {
        for pane_id in pane_ids {
            self.forget_pane(pane_id);
        }
        self.entered
            .retain(|(entered_tab_id, _)| *entered_tab_id != tab_id);
        self.inherited_tab_icons.remove(&tab_id);
    }
}

fn project_key(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        value.to_lowercase()
    } else {
        value
    }
}

#[derive(Debug)]
struct ProjectDetectionResult {
    registered_root: Option<PathBuf>,
    config_root: Option<PathBuf>,
    config: Option<Result<ProjectConfig>>,
    offer_root: Option<PathBuf>,
}

fn detect_project_for_directory(
    directory: &Path,
    registry: &ProjectRegistry,
    base: &Config,
    loaded_roots: &[PathBuf],
) -> ProjectDetectionResult {
    let canonical = fs::canonicalize(directory).ok();
    let directory = canonical.as_deref().unwrap_or(directory);
    let resolution = resolve_registered_project(directory, registry);
    if let Some(root) = resolution.root {
        let config_root = resolution.config_root.unwrap_or_else(|| root.clone());
        let config = (!loaded_roots
            .iter()
            .any(|loaded| paths_equal(loaded, &config_root)))
        .then(|| ProjectConfig::load(&config_root, base));
        return ProjectDetectionResult {
            registered_root: Some(root),
            config_root: Some(config_root),
            config,
            offer_root: None,
        };
    }
    let offer_root = discover_project_config(directory).ok().flatten();
    ProjectDetectionResult {
        registered_root: None,
        config_root: None,
        config: None,
        offer_root,
    }
}

fn wsl_distribution(profile: &Profile) -> Option<&str> {
    if !is_wsl_shell(&profile.command) {
        return None;
    }
    if let Shell::WithArguments { args, .. } = &profile.command {
        for pair in args.windows(2) {
            if matches!(pair[0].as_str(), "--distribution" | "-d") {
                return Some(pair[1].as_str());
            }
        }
    }
    profile.name.strip_prefix("WSL: ")
}

pub(crate) fn wsl_reported_directory(profile: &Profile, directory: &str) -> Option<PathBuf> {
    let distribution = wsl_distribution(profile)?;
    if distribution.is_empty()
        || distribution
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
        || !directory.starts_with('/')
        || directory.chars().any(char::is_control)
        || directory
            .split('/')
            .any(|component| matches!(component, "." | ".."))
    {
        return None;
    }
    let relative = directory.trim_start_matches('/').replace('/', "\\");
    Some(PathBuf::from(format!(
        r"\\wsl.localhost\{distribution}\{relative}"
    )))
}

impl Zetta {
    pub(crate) fn active_project_config(&self) -> Option<&Arc<ProjectConfig>> {
        let pane_id = self.tabs.get(self.active_tab)?.active_pane;
        self.projects.config_for_pane(pane_id)
    }

    pub(crate) fn project_config_for_tab(&self, tab_id: u64) -> Option<&Arc<ProjectConfig>> {
        let pane_id = self.tabs.iter().find(|tab| tab.id == tab_id)?.active_pane;
        self.projects.config_for_pane(pane_id)
    }

    pub(crate) fn effective_config(&self) -> &Config {
        self.active_project_config()
            .map(|project| &project.effective)
            .unwrap_or(&self.launch_config)
    }

    /// Re-binds the built-in `ctrl-shift-{number}` profile shortcuts, and on
    /// macOS rebuilds the native Profile menu, for the profiles that are
    /// currently effective.
    ///
    /// One shortcut is bound per visible profile, so a project that adds, hides,
    /// or unhides a profile changes how many slots exist: without this, a
    /// project profile appears in the menus with no accelerator and its chord
    /// does nothing. Rebinding rebuilds the whole keymap, including re-reading
    /// the user's keymap file, so it is gated on the slot count actually
    /// changing rather than run on every project activation.
    pub(crate) fn refresh_profile_shortcuts(&mut self, cx: &mut App) {
        let slots = {
            let effective = self.effective_config();
            visible_profile_count(&self.profiles, &effective.hidden_profiles)
        };
        #[cfg(target_os = "macos")]
        {
            let (hidden_profiles, default_profile) = {
                let effective = self.effective_config();
                (effective.hidden_profiles.clone(), effective.default_profile)
            };
            crate::startup::update_native_macos_menus(
                cx,
                &self.profiles,
                &hidden_profiles,
                default_profile,
            );
        }
        if slots == self.profile_shortcut_slots {
            return;
        }
        self.profile_shortcut_slots = slots;
        load_keybindings(&self.launch_config.keymap_path, slots, self.no_mux, cx);
    }

    /// Applies the window's current system appearance and refreshes every
    /// terminal view that follows configuration. This is deliberately driven
    /// by GPUI's appearance notification rather than by render or timer work.
    pub(crate) fn handle_window_appearance_change(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        *SystemAppearance::global_mut(cx) = SystemAppearance(window.appearance().into());
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
        tab.theme_override
            .as_deref()
            .and_then(|name| ThemeRegistry::global(cx).get(name).ok())
            .or_else(|| {
                self.projects
                    .config_for_pane(tab.active_pane)
                    .and_then(|project| project_theme_name(project, cx))
                    .and_then(|name| ThemeRegistry::global(cx).get(name).ok())
            })
            .unwrap_or_else(|| tab.theme(cx, || self.application_theme(cx)))
    }

    pub(crate) fn project_environment_for_tab(&self, tab_id: u64) -> HashMap<String, String> {
        self.project_config_for_tab(tab_id)
            .map(|project| project.environment.clone())
            .unwrap_or_default()
    }

    pub(crate) fn schedule_project_detection_for_pane(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .or_else(|| self.background_sessions.iter().find(|tab| tab.id == tab_id))
        else {
            return;
        };
        let Some(pane) = tab.pane(pane_id) else {
            return;
        };
        let is_wsl = is_wsl_shell(&pane.profile.command);
        let directory = if is_wsl {
            pane.wsl_working_directory(cx)
                .and_then(|directory| wsl_reported_directory(&pane.profile, &directory))
        } else {
            pane.current_directory(cx)
                .filter(|(_, authoritative)| *authoritative)
                .map(|(directory, _)| directory)
        };
        let Some(directory) = directory else {
            return;
        };
        let Some(generation) = self.projects.begin_detection(pane_id, directory.clone()) else {
            return;
        };
        let resolved_root =
            resolve_registered_project_config_root(&directory, &self.projects.registry);
        let left_project = self.projects.root_for_pane(pane_id).is_some_and(|root| {
            resolved_root
                .as_ref()
                .is_none_or(|resolved| !paths_equal(resolved, root))
        });
        if left_project {
            self.projects.clear_pane_root(pane_id);
            self.projects.invalidate_active_context();
            if self
                .projects
                .offer
                .as_ref()
                .is_some_and(|offer| offer.pane_id == pane_id)
            {
                self.projects.offer = None;
            }
            let is_active = self
                .tabs
                .get(self.active_tab)
                .is_some_and(|tab| tab.id == tab_id && tab.active_pane == pane_id);
            if is_active {
                self.activate_current_project(window, cx);
            }
        }
        let registry = self.projects.registry.clone();
        let loaded_roots = self.projects.configs.keys().cloned().collect::<Vec<_>>();
        let base = self.project_detection_base.clone();
        let executor = cx.background_executor().clone();
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let detection_directory = directory.clone();
                let result = executor
                    .spawn(async move {
                        detect_project_for_directory(
                            &detection_directory,
                            &registry,
                            &base,
                            &loaded_roots,
                        )
                    })
                    .await;
                this.update_in(cx, |this, window, cx| {
                    this.apply_project_detection(
                        tab_id, pane_id, directory, generation, result, window, cx,
                    );
                })
                .ok();
            })
            .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_project_detection(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        directory: PathBuf,
        generation: u64,
        result: ProjectDetectionResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.projects.detections.get(&pane_id) else {
            return;
        };
        if state.generation != generation || !paths_equal(&state.directory, &directory) {
            return;
        }

        match result.registered_root {
            Some(registered_root) => {
                let config_root = result
                    .config_root
                    .unwrap_or_else(|| registered_root.clone());
                self.projects
                    .pane_roots
                    .insert(pane_id, config_root.clone());
                if let Some(config) = result.config {
                    match config {
                        Ok(config) => {
                            self.projects.insert_config(config);
                            if self
                                .projects
                                .offer
                                .as_ref()
                                .is_some_and(|offer| paths_equal(&offer.root, &registered_root))
                            {
                                self.projects.offer = None;
                            }
                        }
                        Err(error) => {
                            self.projects.configs.remove(&config_root);
                            self.configuration_error = Some(format!(
                                "Could not load project configuration {}: {error:#}",
                                ProjectConfig::path_for(&config_root).display()
                            ));
                        }
                    }
                }
            }
            None => {
                self.projects.clear_pane_root(pane_id);
            }
        }
        if self.projects.offer.as_ref().is_some_and(|offer| {
            offer.pane_id == pane_id
                && result
                    .offer_root
                    .as_ref()
                    .is_none_or(|root| !paths_equal(root, &offer.root))
        }) {
            self.projects.offer = None;
        }
        if let Some(root) = result.offer_root
            && resolve_registered_project_root(&root, &self.projects.registry).is_none()
            && !self.projects.offer_is_dismissed(&root)
        {
            self.projects.offer = Some(ProjectOffer { root, pane_id });
        }

        let is_active = self
            .tabs
            .get(self.active_tab)
            .is_some_and(|tab| tab.id == tab_id && tab.active_pane == pane_id);
        if is_active {
            self.activate_current_project(window, cx);
        } else {
            cx.notify();
        }
    }

    pub(crate) fn activate_current_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((tab_id, pane_id)) = self
            .tabs
            .get(self.active_tab)
            .map(|tab| (tab.id, tab.active_pane))
        else {
            return;
        };
        let project = self.projects.config_for_pane(pane_id).cloned();
        let context_key = (
            tab_id,
            project
                .as_ref()
                .map(|project| Arc::as_ptr(project) as usize),
        );
        if self.projects.active_context.as_ref() == Some(&context_key) {
            return;
        }
        self.projects.active_context = Some(context_key);
        let config = project
            .as_ref()
            .map(|project| &project.effective)
            .unwrap_or(&self.launch_config);
        self.profiles = config.profiles.clone();
        self.working_directory = config.working_directory.clone();
        // The project may add, hide, or unhide profiles, which changes which
        // `ctrl-shift-{number}` slots exist.
        self.refresh_profile_shortcuts(cx);

        if let Some(project) = &project {
            for theme in [project.theme.as_deref(), project.dark_theme.as_deref()]
                .into_iter()
                .flatten()
            {
                if ThemeRegistry::global(cx).get(theme).is_err() {
                    self.configuration_error = Some(format!(
                        "Could not apply project theme {theme:?} for {}",
                        project.root.display()
                    ));
                    break;
                }
            }
        }

        self.refresh_active_project_tab_icon(project.as_deref());

        self.apply_effective_themes_to_tab(tab_id, cx);
        self.refresh_open_command_palette(window, cx);

        if let Some(project) = project
            && self.projects.mark_entered(tab_id, &project.root)
            && let Some(name) = project.initial_split.clone()
        {
            self.apply_pane_split_template_with_profile(&name, None, window, cx);
        }
        cx.notify();
    }

    /// Reapplies the active project's effective icon without changing project
    /// context bookkeeping. Configuration reloads use this path because they
    /// do not have a window with which to call `activate_current_project`.
    pub(crate) fn refresh_active_project_tab_icon(&mut self, project: Option<&ProjectConfig>) {
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

    pub(crate) fn apply_effective_themes_to_tab(&mut self, tab_id: u64, cx: &mut Context<Self>) {
        let Some(tab_index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        let projects = self.tabs[tab_index]
            .panes
            .iter()
            .flat_map(|pane| {
                std::iter::once(pane.id).chain(pane.stack.entries.iter().map(|entry| entry.id))
            })
            .filter_map(|pane_id| {
                self.projects
                    .config_for_pane(pane_id)
                    .cloned()
                    .map(|project| (pane_id, project))
            })
            .collect::<HashMap<_, _>>();
        let tab = &mut self.tabs[tab_index];
        for pane in &mut tab.panes {
            let project = projects.get(&pane.id);
            let configured_profiles = project
                .map(|project| &project.effective.profiles)
                .unwrap_or(&self.launch_config.profiles);
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
            let theme = resolve_terminal_theme(
                pane.theme_override.as_deref(),
                tab.theme_override.as_deref(),
                &pane.profile,
                project.map(Arc::as_ref),
                cx,
            )
            .ok()
            .flatten();
            if let Some(view) = pane.view.clone() {
                view.update(cx, |view, cx| view.set_theme(theme.clone(), cx));
            }
            for entry in &mut pane.stack.entries {
                let project = projects.get(&entry.id);
                let configured_profiles = project
                    .map(|project| &project.effective.profiles)
                    .unwrap_or(&self.launch_config.profiles);
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
                let theme = resolve_terminal_theme(
                    entry.theme_override.as_deref(),
                    tab.theme_override.as_deref(),
                    &entry.profile,
                    project.map(Arc::as_ref),
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
        let Some(tab_index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        let projects = terminal_projects_for_tab(&self.tabs[tab_index], &self.projects);
        refresh_terminal_themes_in_tab(&mut self.tabs[tab_index], &projects, cx);
    }

    pub(crate) fn refresh_terminal_themes_for_appearance(&mut self, cx: &mut Context<Self>) {
        // The pane profiles already contain the effective configuration used
        // when each pane was opened. Keep those values intact here: pane
        // template leaf overrides live on the pane profile, while the
        // appearance change only selects which of its two fields is active.
        for index in 0..self.tabs.len() {
            let projects = terminal_projects_for_tab(&self.tabs[index], &self.projects);
            refresh_terminal_themes_in_tab(&mut self.tabs[index], &projects, cx);
        }

        let background_projects = self
            .background_sessions
            .iter()
            .map(|tab| terminal_projects_for_tab(tab, &self.projects))
            .collect::<Vec<_>>();
        for (tab, projects) in self.background_sessions.iter_mut().zip(background_projects) {
            refresh_terminal_themes_in_tab(tab, &projects, cx);
        }
    }

    /// The command palette's per-project entry, and the same outcome as the
    /// Projects page's Open button. The palette names a canonical root, so the
    /// registry lookup normally settles without touching the filesystem; a
    /// dispatch from elsewhere may name the root the way the user typed it,
    /// which needs canonicalizing before `open_project_tab` can recognize it.
    pub(crate) fn open_project(
        &mut self,
        action: &OpenProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = if self.projects.registry.contains(&action.root) {
            action.root.clone()
        } else {
            canonical_project_root(&action.root).unwrap_or_else(|_| action.root.clone())
        };
        self.open_project_tab(root, window, cx);
    }

    pub(crate) fn open_project_tab(
        &mut self,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_project_tab_with_working_directory(root, None, window, cx);
    }

    pub(crate) fn open_project_tab_with_working_directory(
        &mut self,
        root: PathBuf,
        working_directory: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.projects.registry.contains(&root) {
            self.configuration_error = Some(format!(
                "{} is not a registered Zetta project",
                root.display()
            ));
            cx.notify();
            return;
        }
        let config_root = working_directory
            .as_deref()
            .and_then(|directory| {
                resolve_registered_project_config_root(directory, &self.projects.registry)
            })
            .unwrap_or_else(|| root.clone());
        let base = self.launch_config.clone();
        let executor = cx.background_executor().clone();
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let project_root = config_root.clone();
                let result = executor
                    .spawn(async move { ProjectConfig::load(&project_root, &base) })
                    .await;
                this.update_in(cx, |this, window, cx| match result {
                    Ok(project) => match working_directory {
                        Some(working_directory) => this
                            .open_loaded_project_tab_with_working_directory(
                                project,
                                Some(working_directory),
                                window,
                                cx,
                            ),
                        None => this.open_loaded_project_tab(project, window, cx),
                    },
                    Err(error) => {
                        this.configuration_error = Some(format!(
                            "Could not open project {}: {error:#}",
                            config_root.display()
                        ));
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
    }

    pub(crate) fn open_loaded_project_tab(
        &mut self,
        project: ProjectConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_loaded_project_tab_with_working_directory(project, None, window, cx);
    }

    pub(crate) fn open_loaded_project_tab_with_working_directory(
        &mut self,
        project: ProjectConfig,
        working_directory: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = self.projects.insert_config(project);
        let Some(profile) = project
            .effective
            .profiles
            .get(project.effective.default_profile)
            .cloned()
        else {
            self.configuration_error = Some(format!(
                "Project {} has no available default profile",
                project.root.display()
            ));
            cx.notify();
            return;
        };
        match working_directory {
            None => self.open_tab_with_profile_in_project(profile, project, window, cx),
            Some(working_directory) => self.open_tab_with_profile_context(
                profile,
                Some(project),
                NewTabOrigin::ProjectEntry,
                None,
                Some(working_directory),
                TerminalLaunch::Spawn,
                window,
                cx,
            ),
        }
    }

    pub(crate) fn reload_projects(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let registry = ProjectRegistry::load()?;
        let removed_roots = self
            .projects
            .registry
            .roots()
            .iter()
            .filter(|root| !registry.contains(root))
            .cloned()
            .collect::<Vec<_>>();
        self.projects.registry = registry;
        for root in removed_roots {
            self.projects.suppress_offer_for(&root);
        }
        self.projects.active_context = None;
        self.projects.clear_removed_roots();
        let roots = self
            .projects
            .pane_roots
            .values()
            .filter(|root| is_registered_project_config_root(root, &self.projects.registry))
            .cloned()
            .collect::<HashSet<_>>();
        for root in roots {
            match ProjectConfig::load(&root, &self.launch_config) {
                Ok(config) => {
                    self.projects.insert_config(config);
                }
                Err(error) => {
                    self.projects.configs.remove(&root);
                    self.configuration_error = Some(format!(
                        "Could not reload project configuration {}: {error:#}",
                        ProjectConfig::path_for(&root).display()
                    ));
                }
            }
        }
        self.reschedule_project_detection(window, cx);
        self.activate_current_project(window, cx);
        Ok(())
    }

    pub(crate) fn reschedule_project_detection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.projects.invalidate_detections();
        let panes = self
            .tabs
            .iter()
            .flat_map(|tab| tab.panes.iter().map(move |pane| (tab.id, pane.id)))
            .collect::<Vec<_>>();
        for (tab_id, pane_id) in panes {
            self.schedule_project_detection_for_pane(tab_id, pane_id, window, cx);
        }
    }

    pub(crate) fn reload_project_registry_without_window(&mut self) -> Result<()> {
        let registry = ProjectRegistry::load()?;
        let roots = self
            .projects
            .pane_roots
            .values()
            .filter(|root| is_registered_project_config_root(root, &registry))
            .cloned()
            .collect::<HashSet<_>>();
        let configs = roots
            .iter()
            .map(|root| ProjectConfig::load(root, &self.launch_config))
            .collect::<Result<Vec<_>>>()?;
        self.projects.registry = registry;
        self.projects.active_context = None;
        self.projects.invalidate_detections();
        self.projects.clear_removed_roots();
        for config in configs {
            self.projects.insert_config(config);
        }
        Ok(())
    }

    pub(crate) fn dismiss_project_offer(&mut self, cx: &mut Context<Self>) {
        self.projects.dismiss_offer();
        cx.notify();
    }

    pub(crate) fn accept_project_offer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(offer) = self.projects.offer.clone() else {
            return;
        };
        let base = self.launch_config.clone();
        let registry_path = self.projects.registry.path().to_path_buf();
        let executor = cx.background_executor().clone();
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let root = offer.root.clone();
                let result = executor
                    .spawn(async move {
                        let mut registry = ProjectRegistry::load_from(registry_path)?;
                        let root = canonical_project_root(&root)?;
                        let root =
                            resolve_registered_project_root(&root, &registry).unwrap_or(root);
                        let config = ProjectConfig::load(&root, &base)?;
                        if registry.add(&root)? {
                            registry.save()?;
                        }
                        Ok::<_, anyhow::Error>((registry, config))
                    })
                    .await;
                this.update_in(cx, |this, window, cx| match result {
                    Ok((registry, config)) => {
                        this.projects.registry = registry;
                        let root = config.root.clone();
                        this.projects.insert_config(config);
                        let still_inside = this
                            .projects
                            .detections
                            .get(&offer.pane_id)
                            .is_some_and(|state| {
                                resolve_registered_project_root(
                                    &state.directory,
                                    &this.projects.registry,
                                )
                                .is_some_and(|resolved| paths_equal(&resolved, &root))
                            });
                        if still_inside {
                            this.projects.pane_roots.insert(offer.pane_id, root);
                        }
                        this.projects.offer = None;
                        this.activate_current_project(window, cx);
                        reload_projects_in_other_windows(window.window_handle().window_id(), cx);
                    }
                    Err(error) => {
                        this.configuration_error = Some(format!(
                            "Could not add project {}: {error:#}",
                            offer.root.display()
                        ));
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
    }
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

/// Resolves terminal content styling in one place. Explicit pane state wins
/// over project and profile configuration; an absent result means the
/// application theme selected by the caller/view remains in effect.
pub(crate) fn resolve_terminal_theme(
    pane_theme_override: Option<&str>,
    tab_theme_override: Option<&str>,
    profile: &Profile,
    project: Option<&ProjectConfig>,
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
    resolve_project_profile_theme(profile, project, cx)
}

fn project_theme_name<'a>(project: &'a ProjectConfig, cx: &App) -> Option<&'a str> {
    if SystemAppearance::global(cx).is_light() {
        project.theme.as_deref()
    } else {
        project.dark_theme.as_deref()
    }
}

fn refresh_terminal_themes_in_tab(
    tab: &mut Tab,
    projects: &HashMap<u64, Arc<ProjectConfig>>,
    cx: &mut Context<Zetta>,
) {
    for pane in &mut tab.panes {
        refresh_terminal_theme_for_profile(
            pane.theme_override.as_deref(),
            tab.theme_override.as_deref(),
            &mut pane.profile,
            pane.view.clone(),
            projects.get(&pane.id).map(Arc::as_ref),
            cx,
        );
        for entry in &mut pane.stack.entries {
            refresh_terminal_theme_for_profile(
                entry.theme_override.as_deref(),
                tab.theme_override.as_deref(),
                &mut entry.profile,
                entry.view.clone(),
                projects.get(&entry.id).map(Arc::as_ref),
                cx,
            );
        }
    }
}

fn terminal_projects_for_tab(
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

fn refresh_terminal_theme_for_profile(
    pane_theme_override: Option<&str>,
    tab_theme_override: Option<&str>,
    profile: &mut Profile,
    view: Option<Entity<TerminalView>>,
    project: Option<&ProjectConfig>,
    cx: &mut Context<Zetta>,
) {
    let theme = resolve_terminal_theme(
        pane_theme_override,
        tab_theme_override,
        profile,
        project,
        cx,
    )
    .ok()
    .flatten();
    if let Some(view) = view {
        view.update(cx, |view, cx| view.set_theme(theme, cx));
    }
}

#[cfg(test)]
#[path = "tests/project_context.rs"]
mod tests;
