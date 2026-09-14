//! Headless shared-session creation.
//!
//! This module is deliberately independent of Zetta's windowed configuration
//! model.  A headless host only needs the execution half of a profile and a
//! small, strict layout description that can be sent to the mux daemon.

use std::{collections::HashMap, io::Read, path::PathBuf};

use alacritty_terminal::tty::ConsolePalette;
use anyhow::{Context as _, Result};
use serde::Deserialize;

use crate::{
    messages::{
        CreateSharedRequest, SharedDraftLayout, SharedOperationId, SharedPaneDraft, SharedPaneRef,
        TerminalSize,
    },
    protocol::{BackgroundPaneState, BackgroundPaneSummary},
};

pub const DEFAULT_COLUMNS: u16 = 80;
pub const DEFAULT_LINES: u16 = 24;

/// The JSON envelope accepted by `zmux create --layout`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutFile {
    #[serde(default)]
    pub title: String,
    #[serde(default, alias = "active")]
    pub active_pane: usize,
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub layout: LayoutNode,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum LayoutNode {
    Horizontal(HorizontalLayout),
    Vertical(VerticalLayout),
    Pane(Box<HeadlessPane>),
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HorizontalLayout {
    pub horizontal: [Box<LayoutNode>; 2],
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerticalLayout {
    pub vertical: [Box<LayoutNode>; 2],
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeadlessPane {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub command: Option<HeadlessCommand>,
    #[serde(default)]
    pub working_directory: Option<PathBuf>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub size: Option<HeadlessSize>,
    #[serde(default)]
    pub console_palette: Option<ConsolePalette>,
    #[serde(default)]
    pub metadata: Option<HeadlessMetadata>,
    #[serde(default)]
    pub load_shell_integration: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeadlessCommand {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeadlessSize {
    pub columns: u16,
    pub lines: u16,
    #[serde(default)]
    pub cell_width: u16,
    #[serde(default)]
    pub cell_height: u16,
}

impl From<HeadlessSize> for TerminalSize {
    fn from(size: HeadlessSize) -> Self {
        Self {
            columns: size.columns,
            lines: size.lines,
            cell_width: size.cell_width,
            cell_height: size.cell_height,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeadlessMetadata {
    #[serde(default)]
    pub application: Option<String>,
    #[serde(default)]
    pub terminal_title: Option<String>,
}

/// The validated, daemon-ready form of a headless layout.
#[derive(Clone, Debug)]
pub struct CreateSpec {
    pub title: String,
    pub layout: SharedDraftLayout,
    pub active_pane: SharedPaneRef,
    pub panes: Vec<SharedPaneDraft>,
}

impl CreateSpec {
    pub fn request(
        self,
        operation_id: SharedOperationId,
        verifier: Option<String>,
    ) -> CreateSharedRequest {
        CreateSharedRequest {
            operation_id,
            title: self.title,
            replacement: self.layout,
            panes: self.panes,
            active_pane: Some(self.active_pane),
            verifier,
        }
    }
}

impl LayoutFile {
    pub fn into_spec(self) -> Result<CreateSpec> {
        let mut panes = Vec::new();
        let mut next_draft_id = 1;
        let mut active_pane = None;
        let layout = convert_layout(
            &self.layout,
            &self.env,
            &mut panes,
            &mut next_draft_id,
            self.active_pane,
            &mut active_pane,
        )?;
        let active_pane = active_pane.context("active_pane is outside the layout")?;
        anyhow::ensure!(!panes.is_empty(), "a headless layout must contain a pane");
        Ok(CreateSpec {
            title: self.title,
            layout,
            active_pane,
            panes,
        })
    }
}

pub fn parse_reader(mut reader: impl Read) -> Result<CreateSpec> {
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .context("reading the headless layout")?;
    serde_json::from_slice::<LayoutFile>(&bytes)
        .context("parsing the headless layout JSON")?
        .into_spec()
}

pub fn parse_path(path: &std::path::Path) -> Result<CreateSpec> {
    if path.as_os_str() == "-" {
        return parse_reader(std::io::stdin().lock());
    }
    let file = std::fs::File::open(path)
        .with_context(|| format!("opening headless layout {}", path.display()))?;
    parse_reader(file)
}

pub fn single_pane(
    title: String,
    profile: Option<String>,
    working_directory: Option<PathBuf>,
    env: HashMap<String, String>,
) -> CreateSpec {
    let profile = profile.unwrap_or_else(|| "System".to_owned());
    let draft_id = 1;
    let metadata = pane_metadata(draft_id, None, &profile, None, working_directory.clone());
    CreateSpec {
        title,
        layout: SharedDraftLayout::Draft { draft_id },
        active_pane: SharedPaneRef::Draft { draft_id },
        panes: vec![SharedPaneDraft {
            draft_id,
            profile,
            command: None,
            env,
            working_directory,
            inherit_working_directory_from: None,
            load_shell_integration: false,
            size: default_size(),
            console_palette: ConsolePalette::default(),
            metadata,
        }],
    }
}

fn convert_layout(
    node: &LayoutNode,
    inherited_env: &HashMap<String, String>,
    panes: &mut Vec<SharedPaneDraft>,
    next_draft_id: &mut u64,
    active_index: usize,
    active_pane: &mut Option<SharedPaneRef>,
) -> Result<SharedDraftLayout> {
    match node {
        LayoutNode::Horizontal(layout) => Ok(split_layout(
            "horizontal",
            &layout.horizontal,
            inherited_env,
            panes,
            next_draft_id,
            active_index,
            active_pane,
        )?),
        LayoutNode::Vertical(layout) => Ok(split_layout(
            "vertical",
            &layout.vertical,
            inherited_env,
            panes,
            next_draft_id,
            active_index,
            active_pane,
        )?),
        LayoutNode::Pane(pane) => {
            let draft_id = *next_draft_id;
            *next_draft_id = next_draft_id.saturating_add(1);
            if panes.len() == active_index {
                *active_pane = Some(SharedPaneRef::Draft { draft_id });
            }
            let mut env = inherited_env.clone();
            env.extend(pane.env.clone());
            let profile = pane.profile.clone().unwrap_or_else(|| "System".to_owned());
            let command = pane
                .command
                .as_ref()
                .map(|command| -> Result<_> {
                    anyhow::ensure!(
                        !command.program.trim().is_empty(),
                        "headless command program must not be empty"
                    );
                    Ok(zetta_profiles::ProfileCommand::with_args(
                        command.program.clone(),
                        command.args.clone(),
                    ))
                })
                .transpose()?;
            let working_directory = pane.working_directory.clone();
            let metadata = pane_metadata(
                draft_id,
                pane.label.clone(),
                &profile,
                pane.metadata.as_ref(),
                working_directory.clone(),
            );
            panes.push(SharedPaneDraft {
                draft_id,
                profile,
                command,
                env,
                working_directory,
                inherit_working_directory_from: None,
                load_shell_integration: pane.load_shell_integration,
                size: pane.size.map(Into::into).unwrap_or_else(default_size),
                console_palette: pane.console_palette.unwrap_or_default(),
                metadata,
            });
            Ok(SharedDraftLayout::Draft { draft_id })
        }
    }
}

fn split_layout(
    axis: &str,
    children: &[Box<LayoutNode>; 2],
    inherited_env: &HashMap<String, String>,
    panes: &mut Vec<SharedPaneDraft>,
    next_draft_id: &mut u64,
    active_index: usize,
    active_pane: &mut Option<SharedPaneRef>,
) -> Result<SharedDraftLayout> {
    let first = convert_layout(
        &children[0],
        inherited_env,
        panes,
        next_draft_id,
        active_index,
        active_pane,
    )?;
    let second = convert_layout(
        &children[1],
        inherited_env,
        panes,
        next_draft_id,
        active_index,
        active_pane,
    )?;
    Ok(SharedDraftLayout::Split {
        axis: axis.to_owned(),
        first_ratio: crate::protocol::DEFAULT_BACKGROUND_PANE_SPLIT_RATIO,
        first: Box::new(first),
        second: Box::new(second),
    })
}

fn pane_metadata(
    draft_id: u64,
    label: Option<String>,
    profile: &str,
    metadata: Option<&HeadlessMetadata>,
    working_directory: Option<PathBuf>,
) -> BackgroundPaneSummary {
    BackgroundPaneSummary {
        id: draft_id,
        label: label.unwrap_or_else(|| format!("pane-{draft_id}")),
        profile: profile.to_owned(),
        configured_command: String::new(),
        application: metadata
            .and_then(|metadata| metadata.application.clone())
            .unwrap_or_else(|| profile.to_owned()),
        foreground_command: None,
        terminal_title: metadata.and_then(|metadata| metadata.terminal_title.clone()),
        working_directory,
        state: BackgroundPaneState::Starting,
        exit: None,
    }
}

fn default_size() -> TerminalSize {
    TerminalSize {
        columns: DEFAULT_COLUMNS,
        lines: DEFAULT_LINES,
        cell_width: 0,
        cell_height: 0,
    }
}

#[cfg(test)]
#[path = "tests/headless.rs"]
mod tests;
