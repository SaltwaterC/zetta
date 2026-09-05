//! A window's side of background sessions: leaving one behind, taking one
//! back, and watching the panes that came with it.
//!
//! The root holds the state each half decides from — the free predicates the
//! transitions share, and the types their outcomes are reported as — so that a
//! rule such as "an older live state loses its theme overrides" has one home
//! rather than one per path. The halves are:
//!
//! - `detach.rs` — detaching, protecting, sharing, and storing a tab.
//! - `reconnect.rs` — taking a stored session back, and its authentication.
//! - `restore.rs` — rebuilding the tab a returned session becomes.
//! - `observers.rs` — what a window watches on a background pane.
//! - `multiplexer.rs` — handing a session to `zmux` and attaching one from it.
//! - `shared_panes.rs` — panes several windows watch at once.

use super::*;
use crate::mux::{MuxPaneIds, SharedPaneEntry};
use crate::project::resolve_registered_project_config_root;
use crate::rename::resolve_tab_title;
use crate::worktree_detection::terminal_event_requires_worktree_detection;

/// How often a detached session's panes are asked to re-read their foreground
/// process.
///
/// These panes are off screen: the answer feeds the reconnect picker and the
/// published catalog, not anything being drawn. Each poke also costs a catalog
/// publish when a title changes, so a short interval spends foreground wake-ups
/// to keep a list nobody is looking at a few seconds fresher. The terminal's own
/// refresh is throttled to `PROCESS_INFO_REFRESH_INTERVAL` and runs on the
/// background executor regardless, so this only sets how often it is offered the
/// chance.
const BACKGROUND_PROCESS_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub(crate) struct DiskResumeIdentities {
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) passphrases: Vec<Option<SessionSecret>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReconnectRequest {
    None,
    Immediate(usize),
    Choose,
}

fn reconnect_request(session_count: usize) -> ReconnectRequest {
    match session_count {
        0 => ReconnectRequest::None,
        1 => ReconnectRequest::Immediate(0),
        _ => ReconnectRequest::Choose,
    }
}

/// Whether a reconnect entry names a multiplexer session a tab in this window
/// is already showing.
///
/// Sharing a tab publishes its session while the tab stays on screen, so the
/// window that shared it would otherwise be offered its own session back. Taking
/// that offer is not a join: the multiplexer recognises the process that already
/// holds the pane and hands the terminal straight back, giving this window a
/// second tab reading the same pty as the first, with the two splitting its
/// output between them.
///
/// The runner check is what keeps this from hiding an unrelated session. A
/// session kept inside this process because the multiplexer was unreachable is
/// numbered from a different counter than the multiplexer's, so the two id spaces
/// can collide; an entry under this window's own runner is therefore never
/// hidden, and the reconnect path refuses the duplicate anyway.
fn session_is_already_shown_here(
    panes: &crate::mux::MuxPanes,
    (runner_id, session_id, _, _): &ProcessBackgroundSessionEntry,
    own_runner: u64,
) -> bool {
    *runner_id != own_runner && panes.holds_session(*session_id)
}

/// Copies one pane's project root onto every pane, and every stacked command, of
/// a tab that is arriving in this window.
///
/// Stacked entries are included because they are panes as far as the project
/// registry is concerned: each has its own id, its own view and its own theme.
fn inherit_project_for_panes(
    projects: &mut crate::project_context::ProjectState,
    source_pane_id: u64,
    tab: &Tab,
) {
    for pane in &tab.panes {
        projects.inherit_pane_root(source_pane_id, pane.id);
        for entry in &pane.stack.entries {
            projects.inherit_pane_root(source_pane_id, entry.id);
        }
    }
}

#[derive(Clone, Debug, Default)]
struct RestoredPaneMetadata {
    panes: HashMap<u64, RestoredPaneInfo>,
}

#[derive(Clone, Debug)]
struct RestoredPaneInfo {
    working_directory: Option<PathBuf>,
    project_root: Option<PathBuf>,
}

impl RestoredPaneMetadata {
    fn working_directory(&self, routing_id: u64) -> Option<PathBuf> {
        self.panes
            .get(&routing_id)
            .and_then(|pane| pane.working_directory.clone())
    }

    fn project_root(&self, routing_id: u64) -> Option<&PathBuf> {
        self.panes
            .get(&routing_id)
            .and_then(|pane| pane.project_root.as_ref())
    }
}

fn restored_pane_metadata(
    state: &crate::session_state::TabState,
    summary: &BackgroundSessionSummary,
) -> Vec<(u64, String, Option<PathBuf>)> {
    state
        .panes
        .iter()
        .flat_map(|pane| {
            let base = std::iter::once((
                pane.id,
                pane.profile.clone(),
                summary
                    .panes
                    .iter()
                    .find(|summary| summary.id == pane.id)
                    .and_then(|summary| summary.working_directory.clone()),
            ));
            let stacked = pane.stack.iter().map(|entry| {
                (
                    entry.id,
                    entry.profile.clone(),
                    entry
                        .working_directory
                        .clone()
                        .or_else(|| entry.wsl_directory.as_deref().map(PathBuf::from)),
                )
            });
            base.chain(stacked)
        })
        .collect()
}

fn restored_project_directory(profile: &Profile, directory: &Path) -> PathBuf {
    if is_wsl_shell(&profile.command)
        && let Some(directory) = directory.to_str()
        && let Some(native) = wsl_reported_directory(profile, directory)
    {
        return native;
    }
    directory.to_path_buf()
}

fn pane_theme_source_is_stale(
    source: Option<crate::session_state::PaneThemeSource>,
    process_id: u32,
    configuration_generation: u64,
) -> bool {
    source.is_some_and(|source| {
        source.process_id == process_id
            && source.configuration_generation != configuration_generation
    })
}

fn clear_session_theme_overrides(state: &mut crate::session_state::TabState) {
    state.theme_override = None;
    for pane in &mut state.panes {
        pane.theme_override = None;
        for entry in &mut pane.stack {
            entry.theme_override = None;
        }
    }
}

/// What to do with the size the multiplexer arbitrated for a shared pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SharedSizeAction {
    /// The viewer's own size is not known yet. Keep the arbitrated size pending
    /// rather than resizing against a guess.
    WaitForLayout,
    /// The viewer is already showing the pane at that size.
    AlreadyMatches,
    Resize,
}

/// Whether these bounds describe a pane that has been laid out, rather than the
/// placeholder a `TerminalContent` starts with.
///
/// The placeholder is 100 columns by 6 lines, and mistaking it for a real layout
/// has now caused two separate faults — a window resized to fit a size it already
/// had, and a joining window telling the multiplexer its pane was six rows tall,
/// which arbitrated *every* viewer down to six. Both sides of the size exchange ask
/// this, so they cannot disagree about what counts as known.
fn bounds_are_laid_out(bounds: terminal::TerminalBounds) -> bool {
    bounds != terminal::TerminalBounds::default()
}

/// The size to tell the multiplexer this viewer is showing a pane at, if it is
/// known yet.
///
/// `None` before the pane's first layout. Reporting the placeholder instead made a
/// window that had only just joined claim six rows, and since the pane must fit
/// inside every viewer, the window that had been showing it perfectly well was
/// resized down to match.
fn shared_size_to_report(bounds: terminal::TerminalBounds) -> Option<(u16, u16)> {
    bounds_are_laid_out(bounds).then(|| (bounds.num_columns() as u16, bounds.num_lines() as u16))
}

/// Decides whether an arbitrated size has to be imposed on this viewer.
///
/// The arbitrated size only needs *applying* to a viewer showing the pane at
/// some other size — the pty runs at the smallest of the viewers, so a larger one
/// has to shrink its grid or the shell's wrapping stops lining up with the cells
/// drawn. A viewer that already matches must not be touched: two windows tiled to
/// the same size by a compositor are the common case, and resizing one of them to
/// the size it already had moves the user's window for no reason.
///
/// The layout check is the other half of the same bug. A terminal reports the
/// placeholder bounds a `TerminalContent` starts with until its pane has been laid
/// out and synced once, and those are 100x6 — so a pane that was *already* the
/// arbitrated 98x51 looked like a two-column, forty-five-row difference, and the
/// window was resized to fit a size it already had. This ran before the first
/// paint and then reported success, so nothing ever corrected it.
fn shared_size_action(
    bounds: Option<terminal::TerminalBounds>,
    columns: u16,
    lines: u16,
) -> SharedSizeAction {
    let Some(bounds) = bounds else {
        return SharedSizeAction::WaitForLayout;
    };
    if !bounds_are_laid_out(bounds) {
        return SharedSizeAction::WaitForLayout;
    }
    if (bounds.num_columns(), bounds.num_lines()) == (columns as usize, lines as usize) {
        return SharedSizeAction::AlreadyMatches;
    }
    SharedSizeAction::Resize
}

fn remove_exited_background_pane(
    sessions: &mut BackgroundSessionRunner<Tab>,
    pane_id: u64,
) -> Option<Vec<u64>> {
    let session_index = sessions
        .iter()
        .position(|tab| tab.pane(pane_id).is_some())?;
    let pane_count = sessions.iter().nth(session_index)?.panes.len();
    if pane_count == 1 {
        let tab = sessions.reconnect_at(session_index)?;
        return Some(tab.panes.into_iter().map(|pane| pane.id).collect());
    }

    let tab = sessions.iter_mut().nth(session_index)?;
    let layout = tab.layout.clone().without(pane_id)?;
    tab.remove_pane(pane_id);
    tab.layout = layout;
    tab.restore_focus_after_close(pane_id, tab.layout.first_pane());
    Some(vec![pane_id])
}

/// An action that normally makes a tab's session reachable beyond this window.
///
/// In daemon mode each ends with the session attachable by something other than
/// the window driving it now — another window joining it, or a reconnect after
/// this one is gone — so each offers the secret that will gate that. Offered,
/// not required: an empty dialog leaves the session unprotected, which is what
/// detaching has always meant and therefore what all three mean. They share a
/// path — settle the secret, then act — and differ only in wording and in what
/// "act" means. `KeepRunning` remains process-local when `--no-mux` is active;
/// there is no multiplexer to make it reachable from another process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProtectedSessionAction {
    Detach,
    KeepRunning,
    Share,
}

impl ProtectedSessionAction {
    pub(crate) fn title(self, no_mux: bool) -> &'static str {
        match self {
            Self::Detach => "Detach session",
            Self::KeepRunning if no_mux => "Keep tab running after close",
            Self::KeepRunning => "Keep and share tab after close",
            Self::Share => "Share tab",
        }
    }

    pub(crate) fn description(self, no_mux: bool) -> &'static str {
        match self {
            Self::Detach => {
                "Leave both fields blank and press Enter to detach without authentication. \
                 Otherwise, enter and confirm a secret."
            }
            Self::KeepRunning if no_mux => {
                "Choose the authentication required when this tab is reattached. In --no-mux \
                 mode the session stays inside this Zetta process after the window closes and \
                 cannot be shared with another process. Press Enter with both fields empty for \
                 no authentication."
            }
            Self::KeepRunning => {
                "Choose the authentication required when this tab is reattached. This also makes \
                 the session available to another Zetta process after this window closes. Press \
                 Enter with both fields empty for no authentication."
            }
            Self::Share => {
                "Choose the authentication a window joining this tab has to present; it can \
                 then do everything this tab's terminals can already do. When the last viewer \
                 closes the tab or its window, the session continues running in the multiplexer. \
                 Press Enter with both fields empty for no authentication."
            }
        }
    }

    pub(crate) fn submit_label(self, no_mux: bool) -> &'static str {
        match self {
            Self::Detach => "Protect and detach",
            Self::KeepRunning if no_mux => "Protect and keep running",
            Self::KeepRunning => "Protect, keep, and share",
            Self::Share => "Protect, share, and keep",
        }
    }
}

/// What opening a multiplexer-held session's sealed key produced.
///
/// The passphrase case is a state, not an error, because it is answerable: the
/// window asks for the identity's passphrase and comes back. Collapsing it into
/// a failure is what made an encrypted SSH identity look like the wrong key.
#[cfg_attr(not(feature = "session-persistence"), allow(dead_code))]
pub(crate) enum SealedKeyRecovery {
    /// Not protected this way, or not something this window can open — the
    /// caller attaches without a secret and lets the daemon decide.
    NotSealed,
    /// The identity file is itself encrypted and no passphrase was supplied yet.
    NeedsIdentityPassphrase,
    Recovered(SessionSecret),
}

/// As [`SealedKeyRecovery`], for a session another Zetta process is holding,
/// where the proof rather than the secret is what the caller needs.
#[cfg_attr(not(feature = "session-persistence"), allow(dead_code))]
pub(crate) enum SealedKeyAuthorization {
    NotSealed,
    NeedsIdentityPassphrase,
    Authorized(VerifiedSession),
}

/// How much scrollback is handed over with a detached pane. Enough to restore a
/// screen and the context above it, without making detaching a tab an expensive
/// operation on a session that has been running for hours.
const SNAPSHOT_LINES: usize = 2_000;

mod detach;
mod image_paste;
mod multiplexer;
mod observers;
mod reconnect;
mod restore;
mod shared_panes;

pub(crate) use multiplexer::AttachOutcomeSummary;

#[cfg(test)]
#[path = "tests/background_session_ui.rs"]
mod tests;
