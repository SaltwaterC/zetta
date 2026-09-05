#![cfg_attr(windows, windows_subsystem = "console")]

mod background_sessions;
mod cli_services;
mod command_palette;
mod config;
mod default_terminal;
#[cfg(feature = "http-server")]
mod http_server;
mod keymap_file;
mod mux;
#[cfg(feature = "session-persistence")]
mod mux_identity;
#[cfg(feature = "notifications")]
mod notification_sounds;
mod process_control;
mod profile_cli;
mod profile_icon;
mod project;
mod project_cli;
mod project_commands;
mod project_form;
mod run_command;
#[cfg(feature = "serial-console")]
mod serial_console;
#[cfg(servers_enabled)]
mod server_ui;
mod session_auth_ui;
#[cfg(feature = "session-persistence")]
mod session_auto_protect;
mod session_state;
mod settings_editor;
mod shell_integration;
mod silent_mode;
mod text_edit;
mod text_edit_ui;
#[cfg(tftp_enabled)]
mod tftp;
mod theme_extensions;
#[cfg(feature = "syntax-highlighting")]
mod vi_syntax;
mod zetta_assets;

const ZETTA_APP_ID: &str = "Zetta";
const ZETTA_DEFAULT_THEME: &str = "One Light";
const ZETTA_DEFAULT_DARK_THEME: &str = "One Dark";

use std::{
    collections::{HashMap, HashSet, VecDeque},
    env,
    ffi::OsString,
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context as _, Result};
use background_sessions::{
    BackgroundPaneExit, BackgroundPaneLayout, BackgroundPaneState, BackgroundPaneSummary,
    BackgroundSessionRunner, BackgroundSessionSummary, SessionAuthentication, SessionSecret,
    VerifiedSession, application_from_command_line, background_pane_exit_from_terminal,
};
use command_palette::{
    CommandPalette, PaletteCommand, PaletteKey, humanize_action_name, project_palette_commands,
};
use config::{
    Config, NewTabProfile, PaneControlsPosition, PaneSplitAxis, PaneSplitOverlaySize,
    PaneSplitPane, PaneSplitTemplate, PaneSplitTemplateConfig, Profile, WorkingDirectoryScope,
    profile_is_hidden, visible_profile_count, visible_profile_index,
};
use futures::StreamExt as _;
#[cfg(any(target_os = "windows", linux_like))]
use gpui::WindowControls;
use gpui::{
    Action, Anchor, Animation, AnimationExt, AnyElement, App, AppContext as _, Bounds, Context,
    CursorStyle, Decorations, DismissEvent, Entity, Focusable, FrameEvent, FrameTiming,
    FrameTimingCollector, Global, HitboxBehavior, Hsla, InteractiveElement as _, IntoElement,
    KeyBinding, KeyDownEvent, KeyUpEvent, KeybindingKeystroke, ListSizingBehavior,
    MAX_BUTTONS_PER_SIDE, MouseButton, PathPromptOptions, Pixels, PlatformKeyboardMapper, Point,
    Render, ResizeEdge, ScrollHandle, ScrollStrategy, SharedString, Size, Subscription, Task,
    Tiling, TitlebarOptions, UniformListScrollHandle, WeakEntity, Window,
    WindowBackgroundAppearance, WindowBounds, WindowButton, WindowButtonLayout, WindowControlArea,
    WindowDecorations, WindowId, WindowOptions, actions, anchored, canvas, container_query,
    deferred, div, point, profiler, px, size, svg, transparent_black, uniform_list,
};
use keymap_file::{DEFAULT_KEYMAP_PATH, KeymapFile, KeymapFileLoadResult};
use mux::{MuxPanes, MuxRuntime};
use process_control::{
    ProcessControlCommand, ProcessControlServer, ReconnectSessionResult, TabAttentionRequest,
    request_existing_process_window,
};
use profile_icon::ProfileIcon;
use project::{
    ProjectConfig, ProjectRegistry, canonical_project_root, paths_equal,
    resolve_registered_project_root,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use session_auth_ui::SessionAuthenticationPrompt;
use settings_editor::{
    BindingForm, ConfigTextField, ConfigurationForm, KeymapForm, KeymapSectionForm,
    KeymapTextField, PaneTemplateNodePath, PaneTemplateTextField, SettingsPage,
    save as save_settings_file,
};
use silent_mode::{FocusStatusAccess, SilentModeState};
use task::{Shell, ShellBuilder, SpawnInTerminal, TaskId};
use terminal::{
    Clear, Event as TerminalEvent, Paste, PasteTrimmed, Search, TaskState, TaskStatus, Terminal,
    TerminalBuilder, TerminalExited, terminal_settings::TerminalSettings,
};
use terminal_view::{
    ChangePaneTheme, ClearClipboard, CopyAndClearSelection, DismissSearch, EditScrollback,
    SavePaneOutput, SearchNextMatch, SearchPreviousMatch, SearchScrollback, SelectAll,
    SelectAllSearchText, SetPaneOverlay, TerminalInput, TerminalView, TerminalViewEvent,
};
use text_edit::{
    ClipboardOutcome, TextField, TextFieldEdit, apply_clipboard_shortcut, apply_text_field_key,
    is_copy_chord, previous_char_boundary,
};
use text_edit_ui::{caret, field_box, field_query_run, field_text_run};
use theme::{
    ActiveTheme, ClientDecorationsExt as _, GlobalTheme, SystemAppearance, Theme, ThemeColors,
    ThemeRegistry,
};
use theme_extensions::{InstalledThemeExtension, ThemeExtension};
use ui::{
    Banner, Button, ButtonCommon as _, ButtonLike, ButtonLink, ButtonSize, ButtonStyle,
    Clickable as _, Color, Icon, IconButton, IconButtonShape, IconName, IconSize, Label, LabelSize,
    PopoverMenu, PopoverMenuHandle, Severity, Tooltip, prelude::*, switch,
};
use util::{ResultExt as _, paths::PathStyle};
use zetta_assets::ZettaAssets;

const ZETTA_MINIMUM_WINDOW_SIZE: Size<Pixels> = size(px(520.), px(320.));

actions!(
    zetta,
    [
        NewTab,
        NewWindow,
        SetDefaultTerminal,
        OpenApplicationMenu,
        ActivateApplicationMenuLeft,
        ActivateApplicationMenuRight,
        CloseTab,
        ToggleTabPinning,
        CloseWindow,
        CloseAllWindows,
        MinimizeWindow,
        HideWindow,
        ZoomWindow,
        ToggleFullscreen,
        OpenThemes,
        OpenKeymap,
        OpenTemplates,
        OpenProjects,
        EditConfigFile,
        EditKeymapFile,
        DetachTab,
        ToggleTabSharing,
        ToggleAutoBackgroundTab,
        ReconnectSession,
        OpenRemoteSession,
        NextTab,
        PreviousTab,
        ToggleTabMoveMode,
        MoveTabLeft,
        MoveTabRight,
        RenameTab,
        ChangeTabIcon,
        ChangeTabTheme,
        ResetPaneTheme,
        ResetTabTheme,
        RenamePane,
        ResetPaneOverlay,
        TogglePaneControls,
        ToggleTabPaneControls,
        ClosePane,
        SplitHorizontalDown,
        SplitHorizontalUp,
        SplitVerticalRight,
        SplitVerticalLeft,
        RotatePaneLayout,
        RotatePaneLayoutCounterClockwise,
        TogglePaneResizeMode,
        ResizePaneLeft,
        ResizePaneRight,
        ResizePaneUp,
        ResizePaneDown,
        TogglePaneMoveMode,
        MovePaneLeft,
        MovePaneRight,
        MovePaneUp,
        MovePaneDown,
        FocusPaneLeft,
        FocusPaneRight,
        FocusPaneUp,
        FocusPaneDown,
        ToggleMaximizePane,
        MinimizePane,
        RestoreMinimizedPane,
        SelectPreviousMinimizedPane,
        SelectNextMinimizedPane,
        ToggleBroadcastInput,
        ToggleSilentMode,
        RequestFocusStatusAccess,
        ToggleTabSilentMode,
        ToggleMultiCommand,
        ToggleStackedCommand,
        SelectPreviousStackedPane,
        SelectNextStackedPane,
        CloseStackedPane,
        IncreaseTerminalFontSize,
        DecreaseTerminalFontSize,
        ResetTerminalFontSize,
        IncreasePaneFontSize,
        DecreasePaneFontSize,
        ResetPaneFontSize,
        SearchTabScrollback,
        ReloadConfiguration,
        ToggleCommandPalette,
        ToggleSettings,
        SaveSettings,
        ToggleSerialConsole,
        StartHttpServer,
        StartTftpServer,
        TogglePerformanceOverlay
    ]
);

fn action_is_enabled_in_build(name: &str) -> bool {
    match name {
        name if name == ToggleSerialConsole.name() => cfg!(feature = "serial-console"),
        name if name == StartHttpServer.name() => cfg!(feature = "http-server"),
        name if name == StartTftpServer.name() => cfg!(feature = "tftp-server"),
        name if name == RequestFocusStatusAccess.name() => cfg!(target_os = "macos"),
        _ => true,
    }
}

#[cfg(target_os = "macos")]
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = zetta, no_json, no_register)]
#[serde(deny_unknown_fields)]
pub(crate) struct OpenProfileWindow {
    pub(crate) profile: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = zetta)]
#[serde(deny_unknown_fields)]
struct ApplyPaneSplitTemplate {
    name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ThemeScope {
    Pane,
    Tab,
}

impl ThemeScope {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Pane => "pane",
            Self::Tab => "tab",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = zetta, no_json, no_register)]
#[serde(deny_unknown_fields)]
struct ApplyTheme {
    name: String,
    scope: ThemeScope,
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = zetta, no_json, no_register)]
#[serde(deny_unknown_fields)]
struct ResetTheme {
    scope: ThemeScope,
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = zetta, no_json, no_register)]
struct SelectOverflowTab {
    index: usize,
}

/// Opens one registered project in a new tab. The root is a machine-specific
/// filesystem path, so this is dispatched from the command palette rather than
/// bound in a keymap; `zetta project open` covers the scripted case.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = zetta, no_json, no_register)]
struct OpenProject {
    root: PathBuf,
}

static PERFORMANCE_OVERLAY_COUNT: AtomicUsize = AtomicUsize::new(0);
static PERFORMANCE_OWNS_FRAME_TRACING: AtomicBool = AtomicBool::new(false);
const PERFORMANCE_SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const FRAME_BUDGET_120_HZ: Duration = Duration::from_nanos(8_333_333);
const FRAME_BUDGET_60_HZ: Duration = Duration::from_nanos(16_666_667);

type ProcessBackgroundSessionEntry = (u64, u64, String, String);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ConfigFileStamp {
    pub(crate) modified: Option<SystemTime>,
    pub(crate) len: u64,
}

struct ZettaProcessState {
    windows: HashMap<WindowId, Entity<Zetta>>,
    dormant: Vec<Entity<Zetta>>,
    runners: HashMap<u64, Entity<Zetta>>,
    next_attention_id: u64,
    silent_mode: SilentModeState,
    background_session_entries: Arc<[ProcessBackgroundSessionEntry]>,
    config: Config,
    config_file_stamp: ConfigFileStamp,
    configuration_error: Option<String>,
    no_mux: bool,
    #[cfg(target_os = "linux")]
    linux_desktop_reassociation_generation: u64,
    control_server: ProcessControlServer,
    _quit_subscription: Subscription,
}

impl Global for ZettaProcessState {}

mod pane;
use pane::*;
mod byte_stream_pane;
mod command_panes;
#[cfg(byte_stream_panes)]
use byte_stream_pane::*;
mod multi_command;
use multi_command::*;
mod multi_command_ui;
mod output_benchmark;
use output_benchmark::*;
mod performance;
use performance::*;
mod command_palette_ui;
mod tab_search;
use tab_search::*;
mod tab_icon_picker;
use tab_icon_picker::*;
mod settings_ui;
mod settings_view;
#[cfg(feature = "http-server")]
use http_server::*;
#[cfg(feature = "serial-console")]
use serial_console::*;
use settings_ui::*;
#[cfg(tftp_enabled)]
use tftp::*;
mod app;
#[cfg(feature = "http-server")]
mod http_server_ui;
#[cfg(feature = "serial-console")]
mod serial_console_ui;
#[cfg(feature = "tftp-server")]
mod tftp_server_ui;
use app::*;
mod configuration_reload;
mod pane_controls;
mod rename;
mod terminal_spawn;
use pane_controls::*;
use terminal_spawn::{StackedTerminalSpawnRequest, TerminalSpawnRequest};
mod pane_resize;
use pane_resize::*;
mod app_render;
mod background_session_ui;
mod cli_service_stubs;
mod close_confirmation_ui;
mod pane_overlay;
mod pane_render;
mod pane_theme_picker;
mod pane_view_state;
mod remote_session_ui;
mod stacked_panes;
mod tab_bar_render;
use tab_bar_render::*;
mod tab_body_render;
mod title_bar_render;
use title_bar_render::*;
mod window_frame;
use window_frame::*;
#[cfg(target_os = "linux")]
mod linux_desktop;
mod view_boundary;
use view_boundary::{ZettaSubview, overlay_boundary, overlay_boundary_root};
mod project_context;
mod startup;
mod worktree_detection;
use project_context::*;
use shell_integration::*;
#[cfg(windows)]
mod windows_integration;
use startup::*;
fn main() {
    if let Err(error) = run() {
        eprintln!("Zetta failed to start: {error:#}");
        std::process::exit(1);
    }
}
