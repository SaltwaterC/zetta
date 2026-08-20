//! Publishing and reading the session catalog.
//!
//! The catalog is the one piece of session state that is readable without a
//! connection, so `zmux list` stays cheap. It lives in a directory only
//! the current user may traverse, is replaced atomically, and never contains a
//! verifier or a protected session's details.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{Context as _, Result};
use sysinfo::{Pid, ProcessesToUpdate, System};

use crate::protocol::{
    BackgroundPaneLayout, BackgroundSessionCatalog, BackgroundSessionSummary, CATALOG_VERSION,
};

static NEXT_RUNNER_ID: AtomicU64 = AtomicU64::new(1);

/// The stable identifier shared by `zmux` and `zetta mux reconnect`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionIdentifier {
    pub process_id: u32,
    pub runner_id: u64,
    pub session_id: u64,
}

impl std::fmt::Display for SessionIdentifier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}:{}:{}",
            self.process_id, self.runner_id, self.session_id
        )
    }
}

/// Parses the catalog identifier used by `zetta mux reconnect`.
pub fn parse_session_identifier(value: &str) -> Result<SessionIdentifier> {
    let mut parts = value.split(':');
    let process_id = parts
        .next()
        .filter(|part| !part.is_empty())
        .context("session ID must have the form PROCESS:RUNNER:SESSION")?
        .parse::<u32>()
        .context("session process ID must be a positive whole number")?;
    let runner_id = parts
        .next()
        .filter(|part| !part.is_empty())
        .context("session ID must have the form PROCESS:RUNNER:SESSION")?
        .parse::<u64>()
        .context("session runner ID must be a positive whole number")?;
    let session_id = parts
        .next()
        .filter(|part| !part.is_empty())
        .context("session ID must have the form PROCESS:RUNNER:SESSION")?
        .parse::<u64>()
        .context("session ID must be a positive whole number")?;
    anyhow::ensure!(
        parts.next().is_none(),
        "session ID must have the form PROCESS:RUNNER:SESSION"
    );
    anyhow::ensure!(
        process_id > 0 && runner_id > 0 && session_id > 0,
        "session ID components must be positive whole numbers"
    );
    Ok(SessionIdentifier {
        process_id,
        runner_id,
        session_id,
    })
}

pub struct SessionCatalogPublisher {
    pub(crate) path: PathBuf,
    last_contents: Option<Vec<u8>>,
}

impl SessionCatalogPublisher {
    /// Publishes into `directory` under a name unique to this process and
    /// runner, so several runners in one process cannot overwrite each other.
    pub fn new(directory: &Path) -> Self {
        let runner_id = NEXT_RUNNER_ID.fetch_add(1, Ordering::Relaxed);
        Self::with_generation(directory, runner_id)
    }

    /// Creates a publisher whose generation is supplied by a daemon. Unlike
    /// the in-process Zetta runner, a daemon must carry this value across an
    /// exec so a reconnect identifier does not change while the daemon is
    /// being upgraded.
    pub fn with_generation(directory: &Path, generation: u64) -> Self {
        Self::at_path(directory.join(format!("zetta-{}-{generation}.json", std::process::id())))
    }

    pub fn at_path(path: PathBuf) -> Self {
        Self {
            path,
            last_contents: None,
        }
    }

    pub fn runner_id(&self) -> u64 {
        runner_id_from_path(&self.path).unwrap_or_default()
    }

    /// Publishes `sessions` under the current schema version. Protected
    /// sessions are reduced to an ID and a flag on the way out.
    pub fn publish_sessions(&mut self, sessions: Vec<BackgroundSessionSummary>) -> Result<()> {
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

    pub fn publish(&mut self, catalog: &BackgroundSessionCatalog) -> Result<()> {
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

    pub fn clear(&mut self) -> Result<()> {
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

/// Creates a directory that only the current user may traverse. The session
/// directory holds the process control token and the session catalogs, so the
/// umask must not be allowed to widen it.
pub fn create_private_dir(path: &Path) -> Result<()> {
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

pub fn write_private_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
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

fn runner_id_from_path(path: &Path) -> Option<u64> {
    path.file_stem()?.to_str()?.rsplit('-').next()?.parse().ok()
}

pub fn read_session_catalogs(directory: &Path) -> Result<Vec<BackgroundSessionCatalog>> {
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

pub fn print_session_catalogs(directory: &Path, json: bool) -> Result<()> {
    let catalogs = read_session_catalogs(directory)?;
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
    let unambiguous_session_ids = unambiguous_session_ids(&catalogs);
    for catalog in catalogs {
        for session in catalog.sessions {
            let identifier = SessionIdentifier {
                process_id: catalog.process_id,
                runner_id: catalog.runner_id,
                session_id: session.id,
            };
            // Keep the short number first for a readable administrative list;
            // the stable composite identifier used by `zmux reconnect` is
            // printed immediately below and is what completion offers for
            // every command that takes a session.
            println!(
                "\nsession {}  {}  ({} pane{}{}{})",
                session.id,
                display_text(&session.title),
                session.panes.len(),
                if session.panes.len() == 1 { "" } else { "s" },
                if session.authentication_required {
                    ", protected"
                } else {
                    ""
                },
                // Worth saying, because attaching a session somebody is looking
                // at behaves differently: the multiplexer asks that viewer to
                // hand the terminal over, and from then on both see the same
                // panes. Listing it identically to a detached session made a
                // shared attach look like a plain reconnect.
                if session.held { ", in use" } else { "" }
            );
            // Which window's it is. A scoped session is listed here — this is
            // the administrative view, and hiding what cannot be attached would
            // make `zmux kill` impossible to aim — but it is not offered to
            // another process's picker, so saying so is the difference between
            // "missing" and "not yours".
            if let Some(process_id) = session.scoped_to {
                let instructions =
                    scoped_session_instructions(identifier, &unambiguous_session_ids);
                println!("  scoped to process {process_id}  ({instructions})");
            }
            println!(
                "  reconnect id: {}",
                display_session_identifier(identifier, &unambiguous_session_ids)
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
                if let Some(exit) = pane.exit {
                    println!("    exit: {}", display_text(&exit.reason_text()));
                }
            }
        }
    }
    Ok(())
}

/// Returns the numeric IDs that can safely be used as shorthand. A session ID
/// is allocated by a multiplexer, but the catalog can contain more than one
/// live multiplexer generation, so the number is not globally unique here.
fn unambiguous_session_ids(catalogs: &[BackgroundSessionCatalog]) -> HashSet<u64> {
    let mut counts = HashMap::new();
    for catalog in catalogs {
        for session in &catalog.sessions {
            *counts.entry(session.id).or_insert(0_usize) += 1;
        }
    }
    counts
        .into_iter()
        .filter_map(|(session_id, count)| (count == 1).then_some(session_id))
        .collect()
}

fn display_session_identifier(
    identifier: SessionIdentifier,
    unambiguous_session_ids: &HashSet<u64>,
) -> String {
    if unambiguous_session_ids.contains(&identifier.session_id) {
        format!("{identifier} (short: {})", identifier.session_id)
    } else {
        identifier.to_string()
    }
}

fn scoped_session_instructions(
    identifier: SessionIdentifier,
    unambiguous_session_ids: &HashSet<u64>,
) -> String {
    let session_id = if unambiguous_session_ids.contains(&identifier.session_id) {
        identifier.session_id.to_string()
    } else {
        identifier.to_string()
    };
    format!(
        "run `zmux share {session_id}` to make it shared, then `zmux reconnect {session_id}` to open it"
    )
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

pub fn application_from_command_line(command: Option<&[String]>) -> Option<String> {
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
#[path = "tests/catalog.rs"]
mod tests;
