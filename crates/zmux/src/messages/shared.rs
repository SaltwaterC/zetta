//! The shared-session half of the protocol: the state the daemon holds as the
//! truth for a shared tab, the operations a client asks it to apply, and the
//! pane drafts a shared spawn carries.

use super::*;
use crate::protocol::SplitAxis;

/// The monotonically increasing version of a daemon-owned shared session.
///
/// This is deliberately a newtype rather than a bare integer at call sites:
/// mixing a session revision with a pane or process ID made it too easy for a
/// client to submit an edit against the wrong snapshot.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct SessionRevision(pub u64);

impl SessionRevision {
    pub const INITIAL: Self = Self(0);

    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

/// An idempotency key for one client-issued shared-session mutation.
///
/// The client identity survives reconnects; the sequence is local to that
/// identity. Together they let a retry be recognized without treating a
/// second delivery as a second split, close, or rename.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedOperationId {
    pub client_id: ClientId,
    pub sequence: u64,
}

impl SharedOperationId {
    pub fn new(client_id: ClientId, sequence: u64) -> Self {
        Self {
            client_id,
            sequence,
        }
    }
}

/// The daemon's complete, canonical collaboration state for one session.
///
/// Pane IDs in `summary` are multiplexer IDs, never a Zetta window's local
/// entity IDs. `state` remains an opaque versioned tab payload: the daemon
/// persists and broadcasts it, while each Zetta process interprets it locally.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SharedSessionState {
    pub version: u32,
    pub session_id: u64,
    pub revision: SessionRevision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_operation_id: Option<SharedOperationId>,
    /// The most recent committed operation for each logical client. This is
    /// enough to make a lost-response retry idempotent without retaining an
    /// unbounded transaction log.
    #[serde(default)]
    pub operation_receipts: Vec<SharedOperationReceipt>,
    pub summary: BackgroundSessionSummary,
    /// Geometry and visibility that every viewer renders identically.
    ///
    /// Kept outside the opaque Zetta payload so the daemon can validate and
    /// rebase geometry operations without understanding application state.
    #[serde(default)]
    pub presentation: SharedPresentationState,
    pub state: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SharedSessionStateWire {
    pub(super) version: u32,
    pub(super) session_id: u64,
    pub(super) revision: SessionRevision,
    #[serde(default)]
    pub(super) last_operation_id: Option<SharedOperationId>,
    #[serde(default)]
    pub(super) operation_receipts: Vec<SharedOperationReceipt>,
    pub(super) summary: BackgroundSessionSummary,
    #[serde(default)]
    pub(super) presentation: SharedPresentationState,
    pub(super) state: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedOperationReceipt {
    pub operation_id: SharedOperationId,
    #[serde(default)]
    pub draft_mappings: Vec<SharedDraftMapping>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedPresentationState {
    pub layout: BackgroundPaneLayout,
    pub active_pane: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximized_pane: Option<u64>,
    #[serde(default)]
    pub minimized_panes: Vec<u64>,
}

impl Default for SharedPresentationState {
    fn default() -> Self {
        Self {
            layout: BackgroundPaneLayout::Pane { pane_id: 0 },
            active_pane: 0,
            maximized_pane: None,
            minimized_panes: Vec::new(),
        }
    }
}

impl SharedSessionState {
    pub fn new(
        session_id: u64,
        summary: BackgroundSessionSummary,
        state: serde_json::Value,
    ) -> Self {
        let presentation = SharedPresentationState {
            layout: summary.layout.clone(),
            active_pane: summary.active_pane,
            maximized_pane: None,
            minimized_panes: Vec::new(),
        };
        Self {
            version: SHARED_SESSION_STATE_VERSION,
            session_id,
            revision: SessionRevision::INITIAL,
            last_operation_id: None,
            operation_receipts: Vec::new(),
            summary,
            presentation,
            state,
        }
    }

    /// Validates the durable envelope's schema before interpreting its state.
    pub fn migrate(self) -> anyhow::Result<Self> {
        anyhow::ensure!(
            self.version == SHARED_SESSION_STATE_VERSION,
            "unsupported shared session state version {}",
            self.version
        );
        Ok(self)
    }

    /// Restores the presentation fields that are derived from the daemon's
    /// live summary.
    ///
    /// The summary is the source of truth for the panes a session currently
    /// owns. A handoff can update that summary while leaving the presentation
    /// from an older snapshot behind, so copying the layout and focus here is
    /// what makes an already-persisted session attachable again. Visibility
    /// references are also pruned because they are allowed to outlive a pane's
    /// removal. The opaque tab payload is deliberately not inspected or
    /// changed.
    ///
    /// Returns whether any presentation field changed. Callers that are
    /// repairing a persisted session use that result to advance its shared
    /// revision and publish the repaired snapshot.
    pub fn normalize(&mut self) -> anyhow::Result<bool> {
        validate_summary_panes(&self.summary)?;
        anyhow::ensure!(
            self.summary
                .panes
                .iter()
                .any(|pane| pane.id == self.summary.active_pane),
            "shared active pane {} is not in the live pane set",
            self.summary.active_pane
        );

        let mut changed = false;
        if self.presentation.layout != self.summary.layout {
            self.presentation.layout = self.summary.layout.clone();
            changed = true;
        }
        if self.presentation.active_pane != self.summary.active_pane {
            self.presentation.active_pane = self.summary.active_pane;
            changed = true;
        }

        let live_pane_ids = self
            .summary
            .panes
            .iter()
            .map(|pane| pane.id)
            .collect::<Vec<_>>();
        let before_minimized = self.presentation.minimized_panes.len();
        self.presentation
            .minimized_panes
            .retain(|pane_id| live_pane_ids.contains(pane_id));
        changed |= self.presentation.minimized_panes.len() != before_minimized;
        if self
            .presentation
            .maximized_pane
            .is_some_and(|pane_id| !live_pane_ids.contains(&pane_id))
        {
            self.presentation.maximized_pane = None;
            changed = true;
        }
        Ok(changed)
    }

    pub(super) fn sync_summary_presentation(&mut self) {
        self.summary.layout = self.presentation.layout.clone();
        self.summary.active_pane = self.presentation.active_pane;
    }

    pub fn pane_ids(&self) -> impl Iterator<Item = u64> + '_ {
        self.summary.panes.iter().map(|pane| pane.id)
    }

    pub fn contains_pane(&self, pane_id: u64) -> bool {
        self.summary.panes.iter().any(|pane| pane.id == pane_id)
    }

    pub fn validate_operation(&self, operation: &SharedSessionOperation) -> anyhow::Result<()> {
        match operation {
            SharedSessionOperation::ReplaceTab { summary, .. } => {
                anyhow::ensure!(
                    summary.id == self.session_id,
                    "shared operation targets session {}, expected {}",
                    summary.id,
                    self.session_id
                );
                validate_summary_panes(summary)?;
                let mut current = self.pane_ids().collect::<Vec<_>>();
                let mut replacement = summary.panes.iter().map(|pane| pane.id).collect::<Vec<_>>();
                current.sort_unstable();
                replacement.sort_unstable();
                anyhow::ensure!(
                    current == replacement,
                    "whole-tab state cannot add or remove live shared panes"
                );
            }
            SharedSessionOperation::SetLayout { layout } => {
                validate_layout_panes(layout, |pane_id| self.contains_pane(pane_id))?;
                let mut current = self.pane_ids().collect::<Vec<_>>();
                let mut replacement = Vec::new();
                zetta_layout::collect_pane_ids(layout, &mut replacement);
                current.sort_unstable();
                replacement.sort_unstable();
                anyhow::ensure!(
                    current == replacement,
                    "shared layout must contain every pane exactly once"
                );
            }
            SharedSessionOperation::SetFocus { pane_id }
            | SharedSessionOperation::SetMaximized {
                pane_id: Some(pane_id),
            }
            | SharedSessionOperation::SetMinimized { pane_id, .. } => {
                anyhow::ensure!(
                    self.contains_pane(*pane_id),
                    "shared operation targets missing pane {pane_id}"
                );
            }
            SharedSessionOperation::SetMaximized { pane_id: None } => {}
            SharedSessionOperation::SetSplitRatio {
                first_pane_id,
                second_pane_id,
                first_ratio,
            } => {
                anyhow::ensure!(
                    *first_ratio > 0
                        && *first_ratio < crate::protocol::BACKGROUND_PANE_SPLIT_RATIO_SCALE,
                    "invalid split ratio"
                );
                find_divider(&self.presentation.layout, *first_pane_id, *second_pane_id)
                    .context("shared divider no longer exists")?;
            }
            SharedSessionOperation::SwapPanes {
                first_pane_id,
                second_pane_id,
            } => {
                anyhow::ensure!(
                    first_pane_id != second_pane_id,
                    "cannot swap a pane with itself"
                );
                anyhow::ensure!(
                    self.contains_pane(*first_pane_id),
                    "shared operation targets missing pane {first_pane_id}"
                );
                anyhow::ensure!(
                    self.contains_pane(*second_pane_id),
                    "shared operation targets missing pane {second_pane_id}"
                );
            }
            SharedSessionOperation::MovePane { pane_id, direction } => {
                anyhow::ensure!(
                    self.contains_pane(*pane_id),
                    "shared operation targets missing pane {pane_id}"
                );
                anyhow::ensure!(
                    can_move_layout_pane(&self.presentation.layout, *pane_id, *direction),
                    "shared pane {pane_id} can no longer move {direction:?}"
                );
            }
            SharedSessionOperation::RotateSplit { pane_id, direction } => {
                anyhow::ensure!(
                    can_rotate_layout_pane(&self.presentation.layout, *pane_id, *direction),
                    "shared split for pane {pane_id} no longer exists"
                );
            }
            SharedSessionOperation::SetTabState { .. } => {}
            SharedSessionOperation::SetPaneMetadata { pane_id, .. } => {
                anyhow::ensure!(
                    self.contains_pane(*pane_id),
                    "shared operation targets missing pane {pane_id}"
                );
            }
            SharedSessionOperation::ClosePane { pane_id } => {
                anyhow::ensure!(
                    self.contains_pane(*pane_id),
                    "shared operation targets missing pane {pane_id}"
                );
                anyhow::ensure!(
                    self.summary.panes.len() > 1,
                    "the last shared pane cannot be closed without closing the session"
                );
            }
        }
        Ok(())
    }

    /// Applies an already-authorized operation and advances the revision.
    pub fn apply_operation(
        &mut self,
        operation_id: SharedOperationId,
        operation: &SharedSessionOperation,
    ) -> anyhow::Result<()> {
        self.validate_operation(operation)?;
        match operation {
            SharedSessionOperation::ReplaceTab { summary, state } => {
                self.summary = summary.clone();
                self.state = state.clone();
            }
            SharedSessionOperation::SetLayout { layout } => {
                self.presentation.layout = layout.clone();
            }
            SharedSessionOperation::SetFocus { pane_id } => {
                self.presentation.active_pane = *pane_id;
            }
            SharedSessionOperation::SetMaximized { pane_id } => {
                self.presentation.maximized_pane = *pane_id;
            }
            SharedSessionOperation::SetMinimized { pane_id, minimized } => {
                self.presentation.minimized_panes.retain(|id| id != pane_id);
                if *minimized {
                    self.presentation.minimized_panes.push(*pane_id);
                }
            }
            SharedSessionOperation::SetSplitRatio {
                first_pane_id,
                second_pane_id,
                first_ratio,
            } => {
                *find_divider_mut(
                    &mut self.presentation.layout,
                    *first_pane_id,
                    *second_pane_id,
                )
                .expect("validate_operation checked the divider") = *first_ratio;
            }
            SharedSessionOperation::SwapPanes {
                first_pane_id,
                second_pane_id,
            } => zetta_layout::swap_pane_ids(
                &mut self.presentation.layout,
                *first_pane_id,
                *second_pane_id,
            ),
            SharedSessionOperation::MovePane { pane_id, direction } => {
                let moved = move_layout_pane(&mut self.presentation.layout, *pane_id, *direction);
                debug_assert!(moved, "validate_operation checked the move");
            }
            SharedSessionOperation::RotateSplit { pane_id, direction } => {
                let rotated =
                    rotate_layout_pane(&mut self.presentation.layout, *pane_id, *direction);
                debug_assert!(rotated, "validate_operation checked the rotation");
            }
            SharedSessionOperation::SetTabState { state } => {
                self.state = state.clone();
            }
            SharedSessionOperation::SetPaneMetadata { pane_id, metadata } => {
                let pane = self
                    .summary
                    .panes
                    .iter_mut()
                    .find(|pane| pane.id == *pane_id)
                    .expect("validate_operation checked the pane");
                pane.label = metadata.label.clone();
                pane.profile = metadata.profile.clone();
                pane.configured_command = metadata.configured_command.clone();
                pane.application = metadata.application.clone();
                pane.foreground_command = metadata.foreground_command.clone();
                pane.terminal_title = metadata.terminal_title.clone();
                pane.working_directory = metadata.working_directory.clone();
            }
            SharedSessionOperation::ClosePane { pane_id } => {
                self.summary.panes.retain(|pane| pane.id != *pane_id);
                self.summary.layout = remove_pane_from_layout(&self.summary.layout, *pane_id)
                    .expect("a multi-pane layout remains valid after one pane is removed");
                if self.summary.active_pane == *pane_id {
                    self.presentation.active_pane = self.summary.panes[0].id;
                }
                self.presentation.layout =
                    remove_pane_from_layout(&self.presentation.layout, *pane_id)
                        .expect("a multi-pane layout remains valid after one pane is removed");
                self.presentation.minimized_panes.retain(|id| id != pane_id);
                if self.presentation.maximized_pane == Some(*pane_id) {
                    self.presentation.maximized_pane = None;
                }
            }
        }
        self.sync_summary_presentation();
        self.revision = self.revision.next();
        self.last_operation_id = Some(operation_id);
        self.record_receipt(SharedOperationReceipt {
            operation_id: self
                .last_operation_id
                .clone()
                .expect("operation id was set"),
            draft_mappings: Vec::new(),
        });
        Ok(())
    }

    pub fn receipt(&self, operation_id: &SharedOperationId) -> Option<&SharedOperationReceipt> {
        self.operation_receipts
            .iter()
            .find(|receipt| &receipt.operation_id == operation_id)
    }

    pub fn record_receipt(&mut self, receipt: SharedOperationReceipt) {
        self.operation_receipts
            .retain(|existing| existing.operation_id.client_id != receipt.operation_id.client_id);
        self.operation_receipts.push(receipt);
    }
}

/// A mutation against the canonical shared tab. Spawn is kept separate because
/// creating a pane also creates a daemon-owned PTY and a shared data plane.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum SharedSessionOperation {
    ReplaceTab {
        summary: BackgroundSessionSummary,
        state: serde_json::Value,
    },
    SetLayout {
        layout: BackgroundPaneLayout,
    },
    SetFocus {
        pane_id: u64,
    },
    SetMaximized {
        pane_id: Option<u64>,
    },
    SetMinimized {
        pane_id: u64,
        minimized: bool,
    },
    SetSplitRatio {
        first_pane_id: u64,
        second_pane_id: u64,
        first_ratio: u16,
    },
    MovePane {
        pane_id: u64,
        direction: SharedPaneDirection,
    },
    SwapPanes {
        first_pane_id: u64,
        second_pane_id: u64,
    },
    RotateSplit {
        pane_id: u64,
        direction: SharedRotationDirection,
    },
    SetTabState {
        state: serde_json::Value,
    },
    SetPaneMetadata {
        pane_id: u64,
        metadata: crate::protocol::BackgroundPaneSummary,
    },
    ClosePane {
        pane_id: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedPaneDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedRotationDirection {
    Clockwise,
    CounterClockwise,
}

impl From<SharedPaneDirection> for zetta_layout::PaneDirection {
    fn from(direction: SharedPaneDirection) -> Self {
        match direction {
            SharedPaneDirection::Left => Self::Left,
            SharedPaneDirection::Right => Self::Right,
            SharedPaneDirection::Up => Self::Up,
            SharedPaneDirection::Down => Self::Down,
        }
    }
}

impl From<SharedRotationDirection> for zetta_layout::PaneRotationDirection {
    fn from(direction: SharedRotationDirection) -> Self {
        match direction {
            SharedRotationDirection::Clockwise => Self::Clockwise,
            SharedRotationDirection::CounterClockwise => Self::CounterClockwise,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedSessionOperationRequest {
    pub session_id: u64,
    pub base_revision: SessionRevision,
    pub operation_id: SharedOperationId,
    pub operation: SharedSessionOperation,
}

/// The request body used to create a daemon-owned shared pane. It is separate
/// from [`SpawnRequest`] so a caller cannot accidentally ask the daemon to
/// return a descriptor for a remote/shared pane.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedSpawnRequest {
    pub session_id: u64,
    pub base_revision: SessionRevision,
    pub operation_id: SharedOperationId,
    pub program: Option<String>,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub working_directory: Option<PathBuf>,
    pub size: TerminalSize,
    pub console_palette: ConsolePalette,
    /// Omitted when using the default Windows console backend.
    #[serde(
        default,
        skip_serializing_if = "zetta_profiles::ConsoleBackend::is_default"
    )]
    pub console_backend: zetta_profiles::ConsoleBackend,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SharedDraftLayout {
    Existing {
        pane_id: u64,
    },
    Draft {
        draft_id: u64,
    },
    Split {
        axis: SplitAxis,
        first_ratio: u16,
        first: Box<SharedDraftLayout>,
        second: Box<SharedDraftLayout>,
    },
}

/// A pane a viewer has asked a shared session for.
///
/// It says which profile to run, not what that profile is. The daemon resolves
/// the name against its *own* host's configuration, because that is where the
/// process will run: a viewer on another machine resolved it against its own
/// `$SHELL` and shipped its whole environment along, which is how a Linux
/// client came to ask macOS for `/bin/bash` with a Linux `PATH` in front of it.
///
/// `env` is what only the requester can know — the pane's routing identity —
/// and never its machine's environment; see `SHARED_DRAFT_ENVIRONMENT_PREFIX`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedPaneDraft {
    pub draft_id: u64,
    /// The profile to run, by name. Empty, or a name this host does not have,
    /// means the host's login shell.
    pub profile: String,
    /// What that profile runs, when the requester is on this host and so
    /// resolved the name against the very configuration the daemon would read.
    /// It carries the working-directory tracking wrappers a profile can need,
    /// which a name alone cannot express. Never sent by a remote requester,
    /// whose resolution is its own machine's.
    #[serde(default)]
    pub command: Option<zetta_profiles::ProfileCommand>,
    pub env: HashMap<String, String>,
    pub working_directory: Option<PathBuf>,
    /// The pane to start in the same directory as, when `working_directory` is
    /// absent.
    ///
    /// A viewer knows a pane's directory only if that pane reports it, which
    /// needs shell integration — and a pane on another machine has none that
    /// answers to this viewer. The daemon is that pane's parent and can read it
    /// directly, so the requester names the pane instead of the path. Without
    /// this, a pane split from a remote one started wherever the daemon happened
    /// to be, which is `/`.
    #[serde(default)]
    pub inherit_working_directory_from: Option<u64>,
    /// Whether the daemon should load Zetta's shell integration into this pane
    /// before anybody can attach to it.
    ///
    /// The wrapper that loads it is delivered as if typed, so the terminal
    /// echoes it to *every* viewer of a shared pane. Done here, the pane has no
    /// attachment and nothing retained yet, so the exchange reaches nobody.
    /// Requesters that mean to load it themselves — a pane that is not shared —
    /// leave this false.
    #[serde(default)]
    pub load_shell_integration: bool,
    pub size: TerminalSize,
    pub console_palette: ConsolePalette,
    pub metadata: crate::protocol::BackgroundPaneSummary,
}

/// The only environment variables a draft may carry from the requester.
///
/// A pane's routing identity is the requesting window's to assign and means
/// nothing to the daemon, so it has to cross. Everything else about the
/// environment belongs to the host that runs the process.
pub const SHARED_DRAFT_ENVIRONMENT_PREFIX: &str = "ZETTA_";

/// Drops everything a requester must not decide for another machine.
pub fn shared_draft_environment(
    env: impl IntoIterator<Item = (String, String)>,
) -> HashMap<String, String> {
    env.into_iter()
        .filter(|(name, _)| name.starts_with(SHARED_DRAFT_ENVIRONMENT_PREFIX))
        .collect()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedSpawnBatchRequest {
    pub session_id: u64,
    pub base_revision: SessionRevision,
    pub operation_id: SharedOperationId,
    /// `None` replaces the complete layout and therefore requires an exact
    /// base revision. A pane target is rebased against the latest tree.
    pub target_pane_id: Option<u64>,
    pub replacement: SharedDraftLayout,
    pub panes: Vec<SharedPaneDraft>,
    pub active_pane: Option<SharedPaneRef>,
}

/// Atomically creates a daemon-owned shared session from a complete layout.
///
/// Unlike [`SharedSpawnBatchRequest`], this request has no existing session to
/// authorize or rebase against. The operation ID is retained in the canonical
/// shared state so a client may safely retry after losing the response.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSharedRequest {
    pub operation_id: SharedOperationId,
    pub title: String,
    pub replacement: SharedDraftLayout,
    pub panes: Vec<SharedPaneDraft>,
    pub active_pane: Option<SharedPaneRef>,
    /// An Argon2 verifier, never the plaintext secret. Empty means the new
    /// session is unprotected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verifier: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SharedPaneRef {
    Existing { pane_id: u64 },
    Draft { draft_id: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedDraftMapping {
    pub draft_id: u64,
    pub pane_id: u64,
}
