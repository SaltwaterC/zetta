#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod command_palette;
mod config;
mod settings_editor;
mod theme_extensions;
mod zetta_assets;

const ZETTA_APP_ID: &str = "Zetta";
const ZETTA_DEFAULT_THEME: &str = "One Light";

use std::{
    collections::{HashMap, VecDeque},
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
use command_palette::{CommandPalette, PaletteCommand, humanize_action_name};
use config::{Config, PaneSplitAxis, PaneSplitTemplate, Profile};
use gpui::{
    Action, Anchor, AnyElement, App, AppContext as _, Bounds, Context, CursorStyle, Decorations,
    Entity, Focusable, FrameTiming, FrameTimingCollector, HitboxBehavior, InteractiveElement as _,
    IntoElement, KeyBinding, KeyDownEvent, MAX_BUTTONS_PER_SIDE, MouseButton, Pixels, Point,
    Render, ResizeEdge, ScrollHandle, SharedString, Size, Subscription, Task, Tiling,
    TitlebarOptions, UniformListScrollHandle, Window, WindowBackgroundAppearance, WindowBounds,
    WindowButton, WindowButtonLayout, WindowControlArea, WindowControls, WindowDecorations,
    WindowId, WindowOptions, actions, canvas, div, point, profiler, px, size, svg,
    transparent_black, uniform_list,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settings::{KeymapFile, KeymapFileLoadResult, Settings as _};
use settings_editor::{
    BindingForm, ConfigTextField, ConfigurationForm, KeymapForm, KeymapSectionForm,
    KeymapTextField, SettingsPage, TextField, save as save_settings_file,
};
use task::Shell;
use terminal::{
    Paste, PasteTrimmed, Range, Search, TerminalBuilder, terminal_settings::TerminalSettings,
};
use terminal_view::{
    ClearClipboard, CopyAndClearSelection, DismissSearch, SearchNextMatch, SearchPreviousMatch,
    SearchScrollback, SelectAll, SelectAllSearchText, TerminalInput, TerminalView,
    TerminalViewEvent,
};
use theme::{
    ActiveTheme, ClientDecorationsExt as _, GlobalTheme, Theme, ThemeColors, ThemeRegistry,
};
use theme_extensions::{InstalledThemeExtension, ThemeExtension};
use ui::{
    Banner, ButtonCommon as _, ButtonLike, ButtonSize, ButtonStyle, Clickable as _, Color, Icon,
    IconButton, IconButtonShape, IconName, IconPosition, IconSize, Label, LabelSize, PopoverMenu,
    Severity, Tooltip, prelude::*,
};
use util::{ResultExt as _, paths::PathStyle};
use zetta_assets::ZettaAssets;

actions!(
    zetta,
    [
        NewTab,
        NewWindow,
        CloseTab,
        NextTab,
        PreviousTab,
        RenameTab,
        SplitHorizontal,
        SplitVertical,
        FocusPaneLeft,
        FocusPaneRight,
        FocusPaneUp,
        FocusPaneDown,
        ToggleBroadcastInput,
        ToggleMultiCommand,
        IncreaseTerminalFontSize,
        DecreaseTerminalFontSize,
        ResetTerminalFontSize,
        IncreasePaneFontSize,
        DecreasePaneFontSize,
        ResetPaneFontSize,
        SavePaneOutput,
        SearchTabScrollback,
        ReloadConfiguration,
        ToggleCommandPalette,
        ToggleSettings,
        TogglePerformanceOverlay
    ]
);

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
mod pane;
use pane::*;
mod multi_command;
use multi_command::*;
mod multi_command_ui;
mod performance;
use performance::*;
mod command_palette_ui;
mod tab_search;
use tab_search::*;
mod settings_ui;
mod settings_view;
use settings_ui::*;
mod app;
use app::*;
mod app_render;
mod window_frame;
use window_frame::*;
mod startup;
use startup::*;
fn main() {
    if let Err(error) = run() {
        eprintln!("Zetta failed to start: {error:#}");
        std::process::exit(1);
    }
}
