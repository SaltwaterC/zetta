//! Drawing the title bar: its height and the space it reserves on each
//! platform, which controls and labels fit, and the tab row it holds outside
//! compact mode.

use super::*;
use crate::rename::resolve_tab_title;

mod tabs;
pub(crate) use tabs::*;
mod menus;
pub(crate) use menus::*;
mod controls;
use controls::*;

pub(crate) const TAB_MIN_WIDTH: Pixels = px(80.);
pub(crate) const TAB_MAX_WIDTH: Pixels = px(180.);
// The lifecycle indicator, silent mode, attention, and tab icon, their gaps,
// and the tab's horizontal padding fit in this fixed slot. A pinned tab never
// participates in unpinned overflow.
pub(crate) const PINNED_TAB_WIDTH: Pixels = px(76.);
// Matches the new-tab button's own footprint (ml_1 + a 32px button + mr_2), so a
// trigger reserved at the edge of the tab bar takes up the same visual space.
pub(crate) const TAB_OVERFLOW_TRIGGER_WIDTH: Pixels = px(44.);
pub(crate) const TITLE_BAR_CONTROL_LABEL_MIN_WIDTH: Pixels = px(720.);
pub(crate) const TITLE_BAR_RECONNECT_LABEL_MIN_WIDTH: Pixels = px(800.);
// Compact mode always keeps at least this much of the tab bar draggable, even
// when tabs grow to fill the rest of it, so the window stays movable from there.
pub(crate) const COMPACT_DRAG_AREA_MIN_WIDTH: Pixels = px(60.);
// One Large button (32px) plus the gap_1 (4px) — see the
// `title-bar-controls` reserve this backs on macOS.
pub(crate) const COMPACT_LEADING_CONTROLS_RESERVE: Pixels = px(36.);

/// Whether macOS-specific title-bar space should still be reserved. The
/// fullscreen flag passed here is already qualified to macOS by the caller,
/// so non-macOS platforms keep their existing layout.
pub(crate) fn macos_title_bar_reservations_enabled(is_macos_fullscreen: bool) -> bool {
    cfg!(target_os = "macos") && !is_macos_fullscreen
}

pub(crate) fn compact_leading_controls_reservation_enabled(
    compact_mode: bool,
    is_macos_fullscreen: bool,
) -> bool {
    compact_mode && macos_title_bar_reservations_enabled(is_macos_fullscreen)
}

pub(crate) fn compact_drag_area_visible(compact_mode: bool, is_macos_fullscreen: bool) -> bool {
    compact_mode && !is_macos_fullscreen
}

pub(crate) fn compact_drag_area_reserve_width(show_compact_drag_area: bool) -> Pixels {
    if show_compact_drag_area {
        COMPACT_DRAG_AREA_MIN_WIDTH
    } else {
        px(0.)
    }
}

/// The tab-bar row's height outside compact mode, where the bar is its own row
/// beneath the title bar.
const TAB_BAR_ROW_HEIGHT: gpui::Rems = gpui::Rems(2.);

/// The tab bar's row height: `compact_height` (the platform title bar's own
/// height) in compact mode, since the tab bar shares that row there, or the
/// standard [`TAB_BAR_ROW_HEIGHT`] otherwise. Shared by every element that
/// lines up with the row — the tabs themselves, the overflow triggers, the
/// new-tab button, and the bar's own container — so the compact/regular swap is
/// only written once.
pub(crate) fn tab_bar_row_height(compact_mode: bool, compact_height: Pixels) -> gpui::Div {
    div()
        .h(TAB_BAR_ROW_HEIGHT)
        .when(compact_mode, |el| el.h(compact_height))
}

/// The height of the whole top-of-window chrome: the title bar, plus the
/// tab-bar row beneath it outside compact mode (in compact mode the tab bar
/// shares the title bar's row, so the title bar's height is the whole of it).
///
/// `Zetta::render` needs this as a number rather than letting layout work it
/// out, because the chrome renders inside a cached view and a cached view is
/// laid out from the style the composer supplies rather than measured from its
/// contents. The chrome's own root is `h_full`, so it always fills whatever
/// this returns; getting it wrong moves the panes rather than silently
/// corrupting the cache, which is what
/// `title_bar_chrome_height_matches_the_rows_it_covers` checks.
///
/// The tab-bar row's borders are deliberately *not* added: layout is border-box,
/// so `border_t_1` and `border_b_1` sit inside [`TAB_BAR_ROW_HEIGHT`]. Adding
/// them pushed every pane down by two pixels.
pub(crate) fn title_bar_chrome_height(
    compact_mode: bool,
    title_bar_height: Pixels,
    rem_size: Pixels,
) -> Pixels {
    if compact_mode {
        title_bar_height
    } else {
        title_bar_height + TAB_BAR_ROW_HEIGHT.to_pixels(rem_size)
    }
}

pub(crate) fn title_bar_shows_control_labels(
    viewport_width: Pixels,
    has_reconnect_control: bool,
    hide_labels: bool,
    compact_mode: bool,
) -> bool {
    !compact_mode
        && !hide_labels
        && viewport_width
            >= if has_reconnect_control {
                TITLE_BAR_RECONNECT_LABEL_MIN_WIDTH
            } else {
                TITLE_BAR_CONTROL_LABEL_MIN_WIDTH
            }
}

pub(crate) fn title_bar_buttons_visible(compact_mode: bool, hide_buttons: bool) -> bool {
    !compact_mode && !hide_buttons
}

pub(crate) fn title_bar_keep_running_visible(no_mux: bool, show_title_bar_buttons: bool) -> bool {
    no_mux && show_title_bar_buttons
}

pub(crate) fn title_bar_broadcast_visible(hide_buttons: bool) -> bool {
    !hide_buttons
}

pub(crate) fn title_bar_silent_visible(hide_buttons: bool) -> bool {
    !hide_buttons
}

pub(crate) fn title_bar_silent_icon(silent_mode: bool) -> IconName {
    if silent_mode {
        IconName::BellOff
    } else {
        IconName::Bell
    }
}

pub(crate) fn title_bar_background_indicator_on_right(
    compact_mode: bool,
    hide_buttons: bool,
    background_session_count: usize,
) -> bool {
    (compact_mode || hide_buttons) && background_session_count > 0
}

pub(crate) fn title_bar_pane_size_visible(compact_mode: bool, hide_pane_size: bool) -> bool {
    !compact_mode && !hide_pane_size
}

pub(crate) fn title_bar_app_label_visible(compact_mode: bool) -> bool {
    !compact_mode
}

/// Whether the custom application and profile menus are visible in the title bar.
/// macOS fullscreen temporarily overrides the configured hidden state so the
/// menus remain reachable while the native menu bar is unavailable.
pub(crate) fn title_bar_menus_visible(hide_menus: bool, is_macos_fullscreen: bool) -> bool {
    cfg!(not(target_os = "macos")) || !hide_menus || is_macos_fullscreen
}

pub(crate) fn reconnect_control_label(show_label: bool) -> &'static str {
    if show_label { "Reconnect" } else { "" }
}

/// Compact mode places the tab bar inside the title bar. Keep a portion of it
/// available for moving the window without making tab hitboxes draggable too.
/// This has to follow the tabs and new-tab button (inside the same measured
/// row) rather than sit outside it, so it only claims the width genuinely left
/// over after them instead of pushing them apart from the rest of the bar.
pub(crate) fn render_compact_drag_area(
    compact_height: Pixels,
    zetta_handle: WeakEntity<Zetta>,
) -> AnyElement {
    let down_handle = zetta_handle.clone();
    let up_handle = zetta_handle.clone();
    let out_handle = zetta_handle.clone();
    let click_handle = zetta_handle.clone();
    div()
        .id("compact-title-bar-drag-area")
        .h(compact_height)
        .flex_1()
        .min_w(COMPACT_DRAG_AREA_MIN_WIDTH)
        .window_control_area(WindowControlArea::Drag)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            down_handle
                .update(cx, |this, cx| {
                    this.titlebar_dragging = true;
                    this.focus_active(window, cx);
                })
                .ok();
        })
        .on_mouse_up(MouseButton::Left, move |_, window, cx| {
            up_handle
                .update(cx, |this, cx| {
                    this.titlebar_dragging = false;
                    this.focus_active(window, cx);
                })
                .ok();
        })
        .on_mouse_down_out(move |_, _, cx| {
            out_handle
                .update(cx, |this, _| this.titlebar_dragging = false)
                .ok();
        })
        .on_mouse_move(move |_, window, cx| {
            zetta_handle
                .update(cx, |this, _cx| {
                    if this.titlebar_dragging {
                        this.titlebar_dragging = false;
                        window.start_window_move();
                    }
                })
                .ok();
        })
        .on_click(move |event, window, cx| {
            if event.click_count() == 2
                && let Some(zetta) = click_handle.upgrade()
            {
                cx.stop_propagation();
                zetta.update(cx, |_this, cx| {
                    window.dispatch_action(Box::new(NewTab), cx);
                });
            }
        })
        .into_any_element()
}

/// The title bar plus, outside compact mode, the tab bar that renders as its
/// own row underneath. In compact mode the tab bar is folded into the title bar
/// and `tab_bar` is `None`.
pub(crate) struct TitleBarChrome {
    pub(crate) title_bar: AnyElement,
    pub(crate) tab_bar: Option<AnyElement>,
}

/// The per-frame inputs the title bar renders from, gathered once by
/// [`Zetta::render_title_bar_chrome`]. Named fields rather than a parameter
/// list because eleven of them are adjacent `bool`s: transposing two would be
/// a silent bug that only shows up as a control rendering in the wrong state.
pub(crate) struct TitleBarLayout {
    pub(crate) title_bar_height: Pixels,
    pub(crate) title_bar_background: Hsla,
    pub(crate) rounded_top_left: bool,
    pub(crate) rounded_top_right: bool,
    pub(crate) corner_radius: Pixels,
    pub(crate) compact_mode: bool,
    pub(crate) is_macos_fullscreen: bool,
    /// The tab row, when compact mode folds it into the title bar.
    pub(crate) compact_tab_bar: Option<AnyElement>,
    pub(crate) left_window_controls: AnyElement,
    /// The trailing group: the reconnect indicator that stays beside the
    /// window controls, the application label, and the window controls.
    pub(crate) right_title_bar_controls: AnyElement,
    pub(crate) show_title_bar_menus: bool,
    /// The project sidebar's toggle, left of the application menu; `None`
    /// when `hide_project_sidebar` turns the sidebar off.
    pub(crate) sidebar_toggle: Option<AnyElement>,
    pub(crate) application_menu: AnyElement,
    pub(crate) profile_menu: AnyElement,
    pub(crate) show_title_bar_buttons: bool,
    /// Whether every control that has one renders its text label beside its
    /// icon; the title bar drops the labels as the window narrows.
    pub(crate) show_title_bar_control_labels: bool,
    pub(crate) no_mux: bool,
    pub(crate) auto_background_tab: bool,
    pub(crate) auto_background_protected: bool,
    pub(crate) reconnect_control: Option<AnyElement>,
    pub(crate) show_broadcast_control: bool,
    pub(crate) broadcast_input: bool,
    pub(crate) show_silent_control: bool,
    pub(crate) silent_mode: bool,
    pub(crate) system_silent: bool,
    pub(crate) focus_status_access: FocusStatusAccess,
    pub(crate) active_pane_size: Option<String>,
}

impl Zetta {
    /// Assembles the whole top-of-window chrome: window controls, tab bar,
    /// title-bar menus, and the reconnect/broadcast controls.
    pub(crate) fn render_title_bar_chrome(
        &self,
        frame: &WindowFrameGeometry,
        colors: &ThemeColors,
        handle: &WeakEntity<Self>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> TitleBarChrome {
        let compact_mode = self.launch_config.compact_mode;
        let is_macos_fullscreen = cfg!(target_os = "macos") && window.is_fullscreen();
        let title_bar_height = frame.title_bar_height;
        let active_tab = self.current_tab();
        let broadcast_input = active_tab.is_some_and(|tab| tab.broadcast_input);
        let silent_mode_state = if cx.has_global::<ZettaProcessState>() {
            cx.global::<ZettaProcessState>().silent_mode
        } else {
            SilentModeState::default()
        };
        let focus_status_access = silent_mode_state.focus_status_access();
        let (auto_background_tab, auto_background_protected) = active_tab
            .map(|tab| match &tab.close_policy {
                TabClosePolicy::Background { authentication } => (true, authentication.is_some()),
                TabClosePolicy::Close => (false, false),
            })
            .unwrap_or_default();
        let background_sessions = self.process_background_session_picker_entries(cx);
        let active_pane_size =
            title_bar_pane_size_visible(compact_mode, self.launch_config.hide_pane_size)
                .then(|| {
                    active_tab.and_then(Tab::active_terminal).map(|terminal| {
                        let bounds = terminal.read(cx).last_content().terminal_bounds;
                        terminal_size_label(bounds.num_columns(), bounds.num_lines())
                    })
                })
                .flatten();
        let active_terminal_focus = self.active_terminal_focus(cx);

        let left_window_controls = render_window_controls(
            self.button_layout.left,
            frame.window_control_state,
            false,
            colors,
        );
        let right_window_controls = render_window_controls(
            self.button_layout.right,
            frame.window_control_state,
            true,
            colors,
        );
        let title_bar_background = if cfg!(linux_like) && !window.is_window_active() {
            colors.title_bar_inactive_background
        } else {
            colors.title_bar_background
        };

        let view: Arc<[usize]> = self.tabs_in_view().into();
        let tab_bar = self
            .render_tab_bar(
                TabBarChrome {
                    handle: handle.clone(),
                    compact_mode,
                    title_bar_height,
                    is_macos_fullscreen,
                    rounded_top_right: frame.rounded_top_right,
                    compact_tab_top_left: frame.compact_tab_top_left,
                    compact_tab_top_right: frame.compact_tab_top_right,
                    compact_tab_bottom_left: frame.compact_tab_bottom_left,
                    compact_tab_bottom_right: frame.compact_tab_bottom_right,
                    corner_radius: frame.corner_radius,
                    tab_bar_background: colors.tab_bar_background,
                    compact_leading_background: if self.tab_move_mode {
                        colors.status_bar_background
                    } else {
                        title_bar_background
                    },
                    tab_close_button_on_left: window_close_button_on_left(self.button_layout),
                    no_mux: self.no_mux,
                    is_renaming_tab: self.is_renaming(),
                    tab_count: view.len(),
                    pinned_tab_count: view
                        .iter()
                        .take_while(|index| self.tabs[**index].pinned)
                        .count(),
                    is_renaming_pinned: self.is_renaming()
                        && active_tab.is_some_and(|tab| tab.pinned && tab.renaming_pane.is_none()),
                    selected_tab_index: view
                        .iter()
                        .position(|index| *index == self.active_tab)
                        .unwrap_or_default(),
                    view: view.clone(),
                    tab_move_mode_active: self.tab_move_mode,
                    overflow_selection: self.tab_overflow_selection_side,
                    border_color: colors.border,
                    left_menu_handle: self.tab_overflow_left_menu_handle.clone(),
                    right_menu_handle: self.tab_overflow_right_menu_handle.clone(),
                },
                colors,
            )
            .into_any_element();
        let (compact_tab_bar, regular_tab_bar) = if compact_mode {
            (Some(tab_bar), None)
        } else {
            (None, Some(tab_bar))
        };

        let TitleBarSessionControls {
            show_title_bar_control_labels,
            show_title_bar_buttons,
            reconnect_control,
            right_title_bar_controls,
        } = self.title_bar_session_controls(TitleBarSessionInputs {
            window,
            compact_mode,
            background_sessions: background_sessions.clone(),
            right_window_controls,
            handle,
        });
        let title_bar = self.render_title_bar(
            TitleBarLayout {
                title_bar_height,
                title_bar_background,
                rounded_top_left: frame.rounded_top_left,
                rounded_top_right: frame.rounded_top_right,
                corner_radius: frame.corner_radius,
                compact_mode,
                is_macos_fullscreen,
                compact_tab_bar,
                left_window_controls,
                right_title_bar_controls,
                show_title_bar_menus: title_bar_menus_visible(
                    self.launch_config.hide_title_bar_menus,
                    is_macos_fullscreen,
                ),
                sidebar_toggle: self.tab_groups_enabled().then(|| {
                    crate::sidebar_render::sidebar_toggle_control(
                        self.sidebar.open,
                        show_title_bar_control_labels,
                    )
                }),
                application_menu: self.render_application_menu(
                    show_title_bar_control_labels,
                    handle,
                    active_terminal_focus.clone(),
                ),
                profile_menu: self.render_profile_menu(
                    show_title_bar_control_labels,
                    handle,
                    active_terminal_focus,
                    cx,
                ),
                show_title_bar_buttons,
                show_title_bar_control_labels,
                no_mux: self.no_mux,
                auto_background_tab,
                auto_background_protected,
                reconnect_control,
                show_broadcast_control: title_bar_broadcast_visible(
                    self.launch_config.hide_title_bar_buttons,
                ),
                broadcast_input,
                show_silent_control: title_bar_silent_visible(
                    self.launch_config.hide_title_bar_buttons,
                ),
                silent_mode: silent_mode_state.effective(),
                system_silent: silent_mode_state.system_active(),
                focus_status_access,
                active_pane_size,
            },
            cx,
        );

        TitleBarChrome {
            title_bar,
            tab_bar: regular_tab_bar,
        }
    }

    /// The title bar. Its controls row and the two toggles that need their own
    /// tooltips are built below, so this is the frame and the drag behaviour.
    pub(crate) fn render_title_bar(
        &self,
        mut layout: TitleBarLayout,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let compact_tab_bar = layout.compact_tab_bar.take();
        let left_window_controls =
            std::mem::replace(&mut layout.left_window_controls, div().into_any_element());
        let right_title_bar_controls = std::mem::replace(
            &mut layout.right_title_bar_controls,
            div().into_any_element(),
        );
        let active_pane_size = layout.active_pane_size.take();
        let title_bar_height = layout.title_bar_height;
        let title_bar_background = layout.title_bar_background;
        let rounded_top_left = layout.rounded_top_left;
        let rounded_top_right = layout.rounded_top_right;
        let corner_radius = layout.corner_radius;
        let compact_mode = layout.compact_mode;
        let controls = self.render_title_bar_controls(layout);
        div()
            .id("zetta-title-bar")
            .window_control_area(WindowControlArea::Drag)
            .relative()
            .h(title_bar_height)
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .bg(title_bar_background)
            .when(rounded_top_left, |title_bar| {
                title_bar.rounded_tl(corner_radius)
            })
            .when(rounded_top_right, |title_bar| {
                title_bar.rounded_tr(corner_radius)
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.titlebar_dragging = true;
                    this.focus_active(window, cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.titlebar_dragging = false;
                    this.focus_active(window, cx);
                }),
            )
            .on_mouse_down_out(cx.listener(|this, _, _, _| this.titlebar_dragging = false))
            .on_mouse_move(cx.listener(|this, _, window, _| {
                if this.titlebar_dragging {
                    this.titlebar_dragging = false;
                    window.start_window_move();
                }
            }))
            .on_click(|event, window, _| {
                if event.click_count() == 2 {
                    if cfg!(target_os = "macos") {
                        window.titlebar_double_click();
                    } else if window.is_resizable() {
                        window.zoom_window();
                    }
                }
            })
            .child(left_window_controls)
            .child(controls)
            .when_some(compact_tab_bar, |title_bar, tab_bar| {
                title_bar.child(tab_bar)
            })
            .when(!compact_mode, |title_bar| {
                title_bar.child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .debug_selector(|| "title-bar-middle".to_owned()),
                )
            })
            .when_some(active_pane_size, |title_bar, active_pane_size| {
                title_bar.child(
                    div().flex_none().px_2().child(
                        Label::new(active_pane_size)
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
                )
            })
            .child(right_title_bar_controls)
            .into_any_element()
    }
}

#[cfg(test)]
#[path = "tests/title_bar_render.rs"]
mod tests;
