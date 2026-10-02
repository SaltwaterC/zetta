//! Runs configuration reloads one at a time and commits each in a fixed order.
//!
//! A reload has three phases:
//!
//! 1. **Snapshot**, on the GUI thread: which windows it covers, which project
//!    roots and multiplexer connections they hold.
//! 2. **Preparation**, on a dedicated worker ([`super::preparation`]): every
//!    file read, the recipient fetch and the daemon requests. A worker thread
//!    rather than an executor task because a daemon upgrade or a GitHub fetch
//!    can take seconds.
//! 3. **Commit**, on the GUI thread, in this order: user themes are registered;
//!    each window is checked against what its daemon and projects returned;
//!    the process-wide theme, terminal settings and keymap are applied once;
//!    each window that passed adopts the configuration; launcher integrations
//!    and the process's own copy follow. A window keeps using its previous
//!    configuration until its own commit, so nothing is ever half-applied.
//!
//! Requests arriving while one runs are coalesced into a single follow-up, and
//! a process-wide request absorbs any window-only ones. A request that is
//! overtaken still commits: its daemon requests were already made, so
//! discarding it would leave windows disagreeing with their daemon until the
//! next reload. What the commit does reject is anything a window changed while
//! the preparation ran; see `commit_prepared_configuration`.

use super::preparation::{HostEnvironment, PreparationRequest, PreparedConfiguration, prepare};
use super::*;

use futures::channel::oneshot;
use gpui::{EntityId, Global, WeakEntity};
use std::thread;

/// What a reload covers.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReloadScope<W> {
    /// Every window, dormant ones included, and the configuration new windows
    /// are opened with.
    Process,
    /// Only these windows. The Reload action uses this; it has never updated
    /// the process's configuration.
    Windows(Vec<W>),
}

impl<W: PartialEq> ReloadScope<W> {
    fn absorb(&mut self, other: Self) {
        match other {
            Self::Process => *self = Self::Process,
            Self::Windows(more) => {
                if let Self::Windows(windows) = self {
                    for window in more {
                        if !windows.contains(&window) {
                            windows.push(window);
                        }
                    }
                }
            }
        }
    }
}

/// Serializes reloads: one runs, and everything asked for meanwhile becomes a
/// single reload that runs after it.
pub(crate) struct ReloadQueue<W, C> {
    running: bool,
    queued: Option<(ReloadScope<W>, Vec<C>)>,
}

impl<W, C> Default for ReloadQueue<W, C> {
    fn default() -> Self {
        Self {
            running: false,
            queued: None,
        }
    }
}

impl<W: PartialEq, C> ReloadQueue<W, C> {
    /// The reload to start now, or `None` when one is already running and this
    /// request has been folded into the one that follows it.
    pub(crate) fn request(
        &mut self,
        scope: ReloadScope<W>,
        completion: C,
    ) -> Option<(ReloadScope<W>, Vec<C>)> {
        if !self.running {
            self.running = true;
            return Some((scope, vec![completion]));
        }
        match &mut self.queued {
            Some((queued, completions)) => {
                queued.absorb(scope);
                completions.push(completion);
            }
            None => self.queued = Some((scope, vec![completion])),
        }
        None
    }

    /// Called when the running reload has committed: the next one to start, if
    /// anything was asked for meanwhile.
    pub(crate) fn finish(&mut self) -> Option<(ReloadScope<W>, Vec<C>)> {
        let next = self.queued.take();
        self.running = next.is_some();
        next
    }
}

/// Why a reload did not apply to a window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReloadFailure {
    /// The configuration file could not be read or parsed. Nothing changed.
    Load(String),
    /// The configuration loaded but this window could not take it.
    Apply(String),
}

impl ReloadFailure {
    pub(crate) fn window_message(&self, config_path: &Path) -> String {
        match self {
            Self::Load(error) => format!("Could not load {}: {error}", config_path.display()),
            Self::Apply(error) => format!("Could not apply {}: {error}", config_path.display()),
        }
    }
}

/// How a committed reload went, handed to every request it answered.
pub(crate) struct ReloadOutcome {
    config_path: PathBuf,
    /// A failure every covered window shares.
    failure: Option<ReloadFailure>,
    window_failures: HashMap<EntityId, String>,
}

impl ReloadOutcome {
    pub(crate) fn window_result(&self, window: EntityId) -> Result<(), ReloadFailure> {
        if let Some(failure) = &self.failure {
            return Err(failure.clone());
        }
        match self.window_failures.get(&window) {
            Some(error) => Err(ReloadFailure::Apply(error.clone())),
            None => Ok(()),
        }
    }

    /// Success only when every covered window took the configuration.
    pub(crate) fn process_result(&self) -> Result<()> {
        match &self.failure {
            Some(ReloadFailure::Load(error)) => anyhow::bail!("{error}"),
            Some(ReloadFailure::Apply(error)) => anyhow::bail!(
                "applying reloaded configuration {}: {error}",
                self.config_path.display()
            ),
            None => match self.window_failures.values().next() {
                Some(error) => anyhow::bail!(
                    "applying reloaded configuration {}: {error}",
                    self.config_path.display()
                ),
                None => Ok(()),
            },
        }
    }
}

pub(crate) type ReloadCompletion = Box<dyn FnOnce(&ReloadOutcome, &mut App)>;

#[derive(Default)]
struct ConfigurationReloads(ReloadQueue<WeakEntity<Zetta>, ReloadCompletion>);

impl Global for ConfigurationReloads {}

/// Reloads the configuration for `scope`, calling `completion` once the result
/// has been committed — or once it is known that nothing will be.
///
/// Deferred, because the snapshot updates every covered window and a request
/// usually comes from inside one of them.
pub(crate) fn request_configuration_reload(
    scope: ReloadScope<WeakEntity<Zetta>>,
    completion: ReloadCompletion,
    cx: &mut App,
) {
    cx.defer(move |cx| {
        let start = cx
            .default_global::<ConfigurationReloads>()
            .0
            .request(scope, completion);
        if let Some((scope, completions)) = start {
            start_reload(scope, completions, cx);
        }
    });
}

/// A window covered by a running reload, and what it held when it started.
struct ReloadTarget {
    zetta: Entity<Zetta>,
    snapshot: WindowReloadSnapshot,
}

fn start_reload(
    scope: ReloadScope<WeakEntity<Zetta>>,
    completions: Vec<ReloadCompletion>,
    cx: &mut App,
) {
    let process_scope = matches!(scope, ReloadScope::Process);
    let windows = match scope {
        ReloadScope::Process => process_zetta_entities(cx),
        ReloadScope::Windows(windows) => windows.iter().filter_map(WeakEntity::upgrade).collect(),
    };
    let paths = if process_scope && cx.has_global::<ZettaProcessState>() {
        let config = &cx.global::<ZettaProcessState>().config;
        Some((config.config_path.clone(), config.keymap_override.clone()))
    } else {
        windows.first().map(|zetta| {
            let config = &zetta.read(cx).launch_config;
            (config.config_path.clone(), config.keymap_override.clone())
        })
    };
    let Some((config_path, keymap_override)) = paths else {
        // Every window this was for has closed.
        let outcome = ReloadOutcome {
            config_path: PathBuf::new(),
            failure: None,
            window_failures: HashMap::new(),
        };
        finish_reload(outcome, completions, cx);
        return;
    };
    let targets = windows
        .into_iter()
        .map(|zetta| {
            let snapshot = zetta.update(cx, |zetta, _| zetta.configuration_reload_snapshot());
            ReloadTarget { zetta, snapshot }
        })
        .collect::<Vec<_>>();
    let request = preparation_request(config_path.clone(), keymap_override, &targets);
    let prepared = run_on_worker("zetta-config-reload", move || {
        prepare(&request, &HostEnvironment)
    });
    cx.spawn(async move |cx| {
        let prepared = prepared
            .await
            .unwrap_or_else(|_| Err(anyhow::anyhow!("the configuration reload worker stopped")));
        cx.update(|cx| {
            let outcome = commit_reload(process_scope, config_path, targets, prepared, cx);
            finish_reload(outcome, completions, cx);
        });
    })
    .detach();
}

fn preparation_request(
    config_path: PathBuf,
    keymap_override: Option<PathBuf>,
    targets: &[ReloadTarget],
) -> PreparationRequest {
    let mut project_roots = Vec::<PathBuf>::new();
    for root in targets
        .iter()
        .flat_map(|target| &target.snapshot.project_roots)
    {
        if !project_roots.contains(root) {
            project_roots.push(root.clone());
        }
    }
    PreparationRequest {
        config_path,
        keymap_override,
        project_roots,
        #[cfg(feature = "zmux")]
        daemons: targets
            .iter()
            .filter_map(|target| target.snapshot.mux.clone())
            .map(|handle| Arc::new(handle) as Arc<dyn super::preparation::ReloadDaemon>)
            .collect(),
    }
}

fn finish_reload(outcome: ReloadOutcome, completions: Vec<ReloadCompletion>, cx: &mut App) {
    for completion in completions {
        completion(&outcome, cx);
    }
    let next = cx.default_global::<ConfigurationReloads>().0.finish();
    if let Some((scope, completions)) = next {
        start_reload(scope, completions, cx);
    }
}

/// Runs blocking work on a named thread of its own, for work that can take
/// long enough that it should not occupy one of the executor's workers.
pub(super) fn run_on_worker<T: Send + 'static>(
    name: &str,
    work: impl FnOnce() -> T + Send + 'static,
) -> oneshot::Receiver<T> {
    let (sender, receiver) = oneshot::channel();
    if let Err(error) = thread::Builder::new().name(name.to_owned()).spawn(move || {
        let _ = sender.send(work());
    }) {
        // The closure, and the sender in it, is dropped: the receiver resolves
        // as cancelled and the caller reports that.
        log::error!("could not start {name}: {error}");
    }
    receiver
}

/// Phase 3: applies a finished preparation, in the order the module describes.
fn commit_reload(
    process_scope: bool,
    config_path: PathBuf,
    targets: Vec<ReloadTarget>,
    prepared: Result<PreparedConfiguration>,
    cx: &mut App,
) -> ReloadOutcome {
    let mut outcome = ReloadOutcome {
        config_path,
        failure: None,
        window_failures: HashMap::new(),
    };
    let mut prepared = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            let failure = ReloadFailure::Load(format!("{error:#}"));
            // The Reload action has always reported a file that does not load;
            // the watcher, which sees every intermediate save an editor makes,
            // has not.
            if !process_scope {
                report_failure(&targets, &failure, &outcome.config_path, cx);
            }
            outcome.failure = Some(failure);
            return outcome;
        }
    };

    register_user_themes(std::mem::take(&mut prepared.themes), cx);

    let mut validated = Vec::new();
    for target in targets {
        match prepared.validate_for(
            target.snapshot.daemon_identity(),
            &target.snapshot.project_roots,
        ) {
            Ok(projects) => validated.push((target, projects)),
            Err(error) => {
                let failure = ReloadFailure::Apply(error.clone());
                report_failure(
                    std::slice::from_ref(&target),
                    &failure,
                    &outcome.config_path,
                    cx,
                );
                outcome
                    .window_failures
                    .insert(target.zetta.entity_id(), error);
            }
        }
    }

    if let Some((first, _)) = validated.first() {
        let no_mux = first.zetta.read(cx).no_mux;
        if let Err(error) = apply_process_wide_settings(&prepared, no_mux, cx) {
            let failure = ReloadFailure::Apply(format!("{error:#}"));
            let targets = validated
                .into_iter()
                .map(|(target, _)| target)
                .collect::<Vec<_>>();
            report_failure(&targets, &failure, &outcome.config_path, cx);
            outcome.failure = Some(failure);
            return outcome;
        }
    }
    for (target, projects) in validated {
        let committed = target.zetta.update(cx, |zetta, cx| {
            zetta.commit_prepared_configuration(&prepared, &target.snapshot, projects, cx)
        });
        if let Err(error) = committed {
            outcome
                .window_failures
                .insert(target.zetta.entity_id(), error);
        }
    }

    update_launcher_integrations(&prepared, cx);
    if process_scope && cx.has_global::<ZettaProcessState>() {
        let process = cx.global_mut::<ZettaProcessState>();
        process.config = prepared.config.as_ref().clone();
        process.config_file_stamp = prepared.config_stamp;
        process.configuration_error = None;
    }
    outcome
}

fn report_failure(
    targets: &[ReloadTarget],
    failure: &ReloadFailure,
    config_path: &Path,
    cx: &mut App,
) {
    let message = failure.window_message(config_path);
    for target in targets {
        target.zetta.update(cx, |zetta, cx| {
            zetta.configuration_error = Some(message.clone());
            cx.notify();
        });
    }
}

/// The state every window reads but none owns: the selected theme, terminal
/// settings, and the keymap. Applied once, before any window commits, and
/// only when at least one will.
fn apply_process_wide_settings(
    prepared: &PreparedConfiguration,
    no_mux: bool,
    cx: &mut App,
) -> Result<()> {
    let config = prepared.config.as_ref();
    apply_config_settings(config, cx)?;
    let profile_count = visible_profile_count(&config.profiles, &config.hidden_profiles);
    bind_keybindings(
        &config.keymap_path,
        &prepared.keymap,
        profile_count,
        no_mux,
        cx,
    );
    // Over-bumping only costs an editor one extra theme query.
    crate::process_control::bump_pane_theme_revision();
    Ok(())
}

/// The native launchers' profile lists, once per reload rather than once per
/// window.
fn update_launcher_integrations(prepared: &PreparedConfiguration, cx: &mut App) {
    let config = prepared.config.as_ref();
    #[cfg(windows)]
    windows_integration::update_profile_jump_list(
        config.profiles.clone(),
        config.hidden_profiles.clone(),
    );
    #[cfg(target_os = "linux")]
    if prepared.desktop_entry_updated {
        crate::startup::schedule_linux_desktop_window_reassociation(cx);
    }
    #[cfg(target_os = "macos")]
    update_native_macos_dock_menu(cx, &config.profiles, &config.hidden_profiles);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let _ = cx;
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = config;
}

#[cfg(test)]
#[path = "../tests/configuration_reload/coordinator.rs"]
mod tests;
