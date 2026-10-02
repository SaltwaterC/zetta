use super::*;

#[cfg(feature = "zmux")]
use std::sync::Mutex;
#[cfg(feature = "zmux")]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(feature = "zmux")]
use std::sync::mpsc;
#[cfg(feature = "zmux")]
use std::time::Duration;

fn scratch_directory(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "zetta-config-reload-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&directory).unwrap();
    directory
}

/// Stands in for the user's theme directory, GitHub and `$HOME`, and counts
/// how often the daemon request is resolved.
#[derive(Default)]
struct StubEnvironment {
    #[cfg(feature = "zmux")]
    plan_resolutions: AtomicUsize,
    #[cfg(feature = "zmux")]
    plan_error: Option<&'static str>,
}

impl PreparationEnvironment for StubEnvironment {
    fn user_themes(&self) -> Result<Vec<theme::ThemeFamily>> {
        Ok(Vec::new())
    }

    #[cfg(feature = "zmux")]
    fn daemon_plan(&self, sessions: &crate::config::SessionsConfig) -> Result<MuxReconfiguration> {
        self.plan_resolutions.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = self.plan_error {
            anyhow::bail!("{error}");
        }
        let retention = sessions.to_zmux_retention()?;
        #[cfg(feature = "session-persistence")]
        return Ok(MuxReconfiguration::for_test(
            zmux::client::ResolvedRetention {
                requested_retention: retention,
                effective_retention: retention,
                degraded_reason: None,
                recipients: Vec::new(),
            },
        ));
        #[cfg(not(feature = "session-persistence"))]
        Ok(MuxReconfiguration::for_test(retention))
    }

    #[cfg(feature = "session-persistence")]
    fn auto_protect(
        &self,
        _: &crate::config::SessionPersistenceConfig,
    ) -> Result<Option<SessionAutoProtect>> {
        Ok(None)
    }

    #[cfg(target_os = "linux")]
    fn update_desktop_entry(&self, _: &Config) -> bool {
        false
    }
}

/// A daemon that records each request and answers with `result`.
#[cfg(feature = "zmux")]
struct StubDaemon {
    identity: usize,
    requests: AtomicUsize,
    result: Mutex<Box<dyn FnMut() -> Result<()> + Send>>,
}

#[cfg(feature = "zmux")]
impl StubDaemon {
    fn new(identity: usize, result: impl FnMut() -> Result<()> + Send + 'static) -> Arc<Self> {
        Arc::new(Self {
            identity,
            requests: AtomicUsize::new(0),
            result: Mutex::new(Box::new(result)),
        })
    }
}

#[cfg(feature = "zmux")]
impl ReloadDaemon for StubDaemon {
    fn identity(&self) -> usize {
        self.identity
    }

    fn reconfigure(&self, _: &MuxReconfiguration) -> Result<()> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        let mut result = self.result.lock().unwrap();
        result()
    }
}

fn request_for(directory: &Path, config: Option<&str>) -> PreparationRequest {
    let config_path = directory.join("config.json");
    if let Some(config) = config {
        fs::write(&config_path, config).unwrap();
    }
    PreparationRequest {
        config_path,
        // Kept inside the scratch directory so the user's keymap is never read.
        keymap_override: Some(directory.join("keymap.json")),
        project_roots: Vec::new(),
        #[cfg(feature = "zmux")]
        daemons: Vec::new(),
    }
}

fn project_root(directory: &Path, name: &str) -> PathBuf {
    let root = directory.join(name);
    fs::create_dir_all(root.join(crate::project::PROJECT_CONFIG_DIRECTORY)).unwrap();
    fs::write(ProjectConfig::path_for(&root), "{}").unwrap();
    fs::canonicalize(root).unwrap()
}

#[test]
fn a_configuration_that_does_not_load_prepares_nothing_else() {
    let directory = scratch_directory("unloadable");
    let mut request = request_for(&directory, Some("{ not json"));
    #[cfg(feature = "zmux")]
    let daemon = StubDaemon::new(1, || Ok(()));
    #[cfg(feature = "zmux")]
    request.daemons.push(daemon.clone());
    request
        .project_roots
        .push(project_root(&directory, "project"));
    let environment = StubEnvironment::default();

    let error = prepare(&request, &environment)
        .err()
        .expect("must not load");

    assert!(format!("{error:#}").contains("config.json"), "{error:#}");
    #[cfg(feature = "zmux")]
    {
        assert_eq!(environment.plan_resolutions.load(Ordering::SeqCst), 0);
        assert_eq!(daemon.requests.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn a_missing_configuration_file_prepares_the_defaults() {
    let directory = scratch_directory("missing");
    let request = request_for(&directory, None);

    let prepared = prepare(&request, &StubEnvironment::default()).unwrap();

    assert_eq!(prepared.config.config_path, request.config_path);
    assert!(prepared.settings_form.is_ok());
    assert!(prepared.validate_for(None, &[]).unwrap().is_empty());
}

#[test]
fn the_keymap_is_read_from_the_configured_path() {
    let directory = scratch_directory("keymap");
    let request = request_for(&directory, Some("{}"));
    fs::write(directory.join("keymap.json"), "[]").unwrap();

    let prepared = prepare(&request, &StubEnvironment::default()).unwrap();

    assert_eq!(
        format!("{:?}", prepared.keymap),
        format!("{:?}", read_keymap_source(&directory.join("keymap.json")))
    );
    assert_eq!(prepared.config.keymap_path, directory.join("keymap.json"));
}

#[test]
fn an_unavailable_project_fails_only_the_windows_that_have_it() {
    let directory = scratch_directory("projects");
    let mut request = request_for(&directory, Some("{}"));
    let present = project_root(&directory, "present");
    let missing = directory.join("missing");
    request.project_roots = vec![present.clone(), missing.clone()];

    let prepared = prepare(&request, &StubEnvironment::default()).unwrap();

    let projects = prepared
        .validate_for(None, std::slice::from_ref(&present))
        .unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].root, present);
    let error = prepared
        .validate_for(None, &[present.clone(), missing.clone()])
        .unwrap_err();
    assert!(error.contains("reloading project configuration"), "{error}");
}

#[test]
fn a_root_the_preparation_never_saw_is_not_silently_accepted() {
    let directory = scratch_directory("unprepared");
    let request = request_for(&directory, Some("{}"));

    let prepared = prepare(&request, &StubEnvironment::default()).unwrap();

    assert!(
        prepared
            .validate_for(None, &[directory.join("elsewhere")])
            .is_err()
    );
}

#[test]
fn every_window_sharing_a_project_shares_one_loaded_configuration() {
    let directory = scratch_directory("shared-project");
    let mut request = request_for(&directory, Some("{}"));
    let root = project_root(&directory, "project");
    request.project_roots = vec![root.clone()];

    let prepared = prepare(&request, &StubEnvironment::default()).unwrap();

    let first = prepared
        .validate_for(None, std::slice::from_ref(&root))
        .unwrap();
    let second = prepared
        .validate_for(None, std::slice::from_ref(&root))
        .unwrap();
    assert!(Arc::ptr_eq(&first[0], &second[0]));
}

#[cfg(feature = "zmux")]
#[test]
fn the_daemon_request_is_resolved_once_and_sent_once_per_connection() {
    let directory = scratch_directory("daemons");
    let mut request = request_for(&directory, Some("{}"));
    let first = StubDaemon::new(1, || Ok(()));
    let second = StubDaemon::new(2, || Ok(()));
    // Two windows on one connection, and a third on its own.
    request.daemons = vec![first.clone(), first.clone(), second.clone()];
    let environment = StubEnvironment::default();

    let prepared = prepare(&request, &environment).unwrap();

    assert_eq!(environment.plan_resolutions.load(Ordering::SeqCst), 1);
    assert_eq!(first.requests.load(Ordering::SeqCst), 1);
    assert_eq!(second.requests.load(Ordering::SeqCst), 1);
    assert!(prepared.validate_for(Some(1), &[]).is_ok());
    assert!(prepared.validate_for(Some(2), &[]).is_ok());
}

#[cfg(feature = "zmux")]
#[test]
fn without_a_daemon_nothing_is_resolved() {
    let directory = scratch_directory("no-daemon");
    let request = request_for(&directory, Some("{}"));
    let environment = StubEnvironment::default();

    prepare(&request, &environment).unwrap();

    assert_eq!(environment.plan_resolutions.load(Ordering::SeqCst), 0);
}

#[cfg(feature = "zmux")]
#[test]
fn a_refused_daemon_fails_only_the_windows_on_it() {
    let directory = scratch_directory("refused");
    let mut request = request_for(&directory, Some("{}"));
    request.daemons = vec![
        StubDaemon::new(1, || anyhow::bail!("the daemon refused")),
        StubDaemon::new(2, || Ok(())),
    ];

    let prepared = prepare(&request, &StubEnvironment::default()).unwrap();

    let error = prepared.validate_for(Some(1), &[]).unwrap_err();
    assert!(error.contains("the daemon refused"), "{error}");
    assert!(prepared.validate_for(Some(2), &[]).is_ok());
    assert!(prepared.validate_for(None, &[]).is_ok());
}

#[cfg(feature = "zmux")]
#[test]
fn a_failed_recipient_resolution_fails_every_window_with_a_daemon() {
    let directory = scratch_directory("recipients");
    let mut request = request_for(&directory, Some("{}"));
    let daemon = StubDaemon::new(1, || Ok(()));
    request.daemons = vec![daemon.clone(), StubDaemon::new(2, || Ok(()))];
    let environment = StubEnvironment {
        plan_error: Some("invalid GitHub SSH key"),
        ..StubEnvironment::default()
    };

    let prepared = prepare(&request, &environment).unwrap();

    for identity in [1, 2] {
        let error = prepared.validate_for(Some(identity), &[]).unwrap_err();
        assert!(error.contains("invalid GitHub SSH key"), "{error}");
    }
    assert_eq!(daemon.requests.load(Ordering::SeqCst), 0);
    assert!(prepared.validate_for(None, &[]).is_ok());
}

/// Each daemon waits for the other to have been asked before answering, so a
/// preparation that sent the requests one after another would time out the
/// first and fail it.
#[cfg(feature = "zmux")]
#[test]
fn slow_daemons_are_reconfigured_in_parallel() {
    let directory = scratch_directory("parallel");
    let mut request = request_for(&directory, Some("{}"));
    let (first_asked, first_seen) = mpsc::channel::<()>();
    let (second_asked, second_seen) = mpsc::channel::<()>();
    let rendezvous = |asked: mpsc::Sender<()>, other: mpsc::Receiver<()>| {
        move || {
            let _ = asked.send(());
            other
                .recv_timeout(Duration::from_secs(5))
                .map_err(|_| anyhow::anyhow!("the other daemon was never asked"))
        }
    };
    request.daemons = vec![
        StubDaemon::new(1, rendezvous(first_asked, second_seen)),
        StubDaemon::new(2, rendezvous(second_asked, first_seen)),
    ];

    let prepared = prepare(&request, &StubEnvironment::default()).unwrap();

    assert!(prepared.validate_for(Some(1), &[]).is_ok());
    assert!(prepared.validate_for(Some(2), &[]).is_ok());
}
