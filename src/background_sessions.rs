use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::{Context as _, Result};
use argon2::{
    Argon2, PasswordHash, PasswordHasher as _, PasswordVerifier as _, password_hash::SaltString,
};
use serde::{Deserialize, Serialize};
use sysinfo::{Pid, ProcessesToUpdate, System};

static NEXT_RUNNER_ID: AtomicU64 = AtomicU64::new(1);
const CATALOG_VERSION: u32 = 3;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BackgroundSessionCatalog {
    pub(crate) version: u32,
    pub(crate) process_id: u32,
    pub(crate) runner_id: u64,
    pub(crate) sessions: Vec<BackgroundSessionSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BackgroundSessionSummary {
    pub(crate) id: u64,
    pub(crate) title: String,
    pub(crate) authentication_required: bool,
    pub(crate) active_pane: u64,
    pub(crate) layout: BackgroundPaneLayout,
    pub(crate) panes: Vec<BackgroundPaneSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum BackgroundPaneLayout {
    Pane {
        pane_id: u64,
    },
    Split {
        axis: String,
        first: Box<BackgroundPaneLayout>,
        second: Box<BackgroundPaneLayout>,
    },
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BackgroundPaneState {
    Starting,
    Running,
    Exited,
    Failed,
}

impl std::fmt::Display for BackgroundPaneState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Exited => "exited",
            Self::Failed => "failed",
        };
        formatter.write_str(state)
    }
}

struct SessionCatalogPublisher {
    path: PathBuf,
    last_contents: Option<Vec<u8>>,
}

impl SessionCatalogPublisher {
    fn new() -> Self {
        let runner_id = NEXT_RUNNER_ID.fetch_add(1, Ordering::Relaxed);
        Self::at_path(
            session_catalog_dir().join(format!("zetta-{}-{runner_id}.json", std::process::id())),
        )
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
        let contents = serde_json::to_vec_pretty(catalog).context("serializing session catalog")?;
        if self.last_contents.as_deref() == Some(contents.as_slice()) {
            return Ok(());
        }
        let parent = self
            .path
            .parent()
            .context("session catalog has no parent")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("creating session catalog directory {}", parent.display()))?;
        let temporary = self.path.with_extension("json.tmp");
        write_private_file(&temporary, &contents)
            .with_context(|| format!("writing session catalog {}", temporary.display()))?;
        #[cfg(windows)]
        if self.path.exists() {
            fs::remove_file(&self.path)
                .with_context(|| format!("replacing session catalog {}", self.path.display()))?;
        }
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

fn write_private_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)?;
        use std::io::Write as _;
        file.write_all(contents)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    fs::write(path, contents)
}

impl Drop for SessionCatalogPublisher {
    fn drop(&mut self) {
        let _ = self.clear();
    }
}

/// Owns sessions that are not currently attached to a terminal view.
///
/// This deliberately has no GPUI or platform dependency. A future local daemon or
/// remote transport can own the same runner without also owning window state.
pub(crate) struct BackgroundSessionRunner<T> {
    sessions: Vec<DetachedSession<T>>,
    catalog: SessionCatalogPublisher,
}

struct DetachedSession<T> {
    value: T,
    authentication: Option<SessionAuthentication>,
}

#[derive(Clone)]
pub(crate) struct SessionAuthentication {
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
        let salt = SaltString::encode_b64(&salt)
            .map_err(|error| anyhow::anyhow!("encoding session authentication salt: {error}"))?;
        let verifier = Argon2::default()
            .hash_password(secret.as_bytes(), &salt)
            .map_err(|error| anyhow::anyhow!("hashing session authentication: {error}"))?
            .to_string()
            .into();
        Ok(Self { verifier })
    }

    pub(crate) fn verify(&self, secret: &str) -> bool {
        PasswordHash::new(&self.verifier)
            .ok()
            .is_some_and(|verifier| {
                Argon2::default()
                    .verify_password(secret.as_bytes(), &verifier)
                    .is_ok()
            })
    }

    pub(crate) fn is_same_verifier(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.verifier, &other.verifier)
    }

    #[cfg(test)]
    fn encoded(&self) -> &str {
        &self.verifier
    }
}

impl<T> Default for BackgroundSessionRunner<T> {
    fn default() -> Self {
        Self {
            sessions: Vec::new(),
            catalog: SessionCatalogPublisher::new(),
        }
    }
}

impl<T> BackgroundSessionRunner<T> {
    pub(crate) fn runner_id(&self) -> u64 {
        runner_id_from_path(&self.catalog.path).unwrap_or_default()
    }

    pub(crate) fn detach(&mut self, session: T, authentication: Option<SessionAuthentication>) {
        self.sessions.push(DetachedSession {
            value: session,
            authentication,
        });
    }

    pub(crate) fn reconnect_at(&mut self, index: usize) -> Option<T> {
        (index < self.sessions.len()).then(|| self.sessions.remove(index).value)
    }

    pub(crate) fn authentication_at(&self, index: usize) -> Option<&SessionAuthentication> {
        self.sessions.get(index)?.authentication.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.sessions.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = &T> {
        self.sessions.iter().map(|session| &session.value)
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.sessions.iter_mut().map(|session| &mut session.value)
    }

    pub(crate) fn publish(&mut self, sessions: Vec<BackgroundSessionSummary>) -> Result<()> {
        let catalog = BackgroundSessionCatalog {
            version: CATALOG_VERSION,
            process_id: std::process::id(),
            runner_id: runner_id_from_path(&self.catalog.path).unwrap_or_default(),
            sessions: sessions
                .into_iter()
                .map(BackgroundSessionSummary::for_public_catalog)
                .collect(),
        };
        self.catalog.publish(&catalog)
    }
}

impl BackgroundSessionSummary {
    fn for_public_catalog(mut self) -> Self {
        if self.authentication_required {
            self.title = "Protected session".to_owned();
            self.active_pane = 0;
            self.layout = BackgroundPaneLayout::Pane { pane_id: 0 };
            self.panes.clear();
        }
        self
    }
}

fn runner_id_from_path(path: &Path) -> Option<u64> {
    path.file_stem()?.to_str()?.rsplit('-').next()?.parse().ok()
}

pub(crate) fn session_catalog_dir() -> PathBuf {
    crate::config::platform_config_dir().join("sessions")
}

pub(crate) fn read_session_catalogs(directory: &Path) -> Result<Vec<BackgroundSessionCatalog>> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("reading session catalogs in {}", directory.display()));
        }
    };
    let mut catalogs = Vec::new();
    for entry in entries {
        let entry = entry.context("reading session catalog entry")?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json")
            || !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("zetta-"))
        {
            continue;
        }
        let contents = fs::read(&path)
            .with_context(|| format!("reading session catalog {}", path.display()))?;
        let catalog: BackgroundSessionCatalog = serde_json::from_slice(&contents)
            .with_context(|| format!("parsing session catalog {}", path.display()))?;
        if catalog.version == CATALOG_VERSION {
            catalogs.push((path, catalog));
        }
    }
    let process_ids = catalogs
        .iter()
        .map(|(_, catalog)| Pid::from_u32(catalog.process_id))
        .collect::<Vec<_>>();
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&process_ids), true);
    catalogs.retain(|(path, catalog)| {
        if system.process(Pid::from_u32(catalog.process_id)).is_some() {
            true
        } else {
            let _ = fs::remove_file(path);
            false
        }
    });
    let mut catalogs = catalogs
        .into_iter()
        .map(|(_, catalog)| catalog)
        .collect::<Vec<_>>();
    catalogs.sort_by_key(|catalog| (catalog.process_id, catalog.runner_id));
    Ok(catalogs)
}

pub(crate) fn print_session_catalogs(json: bool) -> Result<()> {
    let catalogs = read_session_catalogs(&session_catalog_dir())?;
    if json {
        println!("{}", serde_json::to_string_pretty(&catalogs)?);
        return Ok(());
    }
    let session_count = catalogs
        .iter()
        .map(|catalog| catalog.sessions.len())
        .sum::<usize>();
    if session_count == 0 {
        println!("No background sessions.");
        return Ok(());
    }
    println!(
        "{session_count} background session{}:",
        if session_count == 1 { "" } else { "s" }
    );
    for catalog in catalogs {
        for session in catalog.sessions {
            println!(
                "\n{}:{}:{}  {}  ({} pane{}{})",
                catalog.process_id,
                catalog.runner_id,
                session.id,
                display_text(&session.title),
                session.panes.len(),
                if session.panes.len() == 1 { "" } else { "s" },
                if session.authentication_required {
                    ", protected"
                } else {
                    ""
                }
            );
            if session.authentication_required {
                continue;
            }
            println!("  layout: {}", display_layout(&session.layout));
            for pane in session.panes {
                let active = if pane.id == session.active_pane {
                    " active"
                } else {
                    ""
                };
                println!(
                    "  pane {}{}  {}  [{}]",
                    pane.id,
                    active,
                    display_text(&pane.label),
                    pane.state
                );
                println!("    profile: {}", display_text(&pane.profile));
                println!("    configured: {}", display_text(&pane.configured_command));
                println!("    application: {}", display_text(&pane.application));
                if let Some(command) = pane.foreground_command {
                    println!("    command line: {}", display_command(&command));
                }
                if let Some(title) = pane.terminal_title {
                    println!("    title: {}", display_text(&title));
                }
                if let Some(directory) = pane.working_directory {
                    println!(
                        "    directory: {}",
                        display_text(&directory.to_string_lossy())
                    );
                }
            }
        }
    }
    Ok(())
}

fn display_text(text: &str) -> String {
    let mut display = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\n' => display.push_str("\\n"),
            '\r' => display.push_str("\\r"),
            '\t' => display.push_str("\\t"),
            character if character.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(display, "\\u{{{:x}}}", character as u32);
            }
            character => display.push(character),
        }
    }
    display
}

fn display_command(arguments: &[String]) -> String {
    arguments
        .iter()
        .map(|argument| {
            let argument = display_text(argument);
            if argument.is_empty()
                || argument
                    .chars()
                    .any(|character| character.is_whitespace() || matches!(character, '"' | '\''))
            {
                format!("{:?}", argument)
            } else {
                argument
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
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

fn display_layout(layout: &BackgroundPaneLayout) -> String {
    match layout {
        BackgroundPaneLayout::Pane { pane_id } => format!("pane:{pane_id}"),
        BackgroundPaneLayout::Split {
            axis,
            first,
            second,
        } => format!(
            "{axis}({}, {})",
            display_layout(first),
            display_layout(second)
        ),
    }
}

#[cfg(test)]
#[path = "tests/background_sessions.rs"]
mod tests;
