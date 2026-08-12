use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result};
use argon2::{
    Argon2, PasswordHash, PasswordHasher as _, PasswordVerifier as _, password_hash::SaltString,
};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq as _;
use sysinfo::{Pid, ProcessesToUpdate, System};
use zeroize::Zeroizing;

static NEXT_RUNNER_ID: AtomicU64 = AtomicU64::new(1);
const CATALOG_VERSION: u32 = 3;

/// How long a session refuses reconnect attempts after one wrong secret, and
/// the ceiling that doubling reaches.
///
/// The window is enforced by *rejecting* early attempts rather than sleeping on
/// them. Sleeping would hold the process control thread, which answers one
/// request at a time, so a wrong secret could be used deliberately to stall
/// every other control command for the length of the backoff. Rejecting costs
/// an attacker exactly the same waiting time and costs everyone else nothing.
const FAILED_AUTHENTICATION_DELAY: Duration = Duration::from_secs(1);
const MAX_FAILED_AUTHENTICATION_DELAY: Duration = Duration::from_secs(30);

/// The refusal window after `failures` consecutive wrong secrets: doubling from
/// [`FAILED_AUTHENTICATION_DELAY`] up to [`MAX_FAILED_AUTHENTICATION_DELAY`].
///
/// Attempts serialize through the control socket, so this is a global bound on
/// the guessing rate for a session, not a per-connection one.
pub(crate) fn failed_authentication_delay(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(u32::BITS - 1);
    FAILED_AUTHENTICATION_DELAY
        .saturating_mul(1_u32.checked_shl(doublings).unwrap_or(u32::MAX))
        .min(MAX_FAILED_AUTHENTICATION_DELAY)
}

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
        create_private_dir(parent)?;
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

/// Creates a directory that only the current user may traverse. The session
/// directory holds the process control token and the session catalogs, so the
/// umask must not be allowed to widen it.
pub(crate) fn create_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .with_context(|| format!("creating session directory {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("restricting session directory {}", path.display()))?;
    }
    Ok(())
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
    failed_authentications: u32,
    refuse_until: Option<Instant>,
}

/// A session secret in transit between the CLI and the process that owns the
/// session. The inner value is zeroized on drop and never rendered by `Debug`,
/// so it cannot leak through a derived `Debug` on a containing message type.
#[derive(Clone, Default, Eq)]
pub(crate) struct SessionSecret(Zeroizing<String>);

impl SessionSecret {
    pub(crate) fn new(secret: String) -> Self {
        Self(Zeroizing::new(secret))
    }

    /// Takes ownership of an already-protected buffer. Used by the CLI prompt,
    /// which accumulates the typed secret in place: copying it out to call
    /// [`Self::new`] would leave the plaintext behind in freed memory.
    pub(crate) fn from_zeroizing(secret: Zeroizing<String>) -> Self {
        Self(secret)
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
}

/// Proof that a secret was checked against a session's verifier. It can only be
/// produced by [`SessionAuthentication::verify`], so a caller holding one has
/// necessarily authenticated rather than merely obtained a verifier clone.
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
        let salt = SaltString::encode_b64(&salt)
            .map_err(|error| anyhow::anyhow!("encoding session authentication salt: {error}"))?;
        let verifier = Argon2::default()
            .hash_password(secret.as_bytes(), &salt)
            .map_err(|error| anyhow::anyhow!("hashing session authentication: {error}"))?
            .to_string()
            .into();
        Ok(Self { verifier })
    }

    /// Checks `secret` against this verifier, returning proof of the check on
    /// success. Returning [`VerifiedSession`] rather than `bool` is what keeps
    /// authorization and authentication from drifting apart: the only way to
    /// obtain the value `take_background_session_by_id` demands is to pass a
    /// correct secret through here.
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

    /// Whether `authorization` was produced by verifying a secret against *this*
    /// session's verifier, rather than some other session's.
    pub(crate) fn authorizes(&self, authorization: &VerifiedSession) -> bool {
        Arc::ptr_eq(&self.verifier, &authorization.verifier)
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
            failed_authentications: 0,
            refuse_until: None,
        });
    }

    /// Whether this session is inside its backoff window and must refuse a
    /// reconnect attempt without evaluating the secret.
    pub(crate) fn authentication_is_refused_at(&self, index: usize) -> bool {
        self.sessions
            .get(index)
            .and_then(|session| session.refuse_until)
            .is_some_and(|until| Instant::now() < until)
    }

    /// Records a wrong secret and opens the next backoff window.
    ///
    /// Only called for attempts that were actually evaluated. Attempts already
    /// refused by the window do not extend it, so someone retrying too eagerly
    /// cannot drive their own lockout upward.
    pub(crate) fn record_failed_authentication_at(&mut self, index: usize) {
        if let Some(session) = self.sessions.get_mut(index) {
            session.failed_authentications = session.failed_authentications.saturating_add(1);
            session.refuse_until = Instant::now()
                .checked_add(failed_authentication_delay(session.failed_authentications));
        }
    }

    pub(crate) fn clear_failed_authentications_at(&mut self, index: usize) {
        if let Some(session) = self.sessions.get_mut(index) {
            session.failed_authentications = 0;
            session.refuse_until = None;
        }
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

    /// Sessions that reattach without a secret.
    ///
    /// Process control requests are authenticated only by the endpoint token,
    /// and that token sits in a file which every process running as this user
    /// can read. Anything reachable from the control socket must therefore go
    /// through these iterators rather than [`Self::iter`]/[`Self::iter_mut`]:
    /// otherwise the token alone would reveal that a protected session exists
    /// and let its state be modified, which is exactly what holding a secret is
    /// supposed to prevent.
    pub(crate) fn iter_unprotected(&self) -> impl Iterator<Item = &T> {
        self.sessions
            .iter()
            .filter(|session| session.authentication.is_none())
            .map(|session| &session.value)
    }

    pub(crate) fn iter_unprotected_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.sessions
            .iter_mut()
            .filter(|session| session.authentication.is_none())
            .map(|session| &mut session.value)
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
