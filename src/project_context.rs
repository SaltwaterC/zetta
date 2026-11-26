//! The project a tab is in: detecting it from the active pane's directory, the
//! configuration and environment it layers over the application's, and the
//! profile shortcuts it changes.

use super::*;
use crate::project::{
    ProjectConfig, ProjectRegistry, ProjectRootResolution, canonical_project_root,
    discover_project_config_canonical, is_registered_project_config_root, path_is_within_lexically,
    paths_equal, resolve_registered_project_canonical, resolve_registered_project_config_root,
    resolve_registered_project_root,
};

mod state;
pub(crate) use state::*;
mod themes;
pub(crate) use themes::*;

/// One detection in flight: the pane it is for, the directory it was started
/// from, and the generation `begin_detection` gave it. A result is applied only
/// while the pane's current detection still has the same directory and
/// generation, so a newer `cd` supersedes it.
struct ProjectDetection {
    tab_id: u64,
    pane_id: u64,
    directory: PathBuf,
    generation: u64,
}

impl Zetta {
    pub(crate) fn active_project_config(&self) -> Option<&Arc<ProjectConfig>> {
        let tab = self.current_tab()?;
        self.project_config_for_tab(tab.id)
    }

    pub(crate) fn project_config_for_tab(&self, tab_id: u64) -> Option<&Arc<ProjectConfig>> {
        if self.project_context_policy(tab_id).is_remote() {
            return None;
        }
        let pane_id = self.tabs.by_id(tab_id)?.active_pane;
        self.projects.config_for_pane(pane_id)
    }

    pub(crate) fn project_context_policy(&self, tab_id: u64) -> ProjectContextPolicy {
        if self.mux_panes.is_remote_tab(tab_id) {
            ProjectContextPolicy::Remote
        } else {
            ProjectContextPolicy::Local
        }
    }

    pub(crate) fn effective_config(&self) -> &Config {
        self.active_project_config()
            .map_or(&self.launch_config, |project| &project.effective)
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
        self.refresh_profile_shortcuts_from(None, cx);
    }

    /// [`Self::refresh_profile_shortcuts`], binding from a keymap already read
    /// when there is one, so a configuration reload does not read the file on
    /// the GUI thread.
    pub(crate) fn refresh_profile_shortcuts_from(
        &mut self,
        keymap: Option<&KeymapSource>,
        cx: &mut App,
    ) {
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
        match keymap {
            Some(keymap) => bind_keybindings(
                &self.launch_config.keymap_path,
                keymap,
                slots,
                self.no_mux,
                cx,
            ),
            None => load_keybindings(&self.launch_config.keymap_path, slots, self.no_mux, cx),
        }
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
        self.schedule_project_detection(tab_id, pane_id, true, window, cx);
    }

    /// [`Self::schedule_project_detection_for_pane`], refreshing a WSL pane's
    /// tracking-file observation first when `observe_wsl_cwd_file` is set.
    /// The refresh re-enters with it clear, so one trigger reads the file
    /// once.
    fn schedule_project_detection(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        observe_wsl_cwd_file: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project_context_policy(tab_id).is_remote() {
            let has_local_context = self.projects.root_for_pane(pane_id).is_some()
                || self.projects.detections.contains_key(&pane_id)
                || self
                    .projects
                    .offer
                    .as_ref()
                    .is_some_and(|offer| offer.pane_id == pane_id);
            if has_local_context {
                self.projects.forget_pane(pane_id);
            }
            // A remote directory never places a tab, so it keeps the group it
            // arrived in.
            self.tab_groups.arrived(tab_id);
            return;
        }
        let Some(pane) = self.project_detection_pane(tab_id, pane_id) else {
            return;
        };
        // Remote directory reports may be absent or match a local project
        // path. Suspend that project based on the client instead. A change
        // of foreground process emits `TitleChanged`, which re-runs this on
        // both sides of the session, even without remote shell integration.
        let remote_foreground = pane.terminal.as_ref().is_some_and(|terminal| {
            terminal
                .read(cx)
                .foreground_process_command_line()
                .is_some_and(|argv| crate::ssh_image_paste::foreground_is_remote_session(&argv))
        });
        if remote_foreground {
            // Placed when the client exits, the tab would move under a user
            // who has long since started working in it.
            if self
                .tabs
                .iter()
                .any(|tab| tab.id == tab_id && tab.active_pane == pane_id)
            {
                self.tab_groups.arrived(tab_id);
            }
            if self.projects.leave_for_remote_foreground(pane_id) {
                if self.is_active_pane(tab_id, pane_id) {
                    self.activate_current_project(window, cx);
                } else {
                    self.apply_effective_themes_to_tab(tab_id, cx);
                    cx.notify();
                }
            }
            return;
        }
        let wsl_cwd_file = observe_wsl_cwd_file
            .then(|| pane.wsl_cwd_file_needs_observing(cx))
            .flatten();
        let directory = project_detection_directory(pane, cx);
        if let Some(file) = wsl_cwd_file {
            self.observe_wsl_cwd_file(tab_id, pane_id, file, window, cx);
        }
        let directory = match directory {
            ProjectDetectionDirectory::Verified(directory) => directory,
            ProjectDetectionDirectory::Unverified(directory) => {
                // The pane appears to be somewhere its shell has not confirmed:
                // a host it is logged in to, or a forged report. That may take
                // it out of its project, but never into one. The entry
                // is dropped so the shell's next confirmed report is detected
                // again even when it names the directory detected last.
                self.projects.detections.remove(&pane_id);
                self.leave_project_ahead_of_detection(tab_id, pane_id, &directory, window, cx);
                return;
            }
            ProjectDetectionDirectory::Unknown => return,
        };
        let Some(generation) = self.projects.begin_detection(pane_id, directory.clone()) else {
            return;
        };
        self.leave_project_ahead_of_detection(tab_id, pane_id, &directory, window, cx);
        let detection = ProjectDetection {
            tab_id,
            pane_id,
            directory,
            generation,
        };
        self.spawn_project_detection(detection, window, cx);
    }

    fn project_detection_pane(&self, tab_id: u64, pane_id: u64) -> Option<&TerminalPane> {
        self.any_tab(tab_id)?.pane(pane_id)
    }

    /// Reads a WSL pane's legacy tracking file on the background executor and,
    /// if what it says changed, detects the project again from the new answer.
    /// The shell's reported directory stays authoritative: this only runs for
    /// a pane that has not reported one.
    fn observe_wsl_cwd_file(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        file: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self
            .any_tab_mut(tab_id)
            .and_then(|tab| tab.pane_mut(pane_id))
        else {
            return;
        };
        let generation = pane.begin_wsl_cwd_observation();
        let executor = cx.background_executor().clone();
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let observation = executor
                    .spawn(async move { WslCwdObservation::read(file) })
                    .await;
                this.update_in(cx, |this, window, cx| {
                    let changed = this
                        .any_tab_mut(tab_id)
                        .and_then(|tab| tab.pane_mut(pane_id))
                        .is_some_and(|pane| pane.observe_wsl_cwd_file(generation, observation));
                    if changed {
                        this.schedule_project_detection(tab_id, pane_id, false, window, cx);
                    }
                })
                .ok();
            })
            .detach();
    }

    /// Takes the pane out of its project now, rather than when the detection
    /// returns, when the new directory is not lexically inside it; see
    /// [`detection_keeps_pane_root`]. Nothing here touches the filesystem: the
    /// worker's result is what settles the project, and resolving it here as
    /// well put canonicalization and a Git ancestor walk on the GUI thread for
    /// every `cd`.
    fn leave_project_ahead_of_detection(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        directory: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let left_project = self.projects.root_for_pane(pane_id).is_some_and(|root| {
            !detection_keeps_pane_root(
                directory,
                root,
                self.projects.pane_anchors.get(&pane_id),
                &self.projects.registry,
            )
        });
        if !left_project {
            return;
        }
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
        if self.is_active_pane(tab_id, pane_id) {
            self.activate_current_project(window, cx);
        }
    }

    fn spawn_project_detection(
        &mut self,
        detection: ProjectDetection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let registry = self.projects.registry.clone();
        let loaded_roots = self.projects.configs.keys().cloned().collect::<Vec<_>>();
        let base = self.project_detection_base.clone();
        let detection_base = base.clone();
        let executor = cx.background_executor().clone();
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let detection_directory = detection.directory.clone();
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
                    let result = this.discard_stale_detected_config(&detection_base, result, cx);
                    this.apply_project_detection(detection, result, window, cx);
                })
                .ok();
            })
            .detach();
    }

    /// Drops a project configuration a detection loaded against a base that a
    /// configuration reload has since replaced, and loads it again against the
    /// current one. The detected root itself is still good.
    fn discard_stale_detected_config(
        &mut self,
        detection_base: &Arc<Config>,
        mut result: ProjectDetectionResult,
        cx: &mut Context<Self>,
    ) -> ProjectDetectionResult {
        if Arc::ptr_eq(detection_base, &self.project_detection_base) || result.config.is_none() {
            return result;
        }
        result.config = None;
        if let Some(root) = result.config_root.clone() {
            self.reload_stale_project_configs(vec![root], cx);
        }
        result
    }

    fn is_active_pane(&self, tab_id: u64, pane_id: u64) -> bool {
        self.current_tab()
            .is_some_and(|tab| tab.id == tab_id && tab.active_pane == pane_id)
    }

    fn apply_project_detection(
        &mut self,
        detection: ProjectDetection,
        result: ProjectDetectionResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ProjectDetection {
            tab_id,
            pane_id,
            directory,
            generation,
        } = detection;
        if self.project_context_policy(tab_id).is_remote() {
            self.projects.forget_pane(pane_id);
            self.tab_groups.arrived(tab_id);
            return;
        }
        let Some(state) = self.projects.detections.get(&pane_id) else {
            return;
        };
        if state.generation != generation || !paths_equal(&state.directory, &directory) {
            return;
        }

        // A registry replaced since the job started without invalidating
        // detections (a project removed in settings) must not have the pane
        // re-enter what it no longer lists.
        let registered_root = result
            .registered_root
            .filter(|root| self.projects.registry.contains(root));
        let group = crate::tab_groups::group_for_detected_directory(
            &crate::tab_groups::DetectedDirectory {
                reported: &directory,
                canonical: result.canonical.as_deref(),
                registered_root: registered_root.as_deref(),
                managed_worktree: result.managed_worktree.as_deref(),
            },
            &self.sidebar.worktrees,
        );
        self.projects.set_group_for_pane(pane_id, group);
        self.place_arriving_tab(tab_id, cx);
        match registered_root {
            Some(registered_root) => {
                let config_root = result
                    .config_root
                    .unwrap_or_else(|| registered_root.clone());
                if self
                    .projects
                    .root_for_pane(pane_id)
                    .is_none_or(|root| !paths_equal(root, &config_root))
                {
                    self.projects.clear_pane_root(pane_id);
                    self.projects.invalidate_active_context();
                }
                self.projects
                    .pane_roots
                    .insert(pane_id, config_root.clone());
                match result.anchor {
                    Some(anchor) => self.projects.pane_anchors.insert(pane_id, anchor),
                    None => self.projects.pane_anchors.remove(&pane_id),
                };
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
                if self.projects.root_for_pane(pane_id).is_some() {
                    self.projects.clear_pane_root(pane_id);
                    self.projects.invalidate_active_context();
                }
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

        if self.is_active_pane(tab_id, pane_id) {
            self.activate_current_project(window, cx);
        } else {
            cx.notify();
        }
    }

    pub(crate) fn activate_current_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((tab_id, pane_id)) = self.current_tab().map(|tab| (tab.id, tab.active_pane))
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
            .map_or(&self.launch_config, |project| &project.effective);
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
        let group = TabGroup::Project(root.clone());
        self.open_project_tab_in_group(root, working_directory, group, window, cx);
    }

    /// Opens a project tab into a chosen tab group. The group is carried across
    /// the configuration load and applied in the same update that opens the
    /// tab, so a tab opened by anything else in the meantime cannot take it.
    pub(crate) fn open_project_tab_in_group(
        &mut self,
        root: PathBuf,
        working_directory: Option<PathBuf>,
        group: TabGroup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.projects.registry.contains(&root) {
            self.show_error_notice(
                format!("{} is not a registered Zetta project", root.display()),
                cx,
            );
            return;
        }
        let registry = self.projects.registry.clone();
        let base = self.launch_config.clone();
        let executor = cx.background_executor().clone();
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let directory = working_directory.clone();
                let (config_root, result) = executor
                    .spawn(async move {
                        let config_root = directory
                            .as_deref()
                            .and_then(|directory| {
                                resolve_registered_project_config_root(directory, &registry)
                            })
                            .unwrap_or(root);
                        let result = ProjectConfig::load(&config_root, &base);
                        (config_root, result)
                    })
                    .await;
                this.update_in(cx, |this, window, cx| match result {
                    Ok(project) => this.with_next_tab_in_group(group, |this| {
                        this.open_loaded_project_tab_with_working_directory(
                            project,
                            working_directory,
                            window,
                            cx,
                        );
                    }),
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
                NewTabRequest {
                    working_directory: Some(working_directory),
                    ..NewTabRequest::new(profile, Some(project), NewTabOrigin::ProjectEntry)
                },
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
        self.sidebar_registry_changed(cx);
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
                        this.sidebar_registry_changed(cx);
                        reload_projects_in_other_windows(window.window_handle().window_id(), cx);
                    }
                    Err(error) => {
                        this.show_error_notice(
                            format!("Could not add project {}: {error:#}", offer.root.display()),
                            cx,
                        );
                    }
                })
                .ok();
            })
            .detach();
    }
}

#[cfg(test)]
#[path = "tests/project_context.rs"]
mod tests;
