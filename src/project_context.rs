use super::*;
use crate::project::{
    ProjectConfig, ProjectRegistry, discover_project_config, path_is_within, paths_equal,
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
        self.root_for_pane(pane_id)
            .and_then(|root| self.configs.get(root))
    }

    pub(crate) fn insert_config(&mut self, config: ProjectConfig) -> Arc<ProjectConfig> {
        let config = Arc::new(config);
        self.configs.insert(config.root.clone(), config.clone());
        config
    }

    fn mark_entered(&mut self, tab_id: u64, root: &Path) -> bool {
        self.entered.insert((tab_id, project_key(root)))
    }

    pub(crate) fn dismiss_offer(&mut self) {
        if let Some(offer) = self.offer.take() {
            self.dismissed_offers.insert(project_key(&offer.root));
        }
    }

    pub(crate) fn suppress_offer_for(&mut self, root: &Path) {
        self.dismissed_offers.insert(project_key(root));
        if self
            .offer
            .as_ref()
            .is_some_and(|offer| paths_equal(&offer.root, root))
        {
            self.offer = None;
        }
    }

    fn offer_is_dismissed(&self, root: &Path) -> bool {
        self.dismissed_offers.contains(&project_key(root))
    }

    pub(crate) fn clear_removed_roots(&mut self) {
        self.pane_roots
            .retain(|_, root| self.registry.contains(root));
        self.configs.retain(|root, _| self.registry.contains(root));
        if self
            .offer
            .as_ref()
            .is_some_and(|offer| self.registry.contains(&offer.root))
        {
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
    config: Option<Result<ProjectConfig>>,
    offer_root: Option<PathBuf>,
}

fn detect_project_for_directory(
    directory: &Path,
    registry: &ProjectRegistry,
    base: &Config,
    loaded_roots: &[PathBuf],
    allow_discovery: bool,
) -> ProjectDetectionResult {
    let directory = if allow_discovery {
        match fs::canonicalize(directory) {
            Ok(directory) => directory,
            Err(_) => {
                return ProjectDetectionResult {
                    registered_root: None,
                    config: None,
                    offer_root: None,
                };
            }
        }
    } else {
        directory.to_path_buf()
    };
    if let Some(root) = registry.matching_root(&directory).cloned() {
        let config = (!loaded_roots.iter().any(|loaded| paths_equal(loaded, &root)))
            .then(|| ProjectConfig::load(&root, base));
        return ProjectDetectionResult {
            registered_root: Some(root),
            config,
            offer_root: None,
        };
    }
    let offer_root = allow_discovery
        .then(|| discover_project_config(&directory).ok().flatten())
        .flatten();
    ProjectDetectionResult {
        registered_root: None,
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

fn wsl_reported_directory(profile: &Profile, directory: &str) -> Option<PathBuf> {
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
        load_keybindings(&self.launch_config.keymap_path, slots, cx);
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
            .get(selected_theme_name(self.launch_config.theme.as_deref()))
            .unwrap_or_else(|_| cx.theme().clone())
    }

    pub(crate) fn window_theme(&self, cx: &App) -> Arc<Theme> {
        self.active_project_config()
            .and_then(|project| project.effective.theme.as_deref())
            .and_then(|name| ThemeRegistry::global(cx).get(name).ok())
            .unwrap_or_else(|| self.application_theme(cx))
    }

    pub(crate) fn theme_for_tab(&self, tab: &Tab, cx: &App) -> Arc<Theme> {
        self.projects
            .config_for_pane(tab.active_pane)
            .and_then(|project| project.effective.theme.as_deref())
            .and_then(|name| ThemeRegistry::global(cx).get(name).ok())
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
            pane.working_directory(cx)
        };
        let Some(directory) = directory else {
            return;
        };
        let Some(generation) = self.projects.begin_detection(pane_id, directory.clone()) else {
            return;
        };
        let left_project = self
            .projects
            .root_for_pane(pane_id)
            .is_some_and(|root| !path_is_within(&directory, root));
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
                            !is_wsl,
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
            Some(root) => {
                self.projects.pane_roots.insert(pane_id, root.clone());
                if let Some(config) = result.config {
                    match config {
                        Ok(config) => {
                            self.projects.insert_config(config);
                            if self
                                .projects
                                .offer
                                .as_ref()
                                .is_some_and(|offer| paths_equal(&offer.root, &root))
                            {
                                self.projects.offer = None;
                            }
                        }
                        Err(error) => {
                            self.projects.configs.remove(&root);
                            self.configuration_error = Some(format!(
                                "Could not load project configuration {}: {error:#}",
                                ProjectConfig::path_for(&root).display()
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
            && !self.projects.registry.contains(&root)
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

        if let Some(project) = &project
            && let Some(theme) = project.effective.theme.as_deref()
            && ThemeRegistry::global(cx).get(theme).is_err()
        {
            self.configuration_error = Some(format!(
                "Could not apply project theme {theme:?} for {}",
                project.root.display()
            ));
        }

        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            match &project {
                Some(project) => {
                    self.projects
                        .inherited_tab_icons
                        .entry(tab.id)
                        .or_insert(tab.icon);
                    tab.icon = project.effective.default_tab_icon;
                }
                None => {
                    if let Some(icon) = self.projects.inherited_tab_icons.remove(&tab.id) {
                        tab.icon = icon;
                    }
                }
            }
        }

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

    pub(crate) fn apply_effective_themes_to_tab(&mut self, tab_id: u64, cx: &mut Context<Self>) {
        let project = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| self.projects.config_for_pane(tab.active_pane))
            .cloned();
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return;
        };
        for pane in &mut tab.panes {
            let configured_profiles = project
                .as_ref()
                .map(|project| &project.effective.profiles)
                .unwrap_or(&self.launch_config.profiles);
            let configured_profile = configured_profiles
                .iter()
                .find(|profile| profile.name.eq_ignore_ascii_case(&pane.profile.name));
            if let Some(profile) = configured_profile {
                pane.profile.theme = profile.theme.clone();
            } else {
                pane.profile.theme = None;
            }
            let theme = resolve_project_profile_theme(&pane.profile, project.as_deref(), cx)
                .ok()
                .flatten();
            if let Some(view) = pane.view.clone() {
                view.update(cx, |view, cx| view.set_theme(theme.clone(), cx));
            }
            for entry in &mut pane.stack.entries {
                let configured_profile = configured_profiles
                    .iter()
                    .find(|profile| profile.name.eq_ignore_ascii_case(&entry.profile.name));
                if let Some(profile) = configured_profile {
                    entry.profile.theme = profile.theme.clone();
                } else {
                    entry.profile.theme = None;
                }
                let theme = resolve_project_profile_theme(&entry.profile, project.as_deref(), cx)
                    .ok()
                    .flatten();
                if let Some(view) = entry.view.clone() {
                    view.update(cx, |view, cx| view.set_theme(theme, cx));
                }
            }
        }
    }

    pub(crate) fn open_project_tab(
        &mut self,
        root: PathBuf,
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
        let base = self.launch_config.clone();
        let executor = cx.background_executor().clone();
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let project_root = root.clone();
                let result = executor
                    .spawn(async move { ProjectConfig::load(&project_root, &base) })
                    .await;
                this.update_in(cx, |this, window, cx| match result {
                    Ok(project) => this.open_loaded_project_tab(project, window, cx),
                    Err(error) => {
                        this.configuration_error = Some(format!(
                            "Could not open project {}: {error:#}",
                            root.display()
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
        self.open_tab_with_profile_in_project(profile, project, window, cx);
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
            .filter(|root| self.projects.registry.contains(root))
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
            .filter(|root| registry.contains(root))
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
                        let config = ProjectConfig::load(&root, &base)?;
                        let mut registry = ProjectRegistry::load_from(registry_path)?;
                        registry.add(&root)?;
                        registry.save()?;
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
                                crate::project::path_is_within(&state.directory, &root)
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

pub(crate) fn resolve_project_profile_theme(
    profile: &Profile,
    project: Option<&ProjectConfig>,
    cx: &App,
) -> Result<Option<Arc<Theme>>> {
    if profile.theme.is_some() {
        return resolve_profile_theme(profile, cx);
    }
    project
        .and_then(|project| project.effective.theme.as_deref())
        .map(|name| {
            ThemeRegistry::global(cx)
                .get(name)
                .with_context(|| format!("using project theme {name:?}"))
        })
        .transpose()
}

#[cfg(test)]
#[path = "tests/project_context.rs"]
mod tests;
