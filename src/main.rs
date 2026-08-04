#![cfg_attr(windows, windows_subsystem = "console")]

mod background_sessions;
mod cli_services;
mod command_palette;
mod config;
#[cfg(feature = "http-server")]
mod http_server;
#[cfg(feature = "notifications")]
mod notification_sounds;
mod process_control;
#[cfg(feature = "serial-console")]
mod serial_console;
#[cfg(any(feature = "http-server", feature = "tftp-server"))]
mod server_ui;
mod session_auth_ui;
mod session_cli;
mod settings_editor;
mod shell_integration;
#[cfg(any(feature = "tftp-server", feature = "tftp-client"))]
mod tftp;
mod theme_extensions;
#[cfg(feature = "syntax-highlighting")]
mod vi_syntax;
mod zetta_assets;

const ZETTA_APP_ID: &str = "Zetta";
const ZETTA_DEFAULT_THEME: &str = "One Light";

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
    BackgroundPaneLayout, BackgroundPaneState, BackgroundPaneSummary, BackgroundSessionRunner,
    BackgroundSessionSummary, SessionAuthentication, application_from_command_line,
    print_session_catalogs,
};
use command_palette::{CommandPalette, PaletteCommand, humanize_action_name};
use config::{
    Config, NewTabProfile, PaneControlsPosition, PaneSplitAxis, PaneSplitTemplate, Profile,
    WorkingDirectoryScope, profile_is_hidden, visible_profile_count, visible_profile_index,
};
use futures::StreamExt as _;
use gpui::{
    Action, Anchor, AnchoredPositionMode, AnyElement, App, AppContext as _, Bounds, Context,
    CursorStyle, Decorations, DismissEvent, Entity, Focusable, FrameTiming, FrameTimingCollector,
    Global, HitboxBehavior, InteractiveElement as _, IntoElement, KeyBinding,
    KeyBindingContextPredicate, KeyDownEvent, KeyUpEvent, KeybindingKeystroke, ListSizingBehavior,
    MAX_BUTTONS_PER_SIDE, MouseButton, Pixels, PlatformKeyboardMapper, Point, Render, ResizeEdge,
    ScrollHandle, SharedString, Size, Subscription, Task, Tiling, TitlebarOptions,
    UniformListScrollHandle, Window, WindowBackgroundAppearance, WindowBounds, WindowButton,
    WindowButtonLayout, WindowControlArea, WindowControls, WindowDecorations, WindowId,
    WindowOptions, actions, anchored, canvas, container_query, deferred, div, point, profiler, px,
    size, svg, transparent_black, uniform_list,
};
use process_control::{
    ProcessControlCommand, ProcessControlServer, ReconnectSessionResult,
    request_existing_process_window,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use session_auth_ui::SessionAuthenticationPrompt;
use settings::{KeymapFile, KeymapFileLoadResult, Settings as _};
use settings_editor::{
    BindingForm, ConfigTextField, ConfigurationForm, KeymapForm, KeymapSectionForm,
    KeymapTextField, SettingsPage, TextField, save as save_settings_file,
};
use task::Shell;
use terminal::{
    Clear, Event as TerminalEvent, Paste, PasteTrimmed, Search, Terminal, TerminalBuilder,
    terminal_settings::TerminalSettings,
};
use terminal_view::{
    ClearClipboard, CopyAndClearSelection, DismissSearch, EditScrollback, SavePaneOutput,
    SearchNextMatch, SearchPreviousMatch, SearchScrollback, SelectAll, SelectAllSearchText,
    TerminalInput, TerminalView, TerminalViewEvent,
};
use theme::{
    ActiveTheme, ClientDecorationsExt as _, GlobalTheme, Theme, ThemeColors, ThemeRegistry,
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
        OpenApplicationMenu,
        ActivateApplicationMenuLeft,
        ActivateApplicationMenuRight,
        CloseTab,
        CloseWindow,
        CloseAllWindows,
        MinimizeWindow,
        ZoomWindow,
        OpenThemes,
        OpenKeymap,
        DetachTab,
        ToggleAutoBackgroundTab,
        ReconnectSession,
        NextTab,
        PreviousTab,
        RenameTab,
        RenamePane,
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
        ToggleMultiCommand,
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
        _ => true,
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = zetta)]
#[serde(deny_unknown_fields)]
struct ApplyPaneSplitTemplate {
    name: String,
}

static PERFORMANCE_OVERLAY_COUNT: AtomicUsize = AtomicUsize::new(0);
static PERFORMANCE_OWNS_FRAME_TRACING: AtomicBool = AtomicBool::new(false);
const PERFORMANCE_SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const FRAME_BUDGET_120_HZ: Duration = Duration::from_nanos(8_333_333);
const FRAME_BUDGET_60_HZ: Duration = Duration::from_nanos(16_666_667);

type ProcessBackgroundSessionEntry = (u64, u64, String, String);

struct ZettaProcessState {
    windows: HashMap<WindowId, Entity<Zetta>>,
    dormant: Vec<Entity<Zetta>>,
    runners: HashMap<u64, Entity<Zetta>>,
    background_session_entries: Arc<[ProcessBackgroundSessionEntry]>,
    config: Config,
    configuration_error: Option<String>,
    control_server: ProcessControlServer,
    _quit_subscription: Subscription,
}

impl Global for ZettaProcessState {}

mod pane;
use pane::*;
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
mod settings_ui;
mod settings_view;
#[cfg(feature = "http-server")]
use http_server::*;
#[cfg(feature = "serial-console")]
use serial_console::*;
use settings_ui::*;
#[cfg(any(feature = "tftp-server", feature = "tftp-client"))]
use tftp::*;
mod app;
#[cfg(feature = "http-server")]
mod http_server_ui;
#[cfg(feature = "serial-console")]
mod serial_console_ui;
#[cfg(feature = "tftp-server")]
mod tftp_server_ui;
use app::*;
mod app_render;
mod window_frame;
use window_frame::*;
mod startup;
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
