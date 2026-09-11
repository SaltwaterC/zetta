//! The client/daemon wire protocol.
//!
//! Deliberately transport-agnostic: nothing here depends on descriptor passing
//! or on peer credentials, so the same messages can later carry a session over
//! a remote transport. Descriptor passing is an optimisation the local
//! transport applies to [`Response::Spawned`] and [`Response::Attached`].

use std::{collections::HashMap, path::PathBuf};

use alacritty_terminal::tty::ConsolePalette;
use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use crate::protocol::{BackgroundPaneLayout, BackgroundSessionSummary, RestorableSessionRecord};

/// The wire format, and what a client and a multiplexer compare before they
/// trust each other to understand one another.
pub const PROTOCOL_VERSION: u32 = 5;

/// Version of the durable collaboration envelope. This is independent from
/// [`PROTOCOL_VERSION`]: a daemon upgrade may keep a session state produced by
/// an older binary even when the live wire protocol has moved on.
pub const SHARED_SESSION_STATE_VERSION: u32 = 2;

/// Maximum encoded image size accepted by the image-paste request.
pub const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;

/// Every request carries the endpoint token, which authenticates the *channel*
/// only. It says nothing about whether a protected session may be attached —
/// that needs the session's own secret, checked against its verifier.
/// Deliberately tolerant of unknown fields, unlike everything it carries.
///
/// The version is inside the message, so refusing to parse a message that has
/// an unfamiliar field means never getting far enough to report the mismatch.
/// A client built against a newer protocol must get a version error, not a
/// closed connection.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Envelope {
    pub version: u32,
    pub token: String,
    /// The client's own process.
    ///
    /// Two jobs, both on every platform. It is where a terminal is duplicated
    /// *to* where the platform cannot attach one to a message — on Unix the
    /// descriptor travels with the reply instead. And it is the client's
    /// *identity*: which client holds a pane, so a pane can be reclaimed when
    /// that client dies, a revoke can be addressed to the one holder, and
    /// releasing or detaching a pane can be refused to a client that is not
    /// holding it.
    #[serde(default)]
    pub client_process_id: u32,
    /// A stable identity for this logical client. Unlike the process ID it is
    /// unique across clients using one process, and survives reconnects after
    /// a daemon replacement or SSH forward restart.
    #[serde(default)]
    pub client_id: ClientId,
    /// Remote clients cannot receive descriptors or participate in exclusive
    /// handover. They attach through the shared byte-stream path only.
    #[serde(default)]
    pub stream_only: bool,
    /// Authentication for stream-safe administrative requests. Attach keeps
    /// its credential in [`Request::Attach`] because it is the first request
    /// on the long-lived shared data connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_secret: Option<String>,
    pub request: Request,
}

/// The logical identity of one mux client.
///
/// Process IDs are intentionally not enough: two panes opened through one
/// remote Zetta process use the same process ID, and the SSH server may report
/// the same peer process for every forwarded connection. A random ID gives
/// shared-client routing and upgrade handover an unambiguous key without
/// changing local PID ownership and liveness semantics.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClientId(String);

impl ClientId {
    pub fn random() -> anyhow::Result<Self> {
        Ok(Self(crate::transport::random_hex(16)?))
    }

    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

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
struct SharedSessionStateWire {
    version: u32,
    session_id: u64,
    revision: SessionRevision,
    #[serde(default)]
    last_operation_id: Option<SharedOperationId>,
    #[serde(default)]
    operation_receipts: Vec<SharedOperationReceipt>,
    summary: BackgroundSessionSummary,
    #[serde(default)]
    presentation: SharedPresentationState,
    state: serde_json::Value,
}

impl<'de> Deserialize<'de> for SharedSessionState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;
        let wire = SharedSessionStateWire::deserialize(deserializer)?;
        SharedSessionState {
            version: wire.version,
            session_id: wire.session_id,
            revision: wire.revision,
            last_operation_id: wire.last_operation_id,
            operation_receipts: wire.operation_receipts,
            summary: wire.summary,
            presentation: wire.presentation,
            state: wire.state,
        }
        .migrate()
        .map_err(D::Error::custom)
    }
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

    /// Upgrades the durable v1 envelope without touching its live panes.
    pub fn migrate(mut self) -> anyhow::Result<Self> {
        match self.version {
            SHARED_SESSION_STATE_VERSION => {}
            1 => {
                self.presentation = SharedPresentationState {
                    layout: self.summary.layout.clone(),
                    active_pane: self.summary.active_pane,
                    maximized_pane: None,
                    minimized_panes: Vec::new(),
                };
                self.version = SHARED_SESSION_STATE_VERSION;
            }
            version => anyhow::bail!("unsupported shared session state version {version}"),
        }
        self.sync_summary_presentation();
        Ok(self)
    }

    fn sync_summary_presentation(&mut self) {
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
                collect_layout_pane_ids(layout, &mut replacement);
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
            } => swap_layout_panes(
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
        axis: String,
        first_ratio: u16,
        first: Box<SharedDraftLayout>,
        second: Box<SharedDraftLayout>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedPaneDraft {
    pub draft_id: u64,
    pub program: Option<String>,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub working_directory: Option<PathBuf>,
    pub size: TerminalSize,
    pub console_palette: ConsolePalette,
    pub metadata: crate::protocol::BackgroundPaneSummary,
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

fn validate_summary_panes(summary: &BackgroundSessionSummary) -> anyhow::Result<()> {
    anyhow::ensure!(
        summary.panes.iter().all(|pane| pane.id != 0),
        "shared pane IDs must be positive multiplexer IDs"
    );
    let mut pane_ids = summary.panes.iter().map(|pane| pane.id).collect::<Vec<_>>();
    pane_ids.sort_unstable();
    anyhow::ensure!(
        pane_ids.windows(2).all(|pair| pair[0] != pair[1]),
        "shared pane IDs must be unique"
    );
    validate_layout_panes(&summary.layout, |pane_id| {
        summary.panes.iter().any(|pane| pane.id == pane_id)
    })?;
    let mut layout_ids = Vec::new();
    collect_layout_pane_ids(&summary.layout, &mut layout_ids);
    layout_ids.sort_unstable();
    anyhow::ensure!(
        pane_ids == layout_ids,
        "shared layout must contain every pane exactly once"
    );
    Ok(())
}

fn validate_layout_panes(
    layout: &BackgroundPaneLayout,
    contains: impl Fn(u64) -> bool + Copy,
) -> anyhow::Result<()> {
    match layout {
        BackgroundPaneLayout::Pane { pane_id } => {
            anyhow::ensure!(
                contains(*pane_id),
                "shared layout references missing pane {pane_id}"
            );
            Ok(())
        }
        BackgroundPaneLayout::Split {
            axis,
            first_ratio,
            first,
            second,
        } => {
            anyhow::ensure!(
                axis == "horizontal" || axis == "vertical",
                "invalid shared split axis {axis}"
            );
            anyhow::ensure!(
                *first_ratio > 0
                    && *first_ratio < crate::protocol::BACKGROUND_PANE_SPLIT_RATIO_SCALE,
                "invalid shared split ratio"
            );
            validate_layout_panes(first, contains)?;
            validate_layout_panes(second, contains)
        }
    }
}

fn collect_layout_pane_ids(layout: &BackgroundPaneLayout, ids: &mut Vec<u64>) {
    match layout {
        BackgroundPaneLayout::Pane { pane_id } => ids.push(*pane_id),
        BackgroundPaneLayout::Split { first, second, .. } => {
            collect_layout_pane_ids(first, ids);
            collect_layout_pane_ids(second, ids);
        }
    }
}

fn remove_pane_from_layout(
    layout: &BackgroundPaneLayout,
    pane_id: u64,
) -> Option<BackgroundPaneLayout> {
    match layout {
        BackgroundPaneLayout::Pane { pane_id: id } if *id == pane_id => None,
        BackgroundPaneLayout::Pane { .. } => Some(layout.clone()),
        BackgroundPaneLayout::Split {
            first,
            second,
            axis,
            first_ratio,
        } => {
            let first = remove_pane_from_layout(first, pane_id);
            let second = remove_pane_from_layout(second, pane_id);
            match (first, second) {
                (Some(first), Some(second)) => Some(BackgroundPaneLayout::Split {
                    axis: axis.clone(),
                    first_ratio: *first_ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(remaining), None) | (None, Some(remaining)) => Some(remaining),
                (None, None) => None,
            }
        }
    }
}

fn layout_contains(layout: &BackgroundPaneLayout, pane_id: u64) -> bool {
    match layout {
        BackgroundPaneLayout::Pane { pane_id: id } => *id == pane_id,
        BackgroundPaneLayout::Split { first, second, .. } => {
            layout_contains(first, pane_id) || layout_contains(second, pane_id)
        }
    }
}

fn find_divider(
    layout: &BackgroundPaneLayout,
    first_pane_id: u64,
    second_pane_id: u64,
) -> Option<u16> {
    match layout {
        BackgroundPaneLayout::Pane { .. } => None,
        BackgroundPaneLayout::Split {
            first_ratio,
            first,
            second,
            ..
        } => {
            if layout_contains(first, first_pane_id) && layout_contains(second, second_pane_id) {
                Some(*first_ratio)
            } else {
                find_divider(first, first_pane_id, second_pane_id)
                    .or_else(|| find_divider(second, first_pane_id, second_pane_id))
            }
        }
    }
}

fn find_divider_mut(
    layout: &mut BackgroundPaneLayout,
    first_pane_id: u64,
    second_pane_id: u64,
) -> Option<&mut u16> {
    match layout {
        BackgroundPaneLayout::Pane { .. } => None,
        BackgroundPaneLayout::Split {
            first_ratio,
            first,
            second,
            ..
        } => {
            if layout_contains(first, first_pane_id) && layout_contains(second, second_pane_id) {
                Some(first_ratio)
            } else if layout_contains(first, first_pane_id)
                || layout_contains(first, second_pane_id)
            {
                find_divider_mut(first, first_pane_id, second_pane_id)
            } else {
                find_divider_mut(second, first_pane_id, second_pane_id)
            }
        }
    }
}

fn swap_layout_panes(layout: &mut BackgroundPaneLayout, first_id: u64, second_id: u64) {
    match layout {
        BackgroundPaneLayout::Pane { pane_id } if *pane_id == first_id => *pane_id = second_id,
        BackgroundPaneLayout::Pane { pane_id } if *pane_id == second_id => *pane_id = first_id,
        BackgroundPaneLayout::Pane { .. } => {}
        BackgroundPaneLayout::Split { first, second, .. } => {
            swap_layout_panes(first, first_id, second_id);
            swap_layout_panes(second, first_id, second_id);
        }
    }
}

pub fn move_layout_pane(
    layout: &mut BackgroundPaneLayout,
    pane_id: u64,
    direction: SharedPaneDirection,
) -> bool {
    let (axis, toward_first) = match direction {
        SharedPaneDirection::Left => ("vertical", true),
        SharedPaneDirection::Right => ("vertical", false),
        SharedPaneDirection::Up => ("horizontal", true),
        SharedPaneDirection::Down => ("horizontal", false),
    };
    move_layout_pane_inner(layout, pane_id, axis, toward_first).unwrap_or(false)
}

fn can_move_layout_pane(
    layout: &BackgroundPaneLayout,
    pane_id: u64,
    direction: SharedPaneDirection,
) -> bool {
    move_layout_pane(&mut layout.clone(), pane_id, direction)
}

fn move_layout_pane_inner(
    layout: &mut BackgroundPaneLayout,
    pane_id: u64,
    axis: &str,
    toward_first: bool,
) -> Option<bool> {
    let BackgroundPaneLayout::Split {
        axis: split_axis,
        first_ratio,
        first,
        second,
    } = layout
    else {
        return None;
    };
    if layout_contains(first, pane_id) {
        if let Some(handled) = move_layout_pane_inner(first, pane_id, axis, toward_first) {
            return Some(handled);
        }
        (split_axis == axis && !toward_first).then(|| {
            std::mem::swap(first, second);
            *first_ratio = crate::protocol::BACKGROUND_PANE_SPLIT_RATIO_SCALE - *first_ratio;
            true
        })
    } else if layout_contains(second, pane_id) {
        if let Some(handled) = move_layout_pane_inner(second, pane_id, axis, toward_first) {
            return Some(handled);
        }
        (split_axis == axis && toward_first).then(|| {
            std::mem::swap(first, second);
            *first_ratio = crate::protocol::BACKGROUND_PANE_SPLIT_RATIO_SCALE - *first_ratio;
            true
        })
    } else {
        None
    }
}

pub fn rotate_layout_pane(
    layout: &mut BackgroundPaneLayout,
    pane_id: u64,
    direction: SharedRotationDirection,
) -> bool {
    let Some(active_area) = layout_pane_area(layout, pane_id, 1.) else {
        return false;
    };
    let Some(path) = rotation_target(layout, pane_id, active_area, 1.) else {
        return false;
    };
    rotate_at_path(layout, &path, direction);
    true
}

fn can_rotate_layout_pane(
    layout: &BackgroundPaneLayout,
    pane_id: u64,
    direction: SharedRotationDirection,
) -> bool {
    rotate_layout_pane(&mut layout.clone(), pane_id, direction)
}

fn layout_pane_area(layout: &BackgroundPaneLayout, pane_id: u64, area: f64) -> Option<f64> {
    match layout {
        BackgroundPaneLayout::Pane { pane_id: id } => (*id == pane_id).then_some(area),
        BackgroundPaneLayout::Split {
            first_ratio,
            first,
            second,
            ..
        } => {
            let first_area = area * f64::from(*first_ratio)
                / f64::from(crate::protocol::BACKGROUND_PANE_SPLIT_RATIO_SCALE);
            layout_pane_area(first, pane_id, first_area)
                .or_else(|| layout_pane_area(second, pane_id, area - first_area))
        }
    }
}

fn rotation_target(
    layout: &BackgroundPaneLayout,
    pane_id: u64,
    active_area: f64,
    area: f64,
) -> Option<Vec<bool>> {
    if has_four_equal_panes(layout, area) || is_two_pane_split(layout) {
        return Some(Vec::new());
    }
    let BackgroundPaneLayout::Split {
        first_ratio,
        first,
        second,
        ..
    } = layout
    else {
        return None;
    };
    let first_area = area * f64::from(*first_ratio)
        / f64::from(crate::protocol::BACKGROUND_PANE_SPLIT_RATIO_SCALE);
    let (child, child_area, sibling_area, child_is_first) = if layout_contains(first, pane_id) {
        (first, first_area, area - first_area, true)
    } else if layout_contains(second, pane_id) {
        (second, area - first_area, first_area, false)
    } else {
        return None;
    };
    if let Some(mut path) = rotation_target(child, pane_id, active_area, child_area) {
        path.insert(0, child_is_first);
        return Some(path);
    }
    (active_area + 1e-9 >= sibling_area).then_some(Vec::new())
}

fn is_two_pane_split(layout: &BackgroundPaneLayout) -> bool {
    matches!(
        layout,
        BackgroundPaneLayout::Split { first, second, .. }
            if matches!(first.as_ref(), BackgroundPaneLayout::Pane { .. })
                && matches!(second.as_ref(), BackgroundPaneLayout::Pane { .. })
    )
}

fn has_four_equal_panes(layout: &BackgroundPaneLayout, area: f64) -> bool {
    let mut areas = Vec::with_capacity(4);
    collect_leaf_areas(layout, area, &mut areas);
    areas.len() == 4
        && areas
            .iter()
            .all(|candidate| (*candidate - areas[0]).abs() <= 1e-9)
}

fn collect_leaf_areas(layout: &BackgroundPaneLayout, area: f64, areas: &mut Vec<f64>) {
    match layout {
        BackgroundPaneLayout::Pane { .. } => areas.push(area),
        BackgroundPaneLayout::Split {
            first_ratio,
            first,
            second,
            ..
        } => {
            let first_area = area * f64::from(*first_ratio)
                / f64::from(crate::protocol::BACKGROUND_PANE_SPLIT_RATIO_SCALE);
            collect_leaf_areas(first, first_area, areas);
            collect_leaf_areas(second, area - first_area, areas);
        }
    }
}

fn rotate_at_path(
    layout: &mut BackgroundPaneLayout,
    path: &[bool],
    direction: SharedRotationDirection,
) {
    if let Some((first_path, remaining)) = path.split_first() {
        if let BackgroundPaneLayout::Split { first, second, .. } = layout {
            rotate_at_path(
                if *first_path { first } else { second },
                remaining,
                direction,
            );
        }
        return;
    }
    rotate_layout_geometry(layout, direction);
}

fn rotate_layout_geometry(layout: &mut BackgroundPaneLayout, direction: SharedRotationDirection) {
    let BackgroundPaneLayout::Split {
        axis,
        first_ratio,
        first,
        second,
    } = layout
    else {
        return;
    };
    rotate_layout_geometry(first, direction);
    rotate_layout_geometry(second, direction);
    let reverse_children = matches!(
        (direction, axis.as_str()),
        (SharedRotationDirection::Clockwise, "horizontal")
            | (SharedRotationDirection::CounterClockwise, "vertical")
    );
    *axis = if axis == "horizontal" {
        "vertical"
    } else {
        "horizontal"
    }
    .to_owned();
    if reverse_children {
        std::mem::swap(first, second);
        *first_ratio = crate::protocol::BACKGROUND_PANE_SPLIT_RATIO_SCALE - *first_ratio;
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum Request {
    /// Confirms that the daemon has completed startup and can answer requests.
    Ping,
    /// Starts a process under the multiplexer and hands its terminal back.
    Spawn(SpawnRequest),
    /// Creates a pane owned by the daemon and attaches the caller through the
    /// shared byte-stream path. No descriptor is ever returned.
    SpawnShared(SharedSpawnRequest),
    /// Atomically starts every draft process and commits one canonical layout.
    /// The response is stream-free; callers attach every returned pane through
    /// the same shared path used by remote viewers.
    SpawnSharedBatch(SharedSpawnBatchRequest),
    /// Takes over a pane's terminal from the multiplexer.
    ///
    /// `pane_id` is absent for the session's first pane, which is how an
    /// attach starts: the caller cannot know a protected session's pane
    /// identifiers, because they are not published until it has authenticated.
    Attach {
        session_id: u64,
        pane_id: Option<u64>,
        secret: Option<String>,
        /// Refuses an exclusive descriptor handoff. Batch-created shared panes
        /// use this even for a local client so every viewer follows the same
        /// relay path.
        #[serde(default)]
        force_shared: bool,
    },
    /// Gives the multiplexer a screen checkpoint from the client that is
    /// showing a pane. During a revoke handover this is the screen that lets
    /// the multiplexer resume reading and relay the pane to every client that
    /// attaches. A live share uses the same one-shot message while the client
    /// remains exclusive, so a persisted offer does not start with an empty
    /// screen.
    ///
    /// Sent over a fresh connection from the client showing the pane. During a
    /// revoke it follows [`Event::Revoke`]; during a live share it precedes the
    /// [`Request::Share`] publication. The raw snapshot bytes follow the
    /// message. The connection carries nothing else and closes afterwards.
    ///
    /// `columns`/`lines` are the size the holder was showing the pane at. The
    /// daemon uses them as the pane's current applied size; shared clients
    /// still remain unmeasured until they report their own initialized layout.
    Snapshot {
        session_id: u64,
        pane_id: u64,
        /// Length of the raw bytes that follow the message on the connection.
        length: usize,
        columns: u16,
        lines: u16,
    },
    /// Stores a normalized clipboard image for a shared pane and returns the
    /// absolute path the attached TUI can consume. The raw PNG bytes follow
    /// the request on the connection.
    StoreImage {
        session_id: u64,
        pane_id: u64,
        /// Length of the raw PNG bytes that follow the message.
        length: usize,
    },
    /// Input from a shared client, on the shared connection [`Request::Attach`]
    /// left open after it was answered with [`Response::SharedAttached`].
    ///
    /// The raw bytes follow the message.
    Input {
        /// Length of the raw bytes that follow the message on the connection.
        length: usize,
    },
    /// Asks to be identified before the real request is sent, answered with
    /// [`Response::Ok`] once the exchange is over — successfully or not.
    ///
    /// How a request that streams raw bytes after its message gets an identity:
    /// the daemon cannot interject a challenge into one of those, because the
    /// bytes would arrive where it was expecting the answer.
    Attest,
    /// Proves, where the platform has no peer credentials, that this connection
    /// really belongs to the process its envelope named: the nonce read back
    /// from the handle in [`Response::AttestationRequired`].
    ///
    /// Sent only in answer to that response, on the same connection, after
    /// which the request that prompted it is dispatched.
    Attested { nonce: String },
    /// Gives a session back to the multiplexer to hold. The client has already
    /// stopped reading the panes' terminals by the time this is sent.
    Detach(DetachRequest),
    /// Restores an encrypted disk record after the client has decrypted it.
    /// The daemon receives state and authentication metadata, never an age
    /// identity or a private key. The raw snapshots described by the request
    /// follow the JSON frame on the same connection.
    Resume(Box<ResumeRequest>),
    /// Takes a shared pane's terminal back, in answer to [`Event::Grant`].
    ///
    /// The reverse of the revoke handover. Only the pane's single remaining
    /// viewer may send it, and the multiplexer answers with the descriptor
    /// exactly as it answers an exclusive [`Request::Attach`] — except that no
    /// replay comes with it, because everything read so far has already been
    /// relayed to this very client. What is left is still in the terminal, for
    /// the client to read itself from now on.
    ///
    /// Sent on a fresh connection, like [`Request::Snapshot`]: the shared
    /// connection is being retired, and the multiplexer closes its end of it once
    /// the last relayed frame has gone out.
    TakeExclusive { session_id: u64, pane_id: u64 },
    /// Offers a session that the client is still showing, so another client can
    /// attach to it and both then see the same panes.
    ///
    /// Deliberately not [`Request::Detach`] with the snapshots left out. A
    /// detach means "I have stopped reading these terminals, hold them for me",
    /// and it is what asks for the session to outlive its window; sharing means
    /// neither. Conflating the two made joining a live session require first
    /// dismissing it, which is the opposite of what the user asked for.
    Share(ShareRequest),
    /// Tells the multiplexer that an attached pane was resized.
    ///
    /// Needed where the pseudoconsole belongs to the multiplexer and only it
    /// can resize the console. On Unix the resize has already happened through
    /// the descriptor the client holds.
    Resize {
        session_id: u64,
        pane_id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        revision: Option<SessionRevision>,
        columns: u16,
        lines: u16,
    },
    /// Updates the legacy Win32 colors for one pane's pseudoconsole.
    SetConsolePalette {
        session_id: u64,
        pane_id: u64,
        palette: ConsolePalette,
    },
    /// The sessions being held, as published in the catalog.
    List,
    /// Ends a session and everything running in it.
    Kill { session_id: u64 },
    /// Scopes a session to one process, or shares it with every process.
    ///
    /// The CLI half of what `Ctrl-Shift-K` does to a tab on screen, for a
    /// session that is in the background and therefore has no window to toggle
    /// it from. Sharing needs no owner; scoping back needs one, and it is the
    /// process recorded when the session was last held — not the caller, which
    /// for a CLI is a process that exits a moment later.
    SetSessionScope {
        session_id: u64,
        shared: bool,
        /// The Argon2id verifier for the secret a joining process must present.
        ///
        /// Required when sharing a session that has none: a session another
        /// process can join unchallenged hands it whatever its terminals can
        /// already do, which for a shell that has answered `sudo` is root.
        verifier: Option<String>,
        /// The sealed session key that goes with `verifier`, when the secret was
        /// generated rather than typed. See
        /// [`crate::protocol::BackgroundSessionSummary::key_envelope`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key_envelope: Option<String>,
    },
    /// Removes a session from the catalog without killing it. The session
    /// continues running under the daemon but is no longer listed or
    /// attachable until the daemon restarts (at which point it is gone).
    Forget { session_id: u64 },
    /// Applies the current client's session retention settings to the daemon.
    ///
    /// A daemon outlives the client that started it, so a later client must not
    /// silently inherit whatever retention mode the earlier client selected.
    /// The recipient strings have already been resolved by the client; they
    /// are public age recipients, never identities or private keys.
    Configure {
        retention: crate::retention::Retention,
        persistence_recipients: Vec<String>,
    },
    /// Turns this connection into an event stream. No response follows; the
    /// daemon sends [`Event`]s until the connection is dropped.
    Subscribe,
    /// What the multiplexer currently knows about these panes.
    ///
    /// A client that lost its subscription — across an upgrade, or a daemon
    /// restart — missed every [`Event::PaneExited`] sent while it was away, and
    /// those events are broadcast to whoever is listening rather than queued.
    /// Asking directly on reconnect is what closes that hole: without it a pane
    /// whose process ended during the gap would wait for a notification that
    /// has already been and gone.
    PaneStates { pane_ids: Vec<u64> },
    /// Applies one collaboration mutation against an exact canonical revision.
    ApplyShared(SharedSessionOperationRequest),
    /// Creates or retrieves the complete canonical collaboration state.
    SharedSnapshot { session_id: u64 },
    /// Explicitly leaves a shared session without changing its daemon-owned
    /// panes. Closing a remote tab uses this rather than `ClosePane`.
    LeaveShared { session_id: u64 },
    /// Releases a pane whose window closed while its process was still running.
    ///
    /// Dropping the client's descriptor is not enough to tell the multiplexer
    /// anything: it holds its own, so the pane stays marked as taken, nobody
    /// drains it, and the program blocks as soon as the terminal's buffer
    /// fills. This hands the pane back so it is either drained or, if the
    /// session was never meant to outlive its window, ended.
    ClosePane { session_id: u64, pane_id: u64 },
    /// Stops the daemon once it is holding nothing.
    Shutdown,
    /// Replaces the daemon with a fresh image of itself, keeping every session.
    /// The image is the one the daemon resolved at startup; a client cannot
    /// choose it, because choosing it would mean inheriting the terminals of
    /// every protected session the daemon holds.
    Upgrade,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpawnRequest {
    /// Adds a pane to an existing session, or starts a new one when absent.
    pub session_id: Option<u64>,
    /// As [`Envelope::client_process_id`].
    #[serde(default)]
    pub client_process_id: u32,
    pub program: Option<String>,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub working_directory: Option<PathBuf>,
    pub size: TerminalSize,
    pub console_palette: ConsolePalette,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalSize {
    pub columns: u16,
    pub lines: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetachRequest {
    pub session_id: u64,
    /// What the catalog publishes for this session.
    pub summary: BackgroundSessionSummary,
    /// The client's own session state, round-tripped without being read. Tab
    /// layout, labels and flags belong to the application, so keeping them
    /// opaque means a new application feature needs no daemon change.
    pub state: serde_json::Value,
    /// An Argon2id verifier, when reattaching is to require a secret. Absent
    /// leaves an already-protected session's verifier as it is.
    pub verifier: Option<String>,
    /// The sealed session key that goes with `verifier`, when the secret was
    /// generated rather than typed. See
    /// [`crate::protocol::BackgroundSessionSummary::key_envelope`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_envelope: Option<String>,
    /// Per pane, the screen the client was showing, replayed on the next
    /// attach so reattaching does not start from a blank terminal.
    pub snapshots: Vec<PaneSnapshot>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeRequest {
    pub record_id: u64,
    pub summary: BackgroundSessionSummary,
    pub state: serde_json::Value,
    /// The daemon-owned collaboration envelope, when the restored record was
    /// shared. Keeping it in the resume request preserves the canonical
    /// revision instead of silently starting a new collaboration history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_state: Option<SharedSessionState>,
    pub verifier: Option<String>,
    /// As [`DetachRequest::key_envelope`], read back out of the record the
    /// client has just decrypted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_envelope: Option<String>,
    pub failed_authentications: u32,
    pub backoff_seconds: u64,
    pub created_at: u64,
    pub updated_at: u64,
    /// The session secret is sent only after the client has decrypted the
    /// record. It is checked by the daemon and then discarded before the
    /// restored record is kept in memory.
    pub secret: Option<String>,
    /// Per-pane lengths for the raw snapshots that follow this message on the
    /// connection. Keeping the screen bytes out of the JSON frame matters for
    /// a disk record with a large scrollback buffer: control messages are
    /// deliberately capped, while snapshots already have their own retention
    /// limit and framing.
    pub snapshots: Vec<ResumeSnapshot>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeSnapshot {
    pub pane_id: u64,
    /// Number of raw bytes following the request for this pane.
    pub length: usize,
}

/// Publishes a session the client is still showing, so other clients can find
/// and attach to it.
///
/// Carries the same summary and state a detach does, and for the same reason:
/// the catalog needs something to list, and a client that joins rebuilds its tab
/// from the state. Snapshot bytes are sent separately with [`Request::Snapshot`]
/// immediately beforehand: the panes are still being read by the sharing
/// client, so the share publication itself must not carry a large raw payload.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShareRequest {
    pub session_id: u64,
    /// What the catalog publishes for this session.
    pub summary: BackgroundSessionSummary,
    /// As [`DetachRequest::state`].
    pub state: serde_json::Value,
    /// As [`DetachRequest::verifier`].
    pub verifier: Option<String>,
    /// As [`DetachRequest::key_envelope`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_envelope: Option<String>,
    /// Whether the session is being offered or withdrawn.
    ///
    /// Withdrawing stops the session being listed and attachable; it does not
    /// evict clients that already joined, because there is no way to give a pane
    /// back to one viewer exclusively while another is still relaying it.
    pub offered: bool,
}

/// What the multiplexer knows about one pane, for a client catching up after
/// its subscription was interrupted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaneStateReport {
    pub pane_id: u64,
    /// The multiplexer is not holding this pane at all: it ended and was
    /// pruned, or its session was killed. Either way there is nothing left to
    /// wait for, which a client must be able to distinguish from "still
    /// running" — the two look identical from a terminal that cannot reap.
    pub unknown: bool,
    pub exited: bool,
    /// The raw status the multiplexer observed, when it observed one.
    pub raw_status: Option<i32>,
    /// As [`Event::PaneExited::input_sent`].
    #[serde(default)]
    pub input_sent: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaneSnapshot {
    pub pane_id: u64,
    /// Length of the raw bytes that follow the message on the connection.
    /// Kept out of the JSON so terminal output is never re-encoded.
    pub length: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "response", rename_all = "snake_case")]
pub enum Response {
    Spawned {
        session_id: u64,
        pane_id: u64,
        child_pid: u32,
        /// Terminal handles already duplicated into the client, where the
        /// platform cannot attach them to the message. Empty on Unix.
        #[serde(default)]
        handles: Vec<i64>,
    },
    Attached {
        pane_id: u64,
        child_pid: u32,
        /// Raw bytes following this message: the snapshot taken at detach plus
        /// everything the pane has produced since.
        replay_length: usize,
        state: serde_json::Value,
        summary: Box<BackgroundSessionSummary>,
        /// As [`Response::Spawned::handles`].
        #[serde(default)]
        handles: Vec<i64>,
    },
    /// An attach that became shared: another client holds the pane, so instead
    /// of the terminal descriptor this connection stays open and carries the
    /// pane's output and this client's input, as framed by [`Event::Output`]
    /// and [`Request::Input`].
    ///
    /// No handles are attached: the connection *is* the terminal. The raw
    /// replay bytes follow the message, exactly as with [`Response::Attached`].
    /// `columns`/`lines` are the daemon's current effective size. They are
    /// advisory: a client reports its own initialized layout over `Resize`
    /// before it participates in shared-size arbitration.
    SharedAttached {
        pane_id: u64,
        child_pid: u32,
        replay_length: usize,
        state: serde_json::Value,
        summary: Box<BackgroundSessionSummary>,
        columns: u16,
        lines: u16,
    },
    /// A shared spawn has the same replay/data-plane framing as an attach, but
    /// also returns the new canonical session snapshot.
    SharedSpawned {
        session_id: u64,
        pane_id: u64,
        child_pid: u32,
        replay_length: usize,
        state: serde_json::Value,
        summary: Box<BackgroundSessionSummary>,
        shared_state: SharedSessionState,
    },
    SharedBatchSpawned {
        mappings: Vec<SharedDraftMapping>,
        state: SharedSessionState,
    },
    SharedOperationApplied {
        state: SharedSessionState,
    },
    SharedSnapshot {
        state: SharedSessionState,
    },
    SharedConflict {
        state: SharedSessionState,
    },
    Detached,
    Resumed {
        session_id: u64,
    },
    Sessions {
        sessions: Vec<BackgroundSessionSummary>,
        #[serde(default)]
        restorable: Vec<RestorableSessionRecord>,
    },
    /// Answers [`Request::PaneStates`], in the order asked.
    PaneStates {
        panes: Vec<PaneStateReport>,
    },
    /// Answers [`Request::StoreImage`] with the path visible to the session's
    /// child process.
    ImageStored {
        path: String,
    },
    Ok,
    /// Asks the client to prove it is the process its envelope named, before a
    /// request that would act on a protected session someone else may own is
    /// dispatched. Sent only where the platform has no peer credentials.
    ///
    /// `handle` is a pipe holding a nonce, already duplicated into the named
    /// process; the client reads it and answers with [`Request::Attested`].
    AttestationRequired {
        handle: i64,
    },
    /// The session exists but is protected and no secret was offered.
    AuthenticationRequired,
    /// The secret was wrong, or the session is inside its backoff window.
    /// Deliberately one answer for both, so the window cannot be probed.
    AuthenticationFailed,
    Error {
        message: String,
    },
}

/// Sent by the daemon to every subscriber, unprompted.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// A pane's process ended. Carries the raw `waitpid` status so a client
    /// holding the terminal can report the same exit it would have observed
    /// had it spawned the process itself.
    PaneExited {
        session_id: u64,
        pane_id: u64,
        raw_status: Option<i32>,
        /// Whether any attached client typed into this pane. Only the shared
        /// data plane can know: the daemon is the one that receives shared
        /// clients' input, and it reports here what no single client could see
        /// by itself. Exclusive clients type through their own descriptor, so
        /// for them this stays `false` and the client's own keystrokes are the
        /// truth.
        #[serde(default)]
        input_sent: bool,
    },
    /// This client is the only viewer left on a shared pane, so it may take the
    /// terminal back and stop being relayed to.
    ///
    /// An offer, not an instruction: a client that cannot take a pty — or does not
    /// want to — ignores it and stays shared. Answered with
    /// [`Request::TakeExclusive`].
    Grant { session_id: u64, pane_id: u64 },
    /// The multiplexer is about to replace itself, so this subscription is
    /// about to end for a reason that is not a failure.
    ///
    /// The sessions, the terminals and the shells all survive; only the
    /// connections do, because an `execv` cannot carry them. Announcing it is
    /// what lets a client tell an orderly replacement from a daemon that died,
    /// and reconnect promptly instead of waiting out a backoff.
    ///
    /// A client that never sees this — because the daemon crashed, or because
    /// the event lost the race with the exec — must still recover, so this is an
    /// optimisation rather than the mechanism. The mechanism is that losing the
    /// subscription is never on its own treated as a pane exiting.
    Replacing,
    /// The client holding this pane's terminal must hand it over: another
    /// client attached, and the pane is becoming shared.
    ///
    /// Delivered on the subscription connection, the only long-lived one a
    /// client keeps. The holder stops reading the pane, sends
    /// [`Request::Snapshot`] with the screen it was showing, and re-attaches —
    /// which answers with [`Response::SharedAttached`] and keeps the new
    /// connection as the shared data plane.
    Revoke { session_id: u64, pane_id: u64 },
    /// Output from a shared pane, on a shared connection. The raw bytes follow
    /// the message.
    Output {
        pane_id: u64,
        /// Length of the raw bytes that follow the message on the connection.
        length: usize,
    },
    /// The size every shared client must show this pane at: the smallest size
    /// any measured client asked for, applied by the multiplexer. Broadcast to
    /// every shared client whenever the smallest changes, and also to a client
    /// whose larger report needs correction back to the effective size.
    Size {
        session_id: u64,
        pane_id: u64,
        revision: SessionRevision,
        columns: u16,
        lines: u16,
    },
    /// Complete canonical state after a successful shared mutation. Sending a
    /// snapshot rather than a delta makes reconnect and event-gap recovery
    /// deterministic.
    SharedSessionUpdated { state: SharedSessionState },
    /// A new daemon-owned pane was added to a shared session. The pane's data
    /// connection is deliberately opened by each viewer after receiving this
    /// event, so the subscription never has to carry raw replay bytes.
    SharedPaneAdded {
        session_id: u64,
        pane_id: u64,
        child_pid: u32,
        state: SharedSessionState,
    },
    /// A pane was removed globally from a shared session. The accompanying
    /// snapshot is authoritative for layout and focus.
    SharedPaneRemoved {
        session_id: u64,
        pane_id: u64,
        state: SharedSessionState,
    },
    /// One atomic pane-set change and the exact canonical snapshot committed
    /// with it. Added and removed IDs are grouped so no viewer observes a
    /// partially applied template.
    SharedPanesChanged {
        session_id: u64,
        added: Vec<u64>,
        removed: Vec<u64>,
        state: SharedSessionState,
    },
    /// A shared data connection failed. This is recoverable; the client should
    /// replace the stream after asking for the authoritative snapshot.
    SharedStreamFailed { session_id: u64, pane_id: u64 },
    /// The shared data connection was retired cleanly after its final queued
    /// output. This is the handover/leave signal; unlike a relay failure it is
    /// an ordinary end of stream and must not trigger reconnect machinery.
    SharedClosed { session_id: u64, pane_id: u64 },
}

#[cfg(test)]
#[path = "tests/messages.rs"]
mod tests;
