use super::*;
use crate::configuration_reload::CONFIGURATION_RELOAD_SUCCESS_MESSAGE;

// GPUI's standard popover menus use priority 1. Keep every local deferred draw on
// an explicit rung around that fixed priority so title-bar repairs stay below
// menus, blocking overlays stay above them, and popups hosted by a modal stay on
// top of their deferred parent.
pub(crate) const TITLE_BAR_CONTROL_PAINT_PRIORITY: usize = 0;
pub(crate) const TITLE_BAR_POPOVER_PAINT_PRIORITY: usize = 1;
pub(crate) const MODAL_OVERLAY_PAINT_PRIORITY: usize = 2;
pub(crate) const MODAL_POPUP_PAINT_PRIORITY: usize = 3;

const _: () = assert!(TITLE_BAR_CONTROL_PAINT_PRIORITY < TITLE_BAR_POPOVER_PAINT_PRIORITY);
const _: () = assert!(TITLE_BAR_POPOVER_PAINT_PRIORITY < MODAL_OVERLAY_PAINT_PRIORITY);
const _: () = assert!(MODAL_OVERLAY_PAINT_PRIORITY < MODAL_POPUP_PAINT_PRIORITY);

fn modal_overlay(overlay: Option<AnyElement>) -> Option<AnyElement> {
    overlay.map(|overlay| {
        deferred(overlay)
            .with_priority(MODAL_OVERLAY_PAINT_PRIORITY)
            .into_any_element()
    })
}

impl Zetta {}

/// Every floating layer rendered above the tab body, in paint order.
///
/// Built in one pass before the window content is composed, because each entry
/// borrows the entity while it reads the state that drives it.
struct ZettaOverlays {
    notice: Option<AnyElement>,
    performance: Option<AnyElement>,
    palette: Option<AnyElement>,
    multi_command: Option<AnyElement>,
    tab_search: Option<AnyElement>,
    settings: Option<AnyElement>,
    tab_icon_picker: Option<AnyElement>,
    theme_picker: Option<AnyElement>,
    overlay_style_picker: Option<AnyElement>,
    serial_console: Option<AnyElement>,
    remote_session: Option<AnyElement>,
    session_authentication: Option<AnyElement>,
    close_confirmation: Option<AnyElement>,
}

impl Zetta {
    fn project_offer_banner(root: &Path, action_slot: impl IntoElement) -> Banner {
        Banner::new()
            .severity(Severity::Warning)
            .wrap_content(true)
            .child(
                Label::new(format!(
                    "Zetta project configuration found in {}. Add this project? Its pane layouts may run commands.",
                    root.display()
                ))
                .size(LabelSize::Small)
                .line_clamp(3),
            )
            .action_slot(action_slot)
    }

    fn render_overlays(
        &mut self,
        colors: &ThemeColors,
        error_color: Hsla,
        handle: &WeakEntity<Self>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ZettaOverlays {
        let entity = cx.entity();
        #[cfg(feature = "serial-console")]
        let serial_console = self.render_serial_console_overlay(cx);
        #[cfg(not(feature = "serial-console"))]
        let serial_console: Option<AnyElement> = None;

        ZettaOverlays {
            notice: self.render_transient_notice_overlay(colors),
            performance: self.render_performance_overlay(colors, window),
            palette: modal_overlay(self.render_command_palette_overlay(colors, handle, cx)),
            multi_command: modal_overlay(self.render_multi_command_overlay(
                colors,
                error_color,
                handle,
            )),
            tab_search: self.render_tab_search_overlay(colors),
            // Both are rendered inside their own cached view: scrolling or
            // hovering one notifies that view instead of `Zetta`, so the other
            // one — and the window column — is reused for the frame. Each fills
            // the window, matching the `absolute inset_0` backdrop it renders.
            settings: modal_overlay(self.settings_editor.is_some().then(|| {
                overlay_boundary(ZettaSubview::get_or_insert(
                    &mut self.settings_surface_view,
                    render_settings_boundary,
                    &entity,
                    cx,
                ))
                .into_any_element()
            })),
            tab_icon_picker: modal_overlay(self.tab_icon_picker.is_some().then(|| {
                overlay_boundary(ZettaSubview::get_or_insert(
                    &mut self.tab_icon_picker_view,
                    render_tab_icon_picker_boundary,
                    &entity,
                    cx,
                ))
                .into_any_element()
            })),
            theme_picker: modal_overlay(self.render_pane_theme_picker_overlay(colors, handle, cx)),
            overlay_style_picker: modal_overlay(
                self.render_overlay_style_picker_overlay(window, cx),
            ),
            serial_console: modal_overlay(serial_console),
            remote_session: modal_overlay(self.render_remote_session_overlay(
                colors,
                error_color,
                handle,
            )),
            session_authentication: modal_overlay(self.render_session_authentication_overlay(cx)),
            close_confirmation: modal_overlay(self.render_tab_close_confirmation_overlay(cx)),
        }
    }

    /// A short-lived message, floating over the content rather than sharing the
    /// column with it.
    ///
    /// Deliberately not a banner in the feedback column. A banner there takes
    /// vertical space, so showing one reflows every terminal in the window — and
    /// for a *shared* pane that reflow is published: the pane reports its smaller
    /// grid, the multiplexer arbitrates every viewer down to the smallest of them,
    /// and the other window is resized to match. Telling the user their tab can now
    /// be joined moved their windows, which is a remarkable amount of damage for a
    /// message that takes itself away again after a few seconds.
    fn render_transient_notice_overlay(&self, colors: &ThemeColors) -> Option<AnyElement> {
        let notice = self.transient_notice.message()?;
        // Styled like the resize- and move-mode labels rather than as a `Banner`.
        // A `Banner` is built to sit on the feedback column's own background and
        // carries a translucent one of its own; floating it over a terminal left
        // the text competing with whatever the shell had drawn underneath. The
        // mode labels solve exactly this problem — an opaque status-bar
        // background and the theme's plain text colour — so this borrows their
        // answer.
        Some(
            div()
                .absolute()
                .bottom(px(12.))
                .right(px(12.))
                .max_w(px(420.))
                .px_2()
                .py_1()
                .rounded_sm()
                .border_1()
                .border_color(colors.border)
                .bg(colors.status_bar_background)
                .text_sm()
                .text_color(colors.text)
                .shadow_sm()
                .child(notice.to_owned())
                .into_any_element(),
        )
    }

    /// Registers every window-level action handler on the root element.
    fn register_actions(content: gpui::Div, cx: &mut Context<Self>) -> gpui::Div {
        content
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::new_window))
            .on_action(cx.listener(Self::set_default_terminal))
            .on_action(cx.listener(Self::open_application_menu))
            .on_action(cx.listener(Self::activate_application_menu_left))
            .on_action(cx.listener(Self::activate_application_menu_right))
            .on_action(cx.listener(Self::open_profile))
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::toggle_tab_pinning))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::close_all_windows))
            .on_action(cx.listener(Self::minimize_window))
            .on_action(cx.listener(Self::hide_window))
            .on_action(cx.listener(Self::zoom_window))
            .on_action(cx.listener(Self::toggle_fullscreen))
            .on_action(cx.listener(Self::open_themes))
            .on_action(cx.listener(Self::open_keymap))
            .on_action(cx.listener(Self::open_templates))
            .on_action(cx.listener(Self::open_projects))
            .on_action(cx.listener(Self::edit_config_file))
            .on_action(cx.listener(Self::edit_keymap_file))
            .on_action(cx.listener(Self::detach_tab))
            .on_action(cx.listener(Self::toggle_tab_sharing))
            .on_action(cx.listener(Self::toggle_auto_background_tab))
            .on_action(cx.listener(Self::reconnect_session))
            .on_action(cx.listener(Self::open_remote_session))
            .on_action(cx.listener(Self::close_active_pane))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::previous_tab))
            .on_action(cx.listener(Self::select_overflow_tab))
            .on_action(cx.listener(Self::toggle_tab_move_mode))
            .on_action(cx.listener(Self::move_tab_left))
            .on_action(cx.listener(Self::move_tab_right))
            .on_action(cx.listener(Self::rename_tab))
            .on_action(cx.listener(Self::change_tab_icon))
            .on_action(cx.listener(Self::change_pane_theme))
            .on_action(cx.listener(Self::change_tab_theme))
            .on_action(cx.listener(Self::apply_theme))
            .on_action(cx.listener(Self::reset_theme))
            .on_action(cx.listener(Self::reset_pane_theme))
            .on_action(cx.listener(Self::reset_tab_theme))
            .on_action(cx.listener(Self::rename_pane))
            .on_action(cx.listener(Self::set_pane_overlay))
            .on_action(cx.listener(Self::reset_pane_overlay))
            .on_action(cx.listener(Self::toggle_pane_controls))
            .on_action(cx.listener(Self::toggle_tab_pane_controls))
            .on_action(cx.listener(Self::split_horizontal_down))
            .on_action(cx.listener(Self::split_horizontal_up))
            .on_action(cx.listener(Self::split_vertical_right))
            .on_action(cx.listener(Self::split_vertical_left))
            .on_action(cx.listener(Self::rotate_pane_layout))
            .on_action(cx.listener(Self::rotate_pane_layout_counter_clockwise))
            .on_action(cx.listener(Self::toggle_pane_resize_mode))
            .on_action(cx.listener(Self::resize_pane_left))
            .on_action(cx.listener(Self::resize_pane_right))
            .on_action(cx.listener(Self::resize_pane_up))
            .on_action(cx.listener(Self::resize_pane_down))
            .on_action(cx.listener(Self::toggle_pane_move_mode))
            .on_action(cx.listener(Self::move_pane_left))
            .on_action(cx.listener(Self::move_pane_right))
            .on_action(cx.listener(Self::move_pane_up))
            .on_action(cx.listener(Self::move_pane_down))
            .on_action(cx.listener(Self::apply_pane_split_template))
            .on_action(cx.listener(Self::open_project))
            .on_action(cx.listener(Self::focus_pane_left))
            .on_action(cx.listener(Self::focus_pane_right))
            .on_action(cx.listener(Self::focus_pane_up))
            .on_action(cx.listener(Self::focus_pane_down))
            .on_action(cx.listener(Self::toggle_maximize_pane))
            .on_action(cx.listener(Self::minimize_pane))
            .on_action(cx.listener(Self::restore_minimized_pane))
            .on_action(cx.listener(Self::select_previous_minimized_pane))
            .on_action(cx.listener(Self::select_next_minimized_pane))
            .on_action(cx.listener(Self::toggle_broadcast_input))
            .on_action(cx.listener(Self::toggle_silent_mode))
            .on_action(cx.listener(Self::request_focus_status_access))
            .on_action(cx.listener(Self::toggle_tab_silent_mode))
            .on_action(cx.listener(Self::toggle_multi_command))
            .on_action(cx.listener(Self::toggle_stacked_command))
            .on_action(cx.listener(Self::select_previous_stacked_pane))
            .on_action(cx.listener(Self::select_next_stacked_pane))
            .on_action(cx.listener(Self::close_stacked_pane))
            .on_action(cx.listener(Self::increase_terminal_font_size))
            .on_action(cx.listener(Self::decrease_terminal_font_size))
            .on_action(cx.listener(Self::reset_terminal_font_size))
            .on_action(cx.listener(Self::increase_pane_font_size))
            .on_action(cx.listener(Self::decrease_pane_font_size))
            .on_action(cx.listener(Self::reset_pane_font_size))
            .on_action(cx.listener(Self::save_pane_output))
            .on_action(cx.listener(Self::search_tab_scrollback))
            .on_action(cx.listener(Self::reload_configuration))
            .on_action(cx.listener(Self::toggle_command_palette))
            .on_action(cx.listener(Self::toggle_settings))
            .on_action(cx.listener(Self::save_settings_action))
            .on_action(cx.listener(Self::toggle_serial_console))
            .on_action(cx.listener(Self::start_http_server))
            .on_action(cx.listener(Self::start_tftp_server))
            .on_action(cx.listener(Self::toggle_performance_overlay))
    }

    /// The feedback banners shown between the tab bar and the tab body.
    fn render_feedback_banners(
        &self,
        content: gpui::Div,
        colors: &ThemeColors,
        handle: &WeakEntity<Zetta>,
    ) -> gpui::Div {
        let banner = |error: String| {
            Banner::new()
                .severity(Severity::Error)
                .child(Label::new(error).size(LabelSize::Small).line_clamp(3))
        };
        let feedback_row = |banner: Banner| {
            div()
                .px_2()
                .py_1()
                .when(cfg!(linux_like), |row| row.bg(colors.editor_background))
                .child(banner)
        };
        content
            .when_some(self.projects.offer.clone(), |content, offer| {
                let add_handle = handle.clone();
                let dismiss_handle = handle.clone();
                content.child(feedback_row(Self::project_offer_banner(
                    &offer.root,
                    h_flex()
                        .flex_none()
                        .gap_1()
                        .child(
                            Button::new("dismiss-project-offer", "Dismiss")
                                .style(ButtonStyle::Outlined)
                                .color(Color::Custom(colors.text))
                                .on_click(move |_, _, cx| {
                                    dismiss_handle
                                        .update(cx, |this, cx| this.dismiss_project_offer(cx))
                                        .ok();
                                }),
                        )
                        .child(
                            Button::new("accept-project-offer", "Add project")
                                .style(ButtonStyle::Filled)
                                .color(Color::Custom(colors.text))
                                .on_click(move |_, window, cx| {
                                    add_handle
                                        .update(cx, |this, cx| {
                                            this.accept_project_offer(window, cx);
                                        })
                                        .ok();
                                }),
                        ),
                )))
            })
            .when(self.configuration_reload_feedback.is_visible(), |content| {
                content.child(feedback_row(
                    Banner::new().severity(Severity::Success).child(
                        Label::new(CONFIGURATION_RELOAD_SUCCESS_MESSAGE).size(LabelSize::Small),
                    ),
                ))
            })
            .when_some(self.configuration_error.clone(), |content, error| {
                content.child(feedback_row(
                    banner(error).action_slot(
                        IconButton::new("reload-invalid-configuration", IconName::RotateCw)
                            .shape(IconButtonShape::Square)
                            .icon_size(IconSize::Small)
                            .icon_color(Color::Custom(colors.icon))
                            .aria_label("Reload configuration")
                            .tooltip(Tooltip::text("Reload configuration"))
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(ReloadConfiguration), cx);
                            }),
                    ),
                ))
            })
            .when_some(self.pane_output_error.clone(), |content, error| {
                content.child(feedback_row(banner(error)))
            })
    }

    /// The chrome and the tab body, in the column they share.
    ///
    /// The title bar and tab bar, the banners, and the tab body, stacked.
    ///
    /// The chrome is cached in its own view while the body is not, which is the
    /// whole point of the arrangement: a frame the terminal caused marks the
    /// body's ancestors dirty but leaves the chrome — a sibling — untouched, so
    /// output no longer rebuilds the title bar and tab bar sixty times a second.
    /// Caching the column as a whole cannot do this, and is in fact worse than
    /// not caching at all: a cached view that misses re-renders its subtree with
    /// `Window::refreshing` set, which suppresses every cache nested under it.
    fn render_window_column(
        &mut self,
        colors: &ThemeColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let frame = WindowFrameGeometry::new(window, cx);
        let entity = cx.entity();
        let handle = entity.downgrade();
        let chrome_height = title_bar_chrome_height(
            self.launch_config.compact_mode,
            frame.title_bar_height,
            window.rem_size(),
        );
        let chrome = ZettaSubview::get_or_insert(
            &mut self.title_bar_chrome_view,
            render_title_bar_chrome_boundary,
            &entity,
            cx,
        )
        .cached(
            gpui::StyleRefinement::default()
                .w_full()
                .flex_none()
                .h(chrome_height),
        );
        let body = self.render_tab_body(
            window,
            frame.rounded_bottom_left,
            frame.rounded_bottom_right,
            frame.corner_radius,
            cx,
        );
        let column = div().size_full().flex().flex_col().child(chrome);
        self.render_feedback_banners(column, colors, &handle)
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }

    /// Stacks the window column and the overlays into the root element.
    fn compose_window_content(
        &self,
        column: AnyElement,
        overlays: ZettaOverlays,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let content = div()
            .key_context("Zetta")
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .when(!cfg!(linux_like), |content| {
                content.bg(colors.editor_background)
            });
        let content = Self::register_actions(content, cx)
            .when(
                self.is_renaming() || self.is_editing_pane_overlay(),
                |content| content.track_focus(&self.rename_focus),
            )
            .when(self.is_picking_overlay_style(), |content| {
                content.track_focus(&self.overlay_style_focus)
            })
            .when(self.tab_icon_picker.is_some(), |content| {
                content.track_focus(&self.tab_icon_picker_focus)
            })
            .when(self.theme_picker.is_some(), |content| {
                content.track_focus(&self.theme_picker_focus)
            })
            .when(self.close_tab_confirmation.is_some(), |content| {
                content.track_focus(&self.close_confirmation_focus)
            })
            .when(self.remote_session_picker.is_some(), |content| {
                content.track_focus(&self.remote_session_focus)
            })
            .capture_key_up(cx.listener(Self::pane_resize_key_up))
            .on_key_down(cx.listener(Self::command_palette_key_down))
            .child(column);

        // Child order preserves the existing relative order within each paint
        // priority; later overlays on the same rung sit above earlier ones.
        [
            overlays.notice,
            overlays.performance,
            overlays.palette,
            overlays.multi_command,
            overlays.tab_search,
            overlays.settings,
            overlays.tab_icon_picker,
            overlays.theme_picker,
            overlays.overlay_style_picker,
            overlays.serial_console,
            overlays.remote_session,
            overlays.session_authentication,
            overlays.close_confirmation,
        ]
        .into_iter()
        .flatten()
        .fold(content, |content, overlay| content.child(overlay))
    }
}

fn render_settings_page_boundary(
    zetta: &mut Zetta,
    _window: &mut Window,
    cx: &mut Context<Zetta>,
) -> AnyElement {
    div()
        .size_full()
        .relative()
        .children(zetta.render_settings_page_region(cx))
        .into_any_element()
}

/// Adapters that give `ZettaSubview` a plain function pointer per boundary.
///
/// The root is `size_full` so the chrome fills exactly the box
/// `title_bar_chrome_height` reserved for it in `render_window_column`.
fn render_title_bar_chrome_boundary(
    zetta: &mut Zetta,
    window: &mut Window,
    cx: &mut Context<Zetta>,
) -> AnyElement {
    let colors = zetta.window_theme(cx).colors().clone();
    let handle = cx.entity().downgrade();
    let frame = WindowFrameGeometry::new(window, cx);
    let chrome = zetta.render_title_bar_chrome(&frame, &colors, &handle, window, cx);
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(chrome.title_bar)
        .when_some(chrome.tab_bar, |column, tab_bar| column.child(tab_bar))
        .into_any_element()
}

fn render_settings_boundary(
    zetta: &mut Zetta,
    window: &mut Window,
    cx: &mut Context<Zetta>,
) -> AnyElement {
    overlay_boundary_root(zetta.render_settings_overlay(window, cx))
}

fn render_tab_icon_picker_boundary(
    zetta: &mut Zetta,
    window: &mut Window,
    cx: &mut Context<Zetta>,
) -> AnyElement {
    overlay_boundary_root(zetta.render_tab_icon_picker_overlay(window, cx))
}

impl Zetta {
    /// The settings page, wrapped in the cached boundary that keeps it out of
    /// frames driven by a modal or a dropdown popup above it.
    pub(crate) fn settings_page_region_element(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let entity = cx.entity();
        ZettaSubview::get_or_insert(
            &mut self.settings_page_view,
            render_settings_page_boundary,
            &entity,
            cx,
        )
        .cached(gpui::StyleRefinement::default().flex_1().min_h_0().w_full())
        .into_any_element()
    }
}

impl Render for Zetta {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Native mouse resizing is constrained by WindowOptions where the
        // platform supports it. Keep it consistent with resize mode if a
        // compositor reports an undersized bound anyway.
        crate::app::enforce_minimum_window_size(window);
        self.sync_visible_terminals(cx);

        let theme = self.window_theme(cx);
        // Zetta's own elements take `colors` explicitly, but Zed's `ui` components
        // — every `Button`/`IconButton`, `Label`, `switch`, `Banner`, and, with no
        // override API at all, every `Tooltip` and `ContextMenu` — resolve their
        // colors from `cx.theme()`. Without this they keep rendering the launch
        // configuration's theme inside a project that selects a different one.
        //
        // Doing it per frame rather than when the project changes is what keeps
        // this correct with several windows open: each window installs its own
        // theme immediately before building the elements that read it, and
        // tooltips and popovers build during that same frame. `update_theme` is a
        // plain global mutation, so it cannot re-enter this render.
        if !Arc::ptr_eq(GlobalTheme::theme(cx), &theme) {
            GlobalTheme::update_theme(cx, theme.clone());
        }
        let colors = theme.colors().clone();
        let error_color = theme.status().error;
        let handle = cx.entity().downgrade();

        // The column itself is composed here rather than behind a boundary of
        // its own: every frame reaches it anyway, and wrapping it in a cache
        // that always misses would suppress the caches inside it.
        let column = self.render_window_column(&colors, window, cx);
        let overlays = self.render_overlays(&colors, error_color, &handle, window, cx);

        let content = self.compose_window_content(column, overlays, &colors, cx);
        client_window_frame(content, window, colors.border)
    }
}

#[cfg(test)]
#[path = "tests/app_render.rs"]
mod tests;
