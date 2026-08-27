//! A tab, as the multiplexer stores it.
//!
//! The multiplexer holds this without reading it: to `zmux` a session's state
//! is an opaque blob it round-trips, so a new tab feature needs no change
//! there. That only works if everything durable about a tab can be written
//! down here and rebuilt from it.
//!
//! Two things are deliberately *not* stored.
//!
//! Transient editing state — a half-typed rename, an open overlay picker — is
//! dropped, because restoring a session into the middle of an interaction the
//! user has since forgotten about is worse than restoring it settled.
//!
//! A pane's profile is stored by name and re-resolved on restore rather than
//! frozen. A profile is user configuration; a session restored after the user
//! edited one should follow the edit, and a profile that has since been
//! removed should surface as a missing profile rather than as a stale copy the
//! configuration no longer describes.

use std::collections::HashMap;

use serde::{Deserialize, Deserializer, Serialize};

use crate::TabIconOverride;
use crate::{
    BackgroundPaneExit, PaneLayout, PaneStack, PaneStackSelection, Profile, SplitAxis, StackedPane,
    Tab, TabClosePolicy, TerminalPane,
};

fn deserialize_icon_override<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TabState {
    /// Identifies the live Zetta configuration generation that published this
    /// state. A process that reloaded after detaching the session must not
    /// resurrect session-theme choices that reload intentionally cleared. Other
    /// processes, and disk resume, still restore the saved choices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pane_theme_source: Option<PaneThemeSource>,
    /// Stable across a move between visible and background storage, which is
    /// what lets process control keep addressing the same tab.
    pub(crate) attention_id: u64,
    pub(crate) next_pane_label: usize,
    pub(crate) layout: LayoutState,
    pub(crate) active_pane: u64,
    pub(crate) focus_history: Vec<u64>,
    pub(crate) maximized_pane: Option<u64>,
    pub(crate) minimized_panes: Vec<u64>,
    pub(crate) selected_minimized_pane: Option<u64>,
    pub(crate) broadcast_input: bool,
    pub(crate) silent_mode: bool,
    /// Whether this tab keeps running when it or its window closes. The
    /// verifier itself is not here: it belongs to the multiplexer, which is
    /// the only thing that checks it.
    pub(crate) keep_running: bool,
    /// Whether the session is offered to other windows.
    ///
    /// Round-tripped so a window that joins a shared session shows the sharing
    /// as switched on rather than as something it would have to switch on again.
    /// `default` because a session detached by an older Zetta has no such field
    /// and was, by definition, not being shared.
    #[serde(default)]
    pub(crate) shared: bool,
    pub(crate) custom_title: Option<String>,
    pub(crate) worktree_seed_title: Option<String>,
    pub(crate) process_title: Option<String>,
    pub(crate) icon: Option<String>,
    /// Absent means no per-tab override. A string selects an icon and an
    /// explicit JSON null hides it; the distinction is needed because the
    /// effective `icon` field alone cannot represent both states.
    #[serde(
        default,
        deserialize_with = "deserialize_icon_override",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) icon_override: Option<Option<String>>,
    pub(crate) pinned: bool,
    pub(crate) panes: Vec<PaneState>,
    /// Absent means the tab follows project/profile/application themes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) theme_override: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PaneState {
    pub(crate) id: u64,
    /// The multiplexer's identifier for this pane's terminal, which is how the
    /// two sides agree on what to hand over at attach.
    pub(crate) mux_pane_id: Option<u64>,
    pub(crate) label_number: usize,
    pub(crate) generated_label: Option<String>,
    pub(crate) custom_label: Option<String>,
    pub(crate) profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) theme_override: Option<String>,
    pub(crate) environment_overrides: HashMap<String, String>,
    pub(crate) overlay: Option<OverlayState>,
    pub(crate) exit: Option<BackgroundPaneExit>,
    pub(crate) base_exited: bool,
    pub(crate) pending_command: Option<String>,
    pub(crate) detected_worktree_title: Option<String>,
    /// The commands stacked on top of this pane's shell, and which of them is
    /// showing.
    ///
    /// Round-tripped rather than dropped because `base_exited` is: a pane
    /// detached with its shell gone and a stacked command in front came back
    /// with neither the command nor a base terminal — nothing to show and no way
    /// to get anything, which is worse than either keeping the stack or refusing
    /// to detach the tab.
    #[serde(default)]
    pub(crate) stack: Vec<StackedPaneState>,
    /// The stacked command selected, by id; absent means the base shell.
    #[serde(default)]
    pub(crate) selected_stacked: Option<u64>,
}

/// One command stacked on a pane.
///
/// Its terminal belongs to the multiplexer like any other, so `mux_pane_id` is
/// what lets a command that is still running be reattached rather than merely
/// remembered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StackedPaneState {
    pub(crate) id: u64,
    pub(crate) mux_pane_id: Option<u64>,
    pub(crate) command: String,
    pub(crate) profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) theme_override: Option<String>,
    pub(crate) state: crate::pane::StackedPaneState,
    pub(crate) exit_code: Option<i32>,
    pub(crate) error: Option<String>,
    pub(crate) working_directory: Option<std::path::PathBuf>,
    pub(crate) wsl_directory: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PaneThemeSource {
    pub(crate) process_id: u32,
    pub(crate) runner_id: u64,
    pub(crate) configuration_generation: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OverlayState {
    pub(crate) text: String,
    /// The size's stable CLI name (`sm`, `xl`, …) rather than a measurement,
    /// so a restored overlay follows the same scale the user chose.
    pub(crate) font_size: Option<String>,
    pub(crate) opacity: Option<f32>,
    /// Hue, saturation, lightness and alpha, stored componentwise rather than
    /// as a theme colour name: an overlay colour is a literal choice and must
    /// not shift when the theme changes.
    pub(crate) color: Option<[f32; 4]>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum LayoutState {
    Pane {
        pane_id: u64,
    },
    Split {
        axis: AxisState,
        /// Scaled by `PANE_SPLIT_RATIO_SCALE`, as in [`PaneLayout`]: an
        /// integer keeps a restored layout identical to the one saved rather
        /// than drifting by a fraction of a pixel.
        first_ratio: u16,
        first: Box<LayoutState>,
        second: Box<LayoutState>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AxisState {
    Horizontal,
    Vertical,
}

impl From<SplitAxis> for AxisState {
    fn from(axis: SplitAxis) -> Self {
        match axis {
            SplitAxis::Horizontal => Self::Horizontal,
            SplitAxis::Vertical => Self::Vertical,
        }
    }
}

impl From<AxisState> for SplitAxis {
    fn from(axis: AxisState) -> Self {
        match axis {
            AxisState::Horizontal => Self::Horizontal,
            AxisState::Vertical => Self::Vertical,
        }
    }
}

impl LayoutState {
    fn from_layout(layout: &PaneLayout) -> Self {
        match layout {
            PaneLayout::Pane(pane_id) => Self::Pane { pane_id: *pane_id },
            PaneLayout::Split {
                axis,
                first_ratio,
                first,
                second,
            } => Self::Split {
                axis: (*axis).into(),
                first_ratio: *first_ratio,
                first: Box::new(Self::from_layout(first)),
                second: Box::new(Self::from_layout(second)),
            },
        }
    }

    fn into_layout(self) -> PaneLayout {
        match self {
            Self::Pane { pane_id } => PaneLayout::Pane(pane_id),
            Self::Split {
                axis,
                first_ratio,
                first,
                second,
            } => PaneLayout::Split {
                axis: axis.into(),
                first_ratio,
                first: Box::new(first.into_layout()),
                second: Box::new(second.into_layout()),
            },
        }
    }

    /// Every pane the layout refers to, in order.
    ///
    /// Recurses, as do [`Self::from_layout`] and [`Self::into_layout`], over a
    /// tree that arrives as JSON from the multiplexer. That is safe without a
    /// depth check of its own because `serde_json` refuses to deserialize past
    /// its own nesting limit, so a layout this deep never becomes a
    /// `LayoutState` in the first place — the attach fails while parsing, with
    /// an error, rather than while walking. Anything that stops parsing the
    /// state through `serde_json` has to bound the depth itself.
    fn pane_ids(&self, into: &mut Vec<u64>) {
        match self {
            Self::Pane { pane_id } => into.push(*pane_id),
            Self::Split { first, second, .. } => {
                first.pane_ids(into);
                second.pane_ids(into);
            }
        }
    }
}

/// Rebuilds a pane's stack of commands.
///
/// A stacked command's terminal is a *task* terminal, and reattaching one is not
/// something the terminal crate can do yet: `new_attached` builds an interactive
/// terminal, so the completion event a stacked entry is driven by would never
/// arrive. So a command that was still running comes back marked as having ended
/// without an answer, with a reason saying why, rather than being restored as
/// "running" and then never finishing. A command that had already completed
/// restores exactly, because its outcome is all there was left of it.
///
/// What must not happen is the entry vanishing, which is what used to happen:
/// the stack was dropped while `base_exited` was kept, so a pane detached with
/// its shell gone and a command in front came back with nothing to show and no
/// way to get anything.
fn restore_stack(
    entries: Vec<StackedPaneState>,
    selected: Option<u64>,
    resolve_profile: &impl Fn(u64, &str) -> Profile,
) -> PaneStack {
    let entries = entries
        .into_iter()
        .map(|entry| {
            let mut restored = StackedPane::new(
                entry.id,
                entry.command,
                resolve_profile(entry.id, &entry.profile),
                entry.working_directory,
                entry.wsl_directory,
            );
            restored.theme_override = entry.theme_override;
            let was_running = matches!(
                entry.state,
                crate::pane::StackedPaneState::Starting | crate::pane::StackedPaneState::Running
            );
            restored.state = if was_running {
                crate::pane::StackedPaneState::Failed
            } else {
                entry.state
            };
            restored.exit_code = entry.exit_code;
            restored.error = if was_running {
                Some(
                    "This command was still running when the session was detached, so its output \
                     could not be restored."
                        .to_owned(),
                )
            } else {
                entry.error
            };
            restored
        })
        .collect::<Vec<_>>();
    let selected = match selected {
        Some(id) if entries.iter().any(|entry| entry.id == id) => PaneStackSelection::Stacked(id),
        _ => PaneStackSelection::Base,
    };
    PaneStack { entries, selected }
}

impl TabState {
    pub(crate) fn from_tab(tab: &Tab, mux_pane_ids: &HashMap<u64, u64>) -> Self {
        Self {
            pane_theme_source: None,
            attention_id: tab.attention_id,
            next_pane_label: tab.next_pane_label,
            layout: LayoutState::from_layout(&tab.layout),
            active_pane: tab.active_pane,
            focus_history: tab.focus_history.clone(),
            maximized_pane: tab.maximized_pane,
            minimized_panes: tab.minimized_panes.clone(),
            selected_minimized_pane: tab.selected_minimized_pane,
            broadcast_input: tab.broadcast_input,
            silent_mode: tab.silent_mode,
            keep_running: matches!(tab.close_policy, TabClosePolicy::Background { .. }),
            shared: tab.shared,
            custom_title: tab.custom_title.clone(),
            worktree_seed_title: tab.worktree_seed_title.clone(),
            process_title: tab.process_title.clone(),
            icon: tab.icon.map(|icon| {
                let name: &'static str = icon.into();
                name.to_owned()
            }),
            icon_override: match tab.icon_override {
                TabIconOverride::None => None,
                TabIconOverride::Icon(icon) => {
                    let name: &'static str = icon.into();
                    Some(Some(name.to_owned()))
                }
                TabIconOverride::Hidden => Some(None),
            },
            pinned: tab.pinned,
            panes: tab
                .panes
                .iter()
                .map(|pane| PaneState {
                    id: pane.id,
                    mux_pane_id: mux_pane_ids.get(&pane.id).copied(),
                    label_number: pane.label_number,
                    generated_label: pane.generated_label.clone(),
                    custom_label: pane.custom_label.clone(),
                    profile: pane.profile.name.clone(),
                    theme_override: pane.theme_override.clone(),
                    environment_overrides: pane.environment_overrides.clone(),
                    overlay: pane.overlay_text.as_ref().map(|text| OverlayState {
                        text: text.clone(),
                        font_size: pane
                            .overlay_font_size
                            .map(|size| size.cli_name().to_owned()),
                        opacity: pane.overlay_opacity,
                        color: pane
                            .overlay_color
                            .map(|color| [color.h, color.s, color.l, color.a]),
                    }),
                    exit: pane.exit.clone(),
                    base_exited: pane.base_exited,
                    pending_command: pane.pending_command.clone(),
                    detected_worktree_title: pane.detected_worktree_title.clone(),
                    stack: pane
                        .stack
                        .entries
                        .iter()
                        .map(|entry| StackedPaneState {
                            id: entry.id,
                            mux_pane_id: mux_pane_ids.get(&entry.id).copied(),
                            command: entry.command.clone(),
                            profile: entry.profile.name.clone(),
                            theme_override: entry.theme_override.clone(),
                            state: entry.state,
                            exit_code: entry.exit_code,
                            error: entry.error.clone(),
                            working_directory: entry.working_directory.clone(),
                            wsl_directory: entry.wsl_directory.clone(),
                        })
                        .collect(),
                    selected_stacked: match pane.stack.selected {
                        PaneStackSelection::Base => None,
                        PaneStackSelection::Stacked(id) => Some(id),
                    },
                })
                .collect(),
            theme_override: tab.theme_override.clone(),
        }
    }

    /// Rebuilds a tab, resolving each pane's profile by name.
    ///
    /// Returns an error rather than substituting a default when the layout and
    /// the pane list disagree: a tab whose layout names a pane that does not
    /// exist has no sensible rendering, and silently dropping the difference
    /// would lose a running terminal from the user's view while leaving it
    /// running in the multiplexer.
    #[cfg(test)]
    pub(crate) fn into_tab(
        self,
        tab_id: u64,
        resolve_profile: impl Fn(&str) -> Profile,
    ) -> anyhow::Result<Tab> {
        self.into_tab_by_pane(tab_id, |_, name| resolve_profile(name))
    }

    pub(crate) fn into_tab_by_pane(
        self,
        tab_id: u64,
        resolve_profile: impl Fn(u64, &str) -> Profile,
    ) -> anyhow::Result<Tab> {
        let mut layout_panes = Vec::new();
        self.layout.pane_ids(&mut layout_panes);
        for pane_id in &layout_panes {
            anyhow::ensure!(
                self.panes.iter().any(|pane| pane.id == *pane_id),
                "the saved layout refers to pane {pane_id}, which the session does not have"
            );
        }
        anyhow::ensure!(
            self.panes
                .iter()
                .all(|pane| layout_panes.contains(&pane.id)),
            "the session has panes the saved layout does not place"
        );
        anyhow::ensure!(
            layout_panes.contains(&self.active_pane),
            "the saved active pane {} is not in the layout",
            self.active_pane
        );

        let icon = self
            .icon
            .as_deref()
            .and_then(crate::tab_icon_picker::parse_tab_icon_name);
        let icon_override = match self.icon_override {
            None => TabIconOverride::None,
            Some(None) => TabIconOverride::Hidden,
            Some(Some(name)) => crate::tab_icon_picker::parse_tab_icon_name(&name)
                .map(TabIconOverride::Icon)
                .unwrap_or_default(),
        };
        let icon = match icon_override {
            TabIconOverride::None => icon,
            TabIconOverride::Icon(icon) => Some(icon),
            TabIconOverride::Hidden => None,
        };

        let panes = self
            .panes
            .into_iter()
            .map(|pane| {
                let mut restored =
                    TerminalPane::new(pane.id, resolve_profile(pane.id, &pane.profile))
                        .with_label_number(pane.label_number);
                restored.theme_override = pane.theme_override;
                restored.generated_label = pane.generated_label;
                restored.custom_label = pane.custom_label;
                restored.environment_overrides = pane.environment_overrides;
                if let Some(overlay) = pane.overlay {
                    restored.overlay_text = Some(overlay.text);
                    restored.overlay_font_size = overlay
                        .font_size
                        .as_deref()
                        .and_then(crate::OverlayFontSize::parse);
                    restored.overlay_opacity = overlay.opacity;
                    restored.overlay_color =
                        overlay.color.map(|[h, s, l, a]| gpui::Hsla { h, s, l, a });
                }
                restored.exit = pane.exit;
                restored.base_exited = pane.base_exited;
                restored.pending_command = pane.pending_command;
                restored.detected_worktree_title = pane.detected_worktree_title;
                restored.stack = restore_stack(pane.stack, pane.selected_stacked, &resolve_profile);
                restored
            })
            .collect::<Vec<_>>();
        let pane_indices = panes
            .iter()
            .enumerate()
            .map(|(index, pane)| (pane.id, index))
            .collect();

        Ok(Tab {
            id: tab_id,
            attention_id: self.attention_id,
            attention: None,
            panes,
            pane_indices,
            next_pane_label: self.next_pane_label,
            theme_override: self.theme_override,
            layout: self.layout.into_layout(),
            active_pane: self.active_pane,
            focus_history: self.focus_history,
            maximized_pane: self.maximized_pane,
            minimized_panes: self.minimized_panes,
            selected_minimized_pane: self.selected_minimized_pane,
            broadcast_input: self.broadcast_input,
            silent_mode: self.silent_mode,
            // The verifier lives in the multiplexer, so a restored tab knows
            // only that it keeps running, not what secret would reattach it.
            close_policy: if self.keep_running {
                TabClosePolicy::Background {
                    authentication: None,
                }
            } else {
                TabClosePolicy::Close
            },
            shared: self.shared,
            custom_title: self.custom_title,
            worktree_seed_title: self.worktree_seed_title,
            process_title: self.process_title,
            icon,
            icon_override,
            pinned: self.pinned,
            renaming_pane: None,
            rename_buffer: None,
            rename_cursor: 0,
            rename_select_all: false,
            editing_overlay_pane: None,
            overlay_buffer: None,
            overlay_cursor: 0,
            overlay_select_all: false,
            overlay_style_picker: None,
        })
    }
}

#[cfg(test)]
#[path = "tests/session_state.rs"]
mod tests;
