//! The small part of the background-session protocol needed by a build without
//! the `zmux` feature.
//!
//! A no-`zmux` build still owns detached sessions in the Zetta process. These
//! types intentionally mirror the application-facing pieces of the shared
//! protocol so that authentication, catalog publication, and local reconnects
//! do not depend on the daemon crate being present.

pub(crate) mod auth {
    use std::{sync::Arc, time::Duration};

    use anyhow::{Context as _, Result};
    use argon2::{
        Argon2, PasswordHash, PasswordHasher as _, PasswordVerifier as _, password_hash::SaltString,
    };
    use subtle::ConstantTimeEq as _;
    use zeroize::Zeroizing;

    pub(crate) fn failed_authentication_delay(failures: u32) -> Duration {
        let doublings = failures.saturating_sub(1).min(u32::BITS - 1);
        Duration::from_secs(1)
            .saturating_mul(1_u32.checked_shl(doublings).unwrap_or(u32::MAX))
            .min(Duration::from_secs(30))
    }

    #[derive(Clone, Default, Eq)]
    pub(crate) struct SessionSecret(pub(crate) Zeroizing<String>);

    impl SessionSecret {
        pub(crate) fn new(secret: String) -> Self {
            Self(Zeroizing::new(secret))
        }

        pub(crate) fn expose(&self) -> &str {
            &self.0
        }
    }

    impl std::fmt::Debug for SessionSecret {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("SessionSecret(<redacted>)")
        }
    }

    impl PartialEq for SessionSecret {
        fn eq(&self, other: &Self) -> bool {
            self.0.as_bytes().ct_eq(other.0.as_bytes()).into()
        }
    }

    #[derive(Clone)]
    pub(crate) struct SessionAuthentication {
        verifier: Arc<str>,
        #[cfg(feature = "session-persistence")]
        key_envelope: Option<Arc<str>>,
    }

    #[derive(Clone)]
    pub(crate) struct VerifiedSession {
        verifier: Arc<str>,
    }

    impl SessionAuthentication {
        pub(crate) fn create(secret: &str) -> Result<Self> {
            anyhow::ensure!(
                !secret.is_empty(),
                "session authentication must not be empty"
            );
            let mut salt = [0; 16];
            getrandom::fill(&mut salt).context("generating session authentication salt")?;
            let salt = SaltString::encode_b64(&salt).map_err(|error| {
                anyhow::anyhow!("encoding session authentication salt: {error}")
            })?;
            let verifier = Argon2::default()
                .hash_password(secret.as_bytes(), &salt)
                .map_err(|error| anyhow::anyhow!("hashing session authentication: {error}"))?
                .to_string()
                .into();
            Ok(Self {
                verifier,
                #[cfg(feature = "session-persistence")]
                key_envelope: None,
            })
        }

        #[cfg(feature = "session-persistence")]
        pub(crate) fn with_key_envelope(mut self, envelope: impl Into<Arc<str>>) -> Self {
            self.key_envelope = Some(envelope.into());
            self
        }

        #[cfg(feature = "session-persistence")]
        pub(crate) fn key_envelope(&self) -> Option<&str> {
            self.key_envelope.as_deref()
        }

        pub(crate) fn verify(&self, secret: &str) -> Option<VerifiedSession> {
            PasswordHash::new(&self.verifier)
                .ok()
                .filter(|verifier| {
                    Argon2::default()
                        .verify_password(secret.as_bytes(), verifier)
                        .is_ok()
                })
                .map(|_| VerifiedSession {
                    verifier: self.verifier.clone(),
                })
        }

        pub(crate) fn authorizes(&self, authorization: &VerifiedSession) -> bool {
            Arc::ptr_eq(&self.verifier, &authorization.verifier)
        }
    }
}

pub(crate) mod protocol {
    use std::path::PathBuf;

    use serde::{Deserialize, Serialize};

    pub(crate) const DEFAULT_BACKGROUND_PANE_SPLIT_RATIO: u16 = 500;

    fn default_background_pane_split_ratio() -> u16 {
        DEFAULT_BACKGROUND_PANE_SPLIT_RATIO
    }

    pub(crate) const CATALOG_VERSION: u32 = 1;
    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub(crate) struct BackgroundSessionCatalog {
        pub(crate) version: u32,
        pub(crate) process_id: u32,
        pub(crate) runner_id: u64,
        pub(crate) sessions: Vec<BackgroundSessionSummary>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    pub(crate) enum BackgroundPaneLayout {
        Pane {
            pane_id: u64,
        },
        Split {
            axis: String,
            #[serde(default = "default_background_pane_split_ratio")]
            first_ratio: u16,
            first: Box<BackgroundPaneLayout>,
            second: Box<BackgroundPaneLayout>,
        },
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub(crate) struct BackgroundSessionSummary {
        pub(crate) id: u64,
        pub(crate) title: String,
        pub(crate) authentication_required: bool,
        pub(crate) active_pane: u64,
        pub(crate) layout: BackgroundPaneLayout,
        pub(crate) panes: Vec<BackgroundPaneSummary>,
        #[serde(default)]
        pub(crate) held: bool,
        #[serde(default)]
        pub(crate) scoped_to: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub(crate) key_envelope: Option<String>,
    }

    impl BackgroundSessionSummary {
        pub(crate) fn for_public_catalog(mut self) -> Self {
            if self.authentication_required {
                self.title = "Protected session".to_owned();
                self.active_pane = 0;
                self.layout = BackgroundPaneLayout::Pane { pane_id: 0 };
                self.panes.clear();
            }
            self
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub(crate) struct BackgroundPaneSummary {
        pub(crate) id: u64,
        pub(crate) label: String,
        pub(crate) profile: String,
        pub(crate) configured_command: String,
        pub(crate) application: String,
        pub(crate) foreground_command: Option<Vec<String>>,
        pub(crate) terminal_title: Option<String>,
        pub(crate) working_directory: Option<PathBuf>,
        pub(crate) state: BackgroundPaneState,
        #[serde(default)]
        pub(crate) exit: Option<BackgroundPaneExit>,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub(crate) enum BackgroundPaneState {
        Starting,
        Running,
        Exited,
        Failed,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub(crate) enum BackgroundPaneExitSource {
        Child,
        StatusUnavailable,
        WatcherDisconnected,
        BackendShutdown,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub(crate) enum BackgroundPaneExitReason {
        StatusUnavailable,
        WatcherDisconnected,
        BackendShutdown,
        ExitedBeforeInput,
        ForegroundCommand,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub(crate) struct BackgroundPaneExit {
        pub(crate) source: BackgroundPaneExitSource,
        pub(crate) reason: BackgroundPaneExitReason,
        pub(crate) exit_code: Option<i32>,
        pub(crate) child_pid: Option<u32>,
        pub(crate) input_sent: bool,
        pub(crate) foreground_is_shell: Option<bool>,
        pub(crate) foreground_command: Option<String>,
    }

    impl BackgroundPaneExit {
        pub(crate) fn foreground_command_is_publishable(command: &str) -> bool {
            !command.is_empty()
                && command.len() <= 64
                && command.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
                })
        }

        pub(crate) fn heading(&self) -> &'static str {
            match self.reason {
                BackgroundPaneExitReason::WatcherDisconnected => {
                    "Lost contact with the session multiplexer"
                }
                _ => "Terminal exited unexpectedly",
            }
        }

        pub(crate) fn reason_text(&self) -> String {
            let mut text = match self.reason {
                BackgroundPaneExitReason::StatusUnavailable => {
                    "the child exited but its exit status was unavailable".to_owned()
                }
                BackgroundPaneExitReason::WatcherDisconnected => {
                    "the session owner stopped reporting this terminal's process, so its exit status cannot be observed here"
                        .to_owned()
                }
                BackgroundPaneExitReason::BackendShutdown => {
                    "the terminal backend shut down unexpectedly".to_owned()
                }
                BackgroundPaneExitReason::ExitedBeforeInput => {
                    "the shell exited before receiving user input".to_owned()
                }
                BackgroundPaneExitReason::ForegroundCommand => self
                    .foreground_command
                    .as_deref()
                    .map_or_else(
                        || "the shell exited while a command was foreground".to_owned(),
                        |command| format!("the shell exited while {command:?} was foreground"),
                    ),
            };
            if let Some(code) = self.exit_code {
                text.push_str(&format!(" (exit code {code})"));
            }
            if let Some(pid) = self.child_pid {
                text.push_str(&format!(" [child PID {pid}]"));
            }
            text
        }
    }

    impl std::fmt::Display for BackgroundPaneState {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str(match self {
                Self::Starting => "starting",
                Self::Running => "running",
                Self::Exited => "exited",
                Self::Failed => "failed",
            })
        }
    }
}

pub(crate) mod catalog {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use anyhow::{Context as _, Result};

    use super::protocol::*;

    static NEXT_RUNNER_ID: AtomicU64 = AtomicU64::new(1);

    pub(crate) struct SessionCatalogPublisher {
        path: PathBuf,
        last_contents: Option<Vec<u8>>,
    }

    impl SessionCatalogPublisher {
        pub(crate) fn new(directory: &Path) -> Self {
            let runner_id = NEXT_RUNNER_ID.fetch_add(1, Ordering::Relaxed);
            Self::at_path(directory.join(format!("zetta-{}-{runner_id}.json", std::process::id())))
        }

        pub(crate) fn runner_id(&self) -> u64 {
            self.path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| stem.rsplit('-').next())
                .and_then(|value| value.parse().ok())
                .unwrap_or_default()
        }

        pub(crate) fn publish_sessions(
            &mut self,
            sessions: Vec<BackgroundSessionSummary>,
        ) -> Result<()> {
            let catalog = BackgroundSessionCatalog {
                version: CATALOG_VERSION,
                process_id: std::process::id(),
                runner_id: self.runner_id(),
                sessions: sessions
                    .into_iter()
                    .map(BackgroundSessionSummary::for_public_catalog)
                    .collect(),
            };
            self.publish(&catalog)
        }

        fn at_path(path: PathBuf) -> Self {
            Self {
                path,
                last_contents: None,
            }
        }

        fn publish(&mut self, catalog: &BackgroundSessionCatalog) -> Result<()> {
            if catalog.sessions.is_empty() {
                self.clear()?;
                return Ok(());
            }
            let contents =
                serde_json::to_vec_pretty(catalog).context("serializing session catalog")?;
            if self.last_contents.as_deref() == Some(contents.as_slice()) {
                return Ok(());
            }
            let parent = self
                .path
                .parent()
                .context("session catalog has no parent")?;
            create_private_dir(parent)?;
            let temporary = self.path.with_extension("json.tmp");
            write_private_file(&temporary, &contents)?;
            fs::rename(&temporary, &self.path)
                .with_context(|| format!("publishing session catalog {}", self.path.display()))?;
            self.last_contents = Some(contents);
            Ok(())
        }

        fn clear(&mut self) -> Result<()> {
            self.last_contents = None;
            match fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error)
                    .with_context(|| format!("removing session catalog {}", self.path.display())),
            }
        }
    }

    impl Drop for SessionCatalogPublisher {
        fn drop(&mut self) {
            let _ = self.clear();
        }
    }

    pub(crate) fn create_private_dir(path: &Path) -> Result<()> {
        fs::create_dir_all(path)
            .with_context(|| format!("creating session directory {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    fn write_private_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use std::io::Write as _;
            use std::os::unix::fs::OpenOptionsExt as _;
            let mut file = fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .mode(0o600)
                .open(path)?;
            file.write_all(contents)
        }
        #[cfg(not(unix))]
        fs::write(path, contents)
    }

    pub(crate) fn read_session_catalogs(directory: &Path) -> Result<Vec<BackgroundSessionCatalog>> {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("reading session catalogs in {}", directory.display())
                });
            }
        };
        let mut catalogs = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json")
                || !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("zetta-"))
            {
                continue;
            }
            let contents = fs::read(&path)?;
            if let Ok(catalog) = serde_json::from_slice::<BackgroundSessionCatalog>(&contents)
                && catalog.version == CATALOG_VERSION
            {
                catalogs.push(catalog);
            }
        }
        catalogs.sort_by_key(|catalog| (catalog.process_id, catalog.runner_id));
        Ok(catalogs)
    }

    pub(crate) fn application_from_command_line(command: Option<&[String]>) -> Option<String> {
        command.and_then(|arguments| {
            let executable = arguments.first()?;
            Some(
                executable
                    .rsplit(['/', '\\'])
                    .next()
                    .filter(|name| !name.is_empty())
                    .unwrap_or(executable)
                    .to_owned(),
            )
        })
    }
}

pub(crate) mod paths {
    use std::{env, path::PathBuf};

    pub(crate) fn platform_config_dir() -> PathBuf {
        #[cfg(windows)]
        if let Some(path) = env::var_os("APPDATA") {
            return PathBuf::from(path).join("Zetta");
        }
        #[cfg(not(windows))]
        if let Some(path) = env::var_os("XDG_CONFIG_HOME")
            && !path.is_empty()
        {
            return PathBuf::from(path).join("zetta");
        }
        #[cfg(not(windows))]
        if let Some(path) = env::var_os("HOME")
            && !path.is_empty()
        {
            return PathBuf::from(path).join(".config/zetta");
        }
        env::temp_dir().join("zetta")
    }

    pub(crate) fn session_catalog_dir() -> PathBuf {
        platform_config_dir().join("sessions")
    }
}
