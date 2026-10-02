//! The half of a configuration reload that reads the disk and talks to the
//! multiplexer, run on a worker so that no window waits on it.
//!
//! Everything here is plain data in and plain data out: no GPUI state is read
//! or written, so the result can be prepared once for a whole process and then
//! committed to each window on the GUI thread. The parts that reach outside the
//! request's own files — user themes, recipient resolution, the desktop entry —
//! go through [`PreparationEnvironment`], which is what lets the tests stand in
//! a slow or failing daemon without touching the user's real directories.

use super::*;

#[cfg(feature = "session-persistence")]
use crate::session_auto_protect::SessionAutoProtect;
use std::thread;

#[cfg(feature = "zmux")]
use crate::mux::{MuxReconfiguration, MuxReconfigureHandle};

/// What a reload has to read, gathered on the GUI thread from every window it
/// covers before the worker starts.
pub(crate) struct PreparationRequest {
    pub(crate) config_path: PathBuf,
    pub(crate) keymap_override: Option<PathBuf>,
    /// Every registered project root any covered window has loaded or has a
    /// pane in. Each is read once however many windows share it.
    pub(crate) project_roots: Vec<PathBuf>,
    /// One per multiplexer connection the covered windows hold.
    #[cfg(feature = "zmux")]
    pub(crate) daemons: Vec<Arc<dyn ReloadDaemon>>,
}

/// A multiplexer connection a reload sends the new retention policy to.
#[cfg(feature = "zmux")]
pub(crate) trait ReloadDaemon: Send + Sync {
    /// Two windows sharing a connection report the same identity, and get one
    /// request between them.
    fn identity(&self) -> usize;
    fn reconfigure(&self, plan: &MuxReconfiguration) -> Result<()>;
}

#[cfg(feature = "zmux")]
impl ReloadDaemon for MuxReconfigureHandle {
    fn identity(&self) -> usize {
        MuxReconfigureHandle::identity(self)
    }

    fn reconfigure(&self, plan: &MuxReconfiguration) -> Result<()> {
        MuxReconfigureHandle::reconfigure(self, plan)
    }
}

/// The work a preparation does outside the files its request names.
pub(crate) trait PreparationEnvironment: Sync {
    /// The user theme files that changed since the last reload, parsed.
    fn user_themes(&self) -> Result<Vec<theme::ThemeFamily>>;
    /// Resolves the request every daemon is sent; can be a GitHub fetch.
    #[cfg(feature = "zmux")]
    fn daemon_plan(&self, sessions: &crate::config::SessionsConfig) -> Result<MuxReconfiguration>;
    /// Can also be a GitHub fetch.
    #[cfg(feature = "session-persistence")]
    fn auto_protect(
        &self,
        persistence: &crate::config::SessionPersistenceConfig,
    ) -> Result<Option<SessionAutoProtect>>;
    /// Rewrites the managed desktop entry's profile actions; whether it did.
    #[cfg(target_os = "linux")]
    fn update_desktop_entry(&self, config: &Config) -> bool;
}

/// The real environment: the user's theme directory, GitHub, and `$HOME`.
pub(crate) struct HostEnvironment;

impl PreparationEnvironment for HostEnvironment {
    fn user_themes(&self) -> Result<Vec<theme::ThemeFamily>> {
        prepare_user_themes()
    }

    #[cfg(feature = "zmux")]
    fn daemon_plan(&self, sessions: &crate::config::SessionsConfig) -> Result<MuxReconfiguration> {
        MuxReconfiguration::resolve(sessions)
    }

    #[cfg(feature = "session-persistence")]
    fn auto_protect(
        &self,
        persistence: &crate::config::SessionPersistenceConfig,
    ) -> Result<Option<SessionAutoProtect>> {
        SessionAutoProtect::resolve(persistence)
    }

    #[cfg(target_os = "linux")]
    fn update_desktop_entry(&self, config: &Config) -> bool {
        linux_desktop::update_profile_actions(&config.profiles, &config.hidden_profiles)
            .log_err()
            .unwrap_or(false)
    }
}

/// Everything a reload needed from the disk and the daemon, ready to commit.
///
/// Failures that only affect some windows are kept per item, as text, because
/// one failure is reported to every window it affects.
pub(crate) struct PreparedConfiguration {
    /// Shared by every window it is committed to, as its project detection base.
    pub(crate) config: Arc<Config>,
    pub(crate) config_stamp: ConfigFileStamp,
    /// Taken by the commit, which registers them before anything that might
    /// name one of them.
    pub(crate) themes: Vec<theme::ThemeFamily>,
    pub(crate) keymap: KeymapSource,
    /// Built from the same read of the file as `config`.
    pub(crate) settings_form: Result<ConfigurationForm, String>,
    pub(crate) projects: HashMap<PathBuf, Result<Arc<ProjectConfig>, String>>,
    /// Keyed by [`ReloadDaemon::identity`].
    #[cfg(feature = "zmux")]
    pub(crate) daemons: HashMap<usize, Result<(), String>>,
    #[cfg(feature = "session-persistence")]
    pub(crate) auto_protect: Result<Option<Arc<SessionAutoProtect>>, String>,
    #[cfg(target_os = "linux")]
    pub(crate) desktop_entry_updated: bool,
}

impl PreparedConfiguration {
    /// The project configurations a window should switch to, or why it cannot
    /// take this configuration at all.
    ///
    /// A window whose daemon refused the new policy, or one of whose projects
    /// failed to load, keeps its current configuration, exactly as it did when
    /// these steps failed part-way through a synchronous reload.
    pub(crate) fn validate_for(
        &self,
        daemon: Option<usize>,
        project_roots: &[PathBuf],
    ) -> std::result::Result<Vec<Arc<ProjectConfig>>, String> {
        #[cfg(feature = "zmux")]
        if let Some(daemon) = daemon {
            match self.daemons.get(&daemon) {
                Some(Ok(())) => {}
                Some(Err(error)) => return Err(error.clone()),
                None => return Err("the multiplexer was not reconfigured".to_owned()),
            }
        }
        #[cfg(not(feature = "zmux"))]
        let _ = daemon;
        project_roots
            .iter()
            .map(|root| match self.projects.get(root) {
                Some(Ok(project)) => Ok(project.clone()),
                Some(Err(error)) => Err(error.clone()),
                None => Err(format!(
                    "project configuration {} was not prepared",
                    ProjectConfig::path_for(root).display()
                )),
            })
            .collect()
    }
}

/// Reads and resolves everything `request` needs. Blocking: run it on a worker.
///
/// Only a configuration that does not load is an error here; everything else
/// is recorded in the result for the commit to apply to the windows it affects.
/// The independent slow parts — each project, each daemon, automatic
/// protection — run in parallel, so one slow project path or one slow daemon
/// does not hold up the rest.
pub(crate) fn prepare(
    request: &PreparationRequest,
    environment: &dyn PreparationEnvironment,
) -> Result<PreparedConfiguration> {
    // Taken before the read, so a write landing between the two is seen as a
    // change by the next poll rather than absorbed.
    let config_stamp = config_file_stamp(&request.config_path);
    let (config, source) =
        Config::load_with_source(Some(&request.config_path), request.keymap_override.clone())?;
    let settings_form = ConfigurationForm::parse(source.as_deref(), &config.config_path, &config)
        .map_err(|error| format!("{error:#}"));
    let keymap = read_keymap_source(&config.keymap_path);
    let themes = environment.user_themes().log_err().unwrap_or_default();

    let prepared = thread::scope(|scope| {
        let projects = request
            .project_roots
            .iter()
            .map(|root| {
                let config = &config;
                (
                    root.clone(),
                    scope.spawn(move || load_project_configuration(root, config)),
                )
            })
            .collect::<Vec<_>>();
        #[cfg(feature = "zmux")]
        let daemons = scope.spawn(|| reconfigure_daemons(&request.daemons, &config, environment));
        #[cfg(feature = "session-persistence")]
        let auto_protect = scope.spawn(|| {
            environment
                .auto_protect(&config.sessions.persistence)
                .map(|auto_protect| auto_protect.map(Arc::new))
                .map_err(|error| format!("{error:#}"))
        });
        #[cfg(target_os = "linux")]
        let desktop_entry_updated = environment.update_desktop_entry(&config);

        let projects = projects
            .into_iter()
            .map(|(root, load)| {
                let result = load
                    .join()
                    .unwrap_or_else(|_| Err("loading the project configuration panicked".into()));
                (root, result)
            })
            .collect();
        PreparedParts {
            projects,
            #[cfg(feature = "zmux")]
            daemons: daemons.join().unwrap_or_default(),
            #[cfg(feature = "session-persistence")]
            auto_protect: auto_protect.join().unwrap_or_else(|_| {
                Err("resolving automatic session protection panicked".to_owned())
            }),
            #[cfg(target_os = "linux")]
            desktop_entry_updated,
        }
    });

    Ok(PreparedConfiguration {
        config: Arc::new(config),
        config_stamp,
        themes,
        keymap,
        settings_form,
        projects: prepared.projects,
        #[cfg(feature = "zmux")]
        daemons: prepared.daemons,
        #[cfg(feature = "session-persistence")]
        auto_protect: prepared.auto_protect,
        #[cfg(target_os = "linux")]
        desktop_entry_updated: prepared.desktop_entry_updated,
    })
}

/// What the scoped threads in [`prepare`] hand back.
struct PreparedParts {
    projects: HashMap<PathBuf, Result<Arc<ProjectConfig>, String>>,
    #[cfg(feature = "zmux")]
    daemons: HashMap<usize, Result<(), String>>,
    #[cfg(feature = "session-persistence")]
    auto_protect: Result<Option<Arc<SessionAutoProtect>>, String>,
    #[cfg(target_os = "linux")]
    desktop_entry_updated: bool,
}

fn load_project_configuration(
    root: &Path,
    config: &Config,
) -> std::result::Result<Arc<ProjectConfig>, String> {
    ProjectConfig::load(root, config)
        .map(Arc::new)
        .with_context(|| {
            format!(
                "reloading project configuration {}",
                ProjectConfig::path_for(root).display()
            )
        })
        .map_err(|error| format!("{error:#}"))
}

/// Sends the new policy to every distinct daemon, in parallel.
///
/// The policy is resolved once, and only when there is a daemon to send it to:
/// with `github:` recipients that resolution is a network fetch, which used to
/// be made once per window, on the GUI thread, before each blocking request.
#[cfg(feature = "zmux")]
fn reconfigure_daemons(
    daemons: &[Arc<dyn ReloadDaemon>],
    config: &Config,
    environment: &dyn PreparationEnvironment,
) -> HashMap<usize, Result<(), String>> {
    let mut distinct = Vec::<&Arc<dyn ReloadDaemon>>::new();
    for daemon in daemons {
        if !distinct
            .iter()
            .any(|seen| seen.identity() == daemon.identity())
        {
            distinct.push(daemon);
        }
    }
    if distinct.is_empty() {
        return HashMap::new();
    }
    let plan = match environment.daemon_plan(&config.sessions) {
        Ok(plan) => plan,
        Err(error) => {
            let error = format!("{error:#}");
            return distinct
                .into_iter()
                .map(|daemon| (daemon.identity(), Err(error.clone())))
                .collect();
        }
    };
    thread::scope(|scope| {
        distinct
            .into_iter()
            .map(|daemon| {
                let plan = &plan;
                (
                    daemon.identity(),
                    scope.spawn(move || daemon.reconfigure(plan)),
                )
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|(identity, request)| {
                let result = match request.join() {
                    Ok(result) => result.map_err(|error| format!("{error:#}")),
                    Err(_) => Err("reconfiguring the multiplexer panicked".to_owned()),
                };
                (identity, result)
            })
            .collect()
    })
}

#[cfg(test)]
#[path = "../tests/configuration_reload/preparation.rs"]
mod tests;
