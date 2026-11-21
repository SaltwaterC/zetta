//! What the window knows about projects: which pane belongs to which
//! project root, the detections in flight, the offers the user dismissed, and
//! where a pane's directory is read from to detect one.

use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectOffer {
    pub(crate) root: PathBuf,
    pub(super) pane_id: u64,
}

#[derive(Clone, Debug)]
pub(super) struct ProjectDetectionState {
    pub(super) directory: PathBuf,
    pub(super) generation: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectState {
    pub(crate) registry: ProjectRegistry,
    pub(crate) trust_prompt: Option<crate::project_trust_ui::ProjectTrustPrompt>,
    pub(crate) configs: HashMap<PathBuf, Arc<ProjectConfig>>,
    pub(crate) pane_roots: HashMap<u64, PathBuf>,
    /// How each detected pane root maps onto the directories its shell
    /// reports; see [`PaneRootAnchor`]. A root inserted any other way has no
    /// entry and is compared against its own spelling.
    pub(super) pane_anchors: HashMap<u64, PaneRootAnchor>,
    /// The tab group each detected pane's directory belongs to, so a tab
    /// arriving from a session can be placed by its saved directories or a
    /// detection that ran while it was away; see `Zetta::insert_tab_into_active_group`.
    pub(super) pane_groups: HashMap<u64, TabGroup>,
    pub(super) detections: HashMap<u64, ProjectDetectionState>,
    pub(super) next_detection_generation: u64,
    pub(super) entered: HashSet<(u64, String)>,
    pub(super) inherited_tab_icons: HashMap<u64, Option<IconName>>,
    pub(super) active_context: Option<(u64, Option<usize>)>,
    pub(super) dismissed_offers: HashSet<String>,
    pub(crate) offer: Option<ProjectOffer>,
}

impl ProjectState {
    pub(crate) fn load() -> Result<Self> {
        Ok(Self::new(ProjectRegistry::load()?))
    }

    pub(crate) fn new(registry: ProjectRegistry) -> Self {
        Self {
            registry,
            trust_prompt: None,
            configs: HashMap::new(),
            pane_roots: HashMap::new(),
            pane_anchors: HashMap::new(),
            pane_groups: HashMap::new(),
            detections: HashMap::new(),
            next_detection_generation: 0,
            entered: HashSet::new(),
            inherited_tab_icons: HashMap::new(),
            active_context: None,
            dismissed_offers: HashSet::new(),
            offer: None,
        }
    }

    /// The group resolved from the pane's saved directory or last local detection.
    pub(crate) fn group_for_pane(&self, pane_id: u64) -> Option<&TabGroup> {
        self.pane_groups.get(&pane_id)
    }

    pub(crate) fn set_group_for_pane(&mut self, pane_id: u64, group: TabGroup) {
        self.pane_groups.insert(pane_id, group);
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
        self.insert_shared_config(config.clone());
        config
    }

    /// [`Self::insert_config`] for a configuration a reload loaded once and
    /// hands to every window that has the project open.
    pub(crate) fn insert_shared_config(&mut self, config: Arc<ProjectConfig>) {
        self.configs.insert(config.root.clone(), config);
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

    pub(super) fn offer_is_dismissed(&self, root: &Path) -> bool {
        self.dismissed_offers.contains(&project_key(root))
    }

    pub(crate) fn clear_removed_roots(&mut self) {
        self.pane_roots
            .retain(|_, root| is_registered_project_config_root(root, &self.registry));
        let pane_roots = &self.pane_roots;
        self.pane_anchors
            .retain(|pane_id, _| pane_roots.contains_key(pane_id));
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
        // A pane changing project can change the theme that project selects.
        crate::process_control::bump_pane_theme_revision();
        self.active_context = None;
    }

    pub(crate) fn invalidate_detections(&mut self) {
        self.detections.clear();
    }

    pub(super) fn begin_detection(&mut self, pane_id: u64, directory: PathBuf) -> Option<u64> {
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
            match self.pane_anchors.get(&source_pane_id).cloned() {
                Some(anchor) => self.pane_anchors.insert(pane_id, anchor),
                None => self.pane_anchors.remove(&pane_id),
            };
        }
    }

    pub(super) fn clear_pane_root(&mut self, pane_id: u64) {
        self.pane_anchors.remove(&pane_id);
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
        self.pane_groups.remove(&pane_id);
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

    /// Takes a pane out of its project while a remote client owns its
    /// foreground, returning whether it was in one.
    ///
    /// The detection entry is dropped as well as the root. When the client
    /// exits, the shell's reported directory is usually the one it reported
    /// before the session started, and a surviving entry for that directory
    /// would make `begin_detection` skip the detection that brings the project
    /// back.
    pub(super) fn leave_for_remote_foreground(&mut self, pane_id: u64) -> bool {
        let was_in_project = self.root_for_pane(pane_id).is_some();
        self.forget_pane(pane_id);
        if was_in_project {
            self.invalidate_active_context();
        }
        was_in_project
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

pub(super) fn project_key(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        value.to_lowercase()
    } else {
        value
    }
}

#[derive(Debug)]
pub(super) struct ProjectDetectionResult {
    pub(super) registered_root: Option<PathBuf>,
    pub(super) config_root: Option<PathBuf>,
    pub(super) config: Option<Result<ProjectConfig>>,
    /// Already filtered against the registry the job ran with, so applying it
    /// does not resolve it again on the GUI thread.
    pub(super) offer_root: Option<PathBuf>,
    pub(super) anchor: Option<PaneRootAnchor>,
    /// The directory as the filesystem spells it, when it could be resolved.
    pub(super) canonical: Option<PathBuf>,
    /// The root of the managed worktree the directory is in.
    pub(super) managed_worktree: Option<PathBuf>,
}

/// How a pane's reported directories map onto the canonical root its project
/// was resolved from, so the GUI thread can tell a `cd` that stays inside the
/// project from one that leaves it without touching the filesystem.
///
/// Shells report the directory as the user spelled it, which through a
/// symlink (FreeBSD's `/home`, macOS's `/var`) is not a prefix of the
/// canonical root; and a managed worktree sits beside, not below, its main
/// repository. Both would otherwise read as leaving the project on every `cd`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PaneRootAnchor {
    /// The reported spelling of `resolved`.
    pub(super) reported: PathBuf,
    /// The canonical directory the project was reached through: the managed
    /// worktree's root, or else the configuration root.
    pub(super) resolved: PathBuf,
    /// Whether `resolved` is a managed worktree. Resolution consults the
    /// worktree's metadata before the registry, so a registered root inside
    /// the worktree (a stale duplicate registration) does not claim it.
    pub(super) managed_worktree: bool,
}

impl PaneRootAnchor {
    pub(super) fn identity(root: &Path) -> Self {
        Self {
            reported: root.to_path_buf(),
            resolved: root.to_path_buf(),
            managed_worktree: false,
        }
    }

    /// The anchor for `resolved`, given one directory below it in both the
    /// spelling the shell reported and its canonical form.
    pub(super) fn new(
        reported_directory: &Path,
        canonical_directory: &Path,
        resolved: PathBuf,
        managed_worktree: bool,
    ) -> Self {
        let reported = canonical_directory
            .strip_prefix(&resolved)
            .ok()
            .filter(|suffix| reported_directory.ends_with(suffix))
            .and_then(|suffix| {
                reported_directory
                    .ancestors()
                    .nth(suffix.components().count())
            })
            .map_or_else(|| resolved.clone(), Path::to_path_buf);
        Self {
            reported,
            resolved,
            managed_worktree,
        }
    }

    /// `directory` in the canonical spelling, when it is lexically below this
    /// anchor in either spelling.
    pub(super) fn resolve(&self, directory: &Path) -> Option<PathBuf> {
        if path_is_within_lexically(directory, &self.resolved) {
            return Some(directory.to_path_buf());
        }
        directory
            .strip_prefix(&self.reported)
            .ok()
            .map(|suffix| self.resolved.join(suffix))
    }
}

/// Whether a pane in `root` may keep it while a detection for `directory`
/// is still running, decided without touching the filesystem.
///
/// Keeping is the common case, a `cd` inside the project, and avoids the
/// project's theme flickering off and back on. Anything else — a directory
/// lexically outside the project, or one claimed by a registered project
/// nested below it — clears the root at once, so no command acts on the old
/// project while the background resolution is outstanding. A transition only
/// the worker can see (a worktree-local `.zetta/config.json`, a symlink inside
/// the project) keeps the old root until its result arrives and replaces it.
pub(super) fn detection_keeps_pane_root(
    directory: &Path,
    root: &Path,
    anchor: Option<&PaneRootAnchor>,
    registry: &ProjectRegistry,
) -> bool {
    let identity;
    let anchor = match anchor {
        Some(anchor) => anchor,
        None => {
            identity = PaneRootAnchor::identity(root);
            &identity
        }
    };
    let Some(resolved_directory) = anchor.resolve(directory) else {
        return false;
    };
    anchor.managed_worktree
        || registry
            .matching_root_lexically(&resolved_directory)
            .is_none_or(|matched| {
                paths_equal(matched, root) || !path_is_within_lexically(matched, &anchor.resolved)
            })
}

pub(super) fn detect_project_for_directory(
    directory: &Path,
    registry: &ProjectRegistry,
    base: &Config,
    loaded_roots: &[PathBuf],
) -> ProjectDetectionResult {
    // Canonicalized once: everything below compares against this answer
    // lexically rather than asking the filesystem again per registry root.
    let canonical = fs::canonicalize(directory).ok();
    let is_directory = canonical
        .as_deref()
        .is_some_and(|canonical| fs::metadata(canonical).is_ok_and(|metadata| metadata.is_dir()));
    let lookup = canonical.as_deref().unwrap_or(directory);
    let resolution = if is_directory {
        resolve_registered_project_canonical(lookup, registry)
    } else {
        // Deleted, unreadable or not a directory: what the lexical path
        // matches is all there is to know, and asking again would only wait on
        // the same failure.
        let root = registry.matching_root_lexically(lookup).cloned();
        ProjectRootResolution {
            config_root: root.clone(),
            root,
            managed_worktree: None,
        }
    };
    let managed_worktree = resolution
        .managed_worktree
        .as_ref()
        .map(|worktree| worktree.root.clone());
    if let Some(root) = resolution.root {
        let config_root = resolution.config_root.unwrap_or_else(|| root.clone());
        let config = (!loaded_roots
            .iter()
            .any(|loaded| paths_equal(loaded, &config_root)))
        .then(|| ProjectConfig::load_in_registry(&config_root, base, registry));
        let anchor = canonical
            .as_deref()
            .map(|canonical| match resolution.managed_worktree {
                Some(worktree) => PaneRootAnchor::new(directory, canonical, worktree.root, true),
                None => PaneRootAnchor::new(directory, canonical, config_root.clone(), false),
            });
        return ProjectDetectionResult {
            registered_root: Some(root),
            config_root: Some(config_root),
            config,
            offer_root: None,
            anchor,
            canonical,
            managed_worktree,
        };
    }
    let offer_root = is_directory
        .then(|| discover_project_config_canonical(lookup).ok().flatten())
        .flatten()
        .filter(|root| {
            resolve_registered_project_canonical(root, registry)
                .root
                .is_none()
        });
    ProjectDetectionResult {
        registered_root: None,
        config_root: None,
        config: None,
        offer_root,
        anchor: None,
        canonical,
        managed_worktree: None,
    }
}

pub(super) fn wsl_distribution(profile: &Profile) -> Option<&str> {
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

/// Whether a tab may consume project-scoped configuration from this viewer.
/// Remote sessions keep reporting their working directories, but those paths
/// belong to the remote host and must never select a local project.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProjectContextPolicy {
    Local,
    Remote,
}

impl ProjectContextPolicy {
    pub(crate) const fn is_remote(self) -> bool {
        matches!(self, Self::Remote)
    }

    pub(crate) fn theme_fallback<'a>(
        self,
        profile: &'a Profile,
        project: Option<&'a ProjectConfig>,
        application_theme: &'a Arc<Theme>,
    ) -> TerminalThemeFallback<'a> {
        match self {
            Self::Local => TerminalThemeFallback::Configured { profile, project },
            Self::Remote => TerminalThemeFallback::Application(application_theme),
        }
    }
}

/// What project detection may do with where a pane is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProjectDetectionDirectory {
    /// Confirmed by the pane's shell or the operating system: detection may
    /// enter the project it is in.
    Verified(PathBuf),
    /// Only an unauthenticated report says so. It may take the pane out of a
    /// project it is outside of, and nothing more.
    Unverified(PathBuf),
    /// Nothing usable is known.
    Unknown,
}

/// Where project detection starts from for `pane`.
///
/// Entering a project applies its configuration and, the first time in a tab,
/// its `initial_split`, so the directory has to be one the pane's shell
/// confirmed — its authenticated report, the foreground process's directory,
/// or a WSL shell's tracking file; see "Which directory a pane is in" in
/// `pane.rs`. When the latest report of any kind says something else, the
/// pane is `Unverified` there: a printed `zetta-cwd` can never enter a
/// project, where previously `cat` of a file naming a registered project's
/// directory would have applied its initial split.
pub(crate) fn project_detection_directory(
    pane: &TerminalPane,
    cx: &App,
) -> ProjectDetectionDirectory {
    let (verified, reported) = if is_wsl_shell(&pane.profile.command) {
        let directory = |directory: Option<String>| {
            directory.and_then(|d| wsl_reported_directory(&pane.profile, &d))
        };
        (
            directory(pane.wsl_working_directory(cx)),
            directory(pane.reported_wsl_working_directory(cx)),
        )
    } else {
        let directory = |current: Option<(PathBuf, bool)>| {
            current
                .filter(|(_, authoritative)| *authoritative)
                .map(|(directory, _)| directory)
        };
        (
            directory(pane.current_directory(cx)),
            directory(pane.reported_current_directory(cx)),
        )
    };
    match (verified, reported) {
        (verified, Some(reported)) if verified.as_ref() != Some(&reported) => {
            ProjectDetectionDirectory::Unverified(reported)
        }
        (Some(verified), _) => ProjectDetectionDirectory::Verified(verified),
        (None, _) => ProjectDetectionDirectory::Unknown,
    }
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
