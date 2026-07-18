use super::*;

pub(crate) const RESIZE_HANDLE: Pixels = px(10.);

pub(crate) fn window_close_button_on_left(layout: WindowButtonLayout) -> bool {
    if layout.left.contains(&Some(WindowButton::Close)) {
        true
    } else if layout.right.contains(&Some(WindowButton::Close)) {
        false
    } else {
        cfg!(target_os = "macos")
    }
}

pub(crate) fn system_window_button_layout(cx: &App) -> WindowButtonLayout {
    #[cfg(target_os = "linux")]
    if let Some(layout) = read_gnome_button_layout() {
        return layout;
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        cx.button_layout()
            .unwrap_or_else(WindowButtonLayout::linux_default)
    }

    #[cfg(target_os = "macos")]
    {
        let _ = cx;
        WindowButtonLayout {
            left: [None; MAX_BUTTONS_PER_SIDE],
            right: [None; MAX_BUTTONS_PER_SIDE],
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "freebsd", target_os = "macos")))]
    {
        let _ = cx;
        WindowButtonLayout {
            left: [None; MAX_BUTTONS_PER_SIDE],
            right: [
                Some(WindowButton::Minimize),
                Some(WindowButton::Maximize),
                Some(WindowButton::Close),
            ],
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn read_gnome_button_layout() -> Option<WindowButtonLayout> {
    let output = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.wm.preferences", "button-layout"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    parse_gsettings_button_layout(std::str::from_utf8(&output.stdout).ok()?)
}

#[cfg(target_os = "linux")]
pub(crate) fn parse_gsettings_button_layout(output: &str) -> Option<WindowButtonLayout> {
    let output = output.trim();
    let layout = output
        .strip_prefix('\'')
        .and_then(|output| output.strip_suffix('\''))
        .unwrap_or(output);
    WindowButtonLayout::parse(layout).ok()
}

pub(crate) fn platform_title_bar_height(window: &Window) -> Pixels {
    if cfg!(target_os = "windows") {
        px(32.)
    } else {
        (1.75 * window.rem_size()).max(px(34.))
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn render_window_controls(
    buttons: [Option<WindowButton>; MAX_BUTTONS_PER_SIDE],
    _supported_controls: WindowControls,
    is_maximized: bool,
    right_aligned: bool,
    _client_decorations: bool,
    cx: &App,
) -> AnyElement {
    if !right_aligned || buttons.iter().all(Option::is_none) {
        return div().into_any_element();
    }

    let colors = cx.theme().colors();
    let caption_button = |id, glyph, area, close: bool| {
        let hover_background = if close {
            gpui::rgba(0xe81120ff).into()
        } else {
            colors.ghost_element_hover
        };
        let active_background = if close {
            gpui::rgba(0xe81120cc).into()
        } else {
            colors.ghost_element_active
        };

        h_flex()
            .id(id)
            .h_full()
            .w(px(36.))
            .flex_none()
            .justify_center()
            .content_center()
            .occlude()
            .text_size(px(10.))
            .text_color(colors.text)
            .hover(move |style| {
                if close {
                    style.bg(hover_background).text_color(gpui::white())
                } else {
                    style.bg(hover_background)
                }
            })
            .active(move |style| {
                if close {
                    style
                        .bg(active_background)
                        .text_color(gpui::white().opacity(0.8))
                } else {
                    style.bg(active_background)
                }
            })
            .window_control_area(area)
            .child(glyph)
            .into_any_element()
    };

    h_flex()
        .id("windows-window-controls")
        .h_full()
        .ml_auto()
        .flex_none()
        .font_family("Segoe Fluent Icons")
        .child(caption_button(
            "minimize",
            "\u{e921}",
            WindowControlArea::Min,
            false,
        ))
        .child(caption_button(
            if is_maximized { "restore" } else { "maximize" },
            if is_maximized { "\u{e923}" } else { "\u{e922}" },
            WindowControlArea::Max,
            false,
        ))
        .child(caption_button(
            "close",
            "\u{e8bb}",
            WindowControlArea::Close,
            true,
        ))
        .into_any_element()
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub(crate) fn render_window_controls(
    buttons: [Option<WindowButton>; MAX_BUTTONS_PER_SIDE],
    supported_controls: WindowControls,
    is_maximized: bool,
    right_aligned: bool,
    client_decorations: bool,
    cx: &App,
) -> AnyElement {
    if !client_decorations {
        return div().into_any_element();
    }

    let colors = cx.theme().colors();
    let controls = buttons.into_iter().flatten().filter_map(|button| {
        let (icon, area, enabled) = match button {
            WindowButton::Minimize => (
                IconName::GenericMinimize,
                WindowControlArea::Min,
                supported_controls.minimize,
            ),
            WindowButton::Maximize => (
                if is_maximized {
                    IconName::GenericRestore
                } else {
                    IconName::GenericMaximize
                },
                WindowControlArea::Max,
                supported_controls.maximize,
            ),
            WindowButton::Close => (IconName::GenericClose, WindowControlArea::Close, true),
        };
        enabled.then(|| {
            let action_button = button;
            h_flex()
                .id(button.id())
                .group("")
                .h_5()
                .w_5()
                .flex_none()
                .cursor_pointer()
                .justify_center()
                .content_center()
                .rounded_2xl()
                .hover(move |style| style.bg(colors.ghost_element_hover))
                .active(move |style| style.bg(colors.ghost_element_hover))
                .window_control_area(area)
                .child(
                    svg()
                        .size_4()
                        .flex_none()
                        .path(icon.path())
                        .text_color(colors.icon)
                        .group_hover("", move |style| style.text_color(colors.icon_muted)),
                )
                .on_mouse_move(|_, _, cx| cx.stop_propagation())
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    match action_button {
                        WindowButton::Minimize => window.minimize_window(),
                        WindowButton::Maximize => window.zoom_window(),
                        WindowButton::Close => window.remove_window(),
                    }
                })
                .into_any_element()
        })
    });

    h_flex()
        .id(if right_aligned {
            "right-window-controls"
        } else {
            "left-window-controls"
        })
        .h_full()
        .flex_none()
        .gap_3()
        .px_3()
        .when(right_aligned, |controls| controls.ml_auto())
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .children(controls)
        .into_any_element()
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "freebsd")))]
pub(crate) fn render_window_controls(
    _buttons: [Option<WindowButton>; MAX_BUTTONS_PER_SIDE],
    _supported_controls: WindowControls,
    _is_maximized: bool,
    _right_aligned: bool,
    _client_decorations: bool,
    _cx: &App,
) -> AnyElement {
    div().into_any_element()
}

pub(crate) fn client_window_frame(
    content: impl IntoElement,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let decorations = window.window_decorations();
    let tiling = match decorations {
        Decorations::Server => Tiling::default(),
        Decorations::Client { tiling } => tiling,
    };
    if matches!(decorations, Decorations::Client { .. }) {
        window.set_client_inset(RESIZE_HANDLE);
    }

    div()
        .id("window-frame")
        .size_full()
        .bg(transparent_black())
        .map(|frame| match decorations {
            Decorations::Server => frame,
            Decorations::Client { .. } => frame
                .rounded_client_corners(tiling)
                .when(!tiling.top, |frame| frame.pt(RESIZE_HANDLE))
                .when(!tiling.bottom, |frame| frame.pb(RESIZE_HANDLE))
                .when(!tiling.left, |frame| frame.pl(RESIZE_HANDLE))
                .when(!tiling.right, |frame| frame.pr(RESIZE_HANDLE))
                .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                    let size = window.window_bounds().get_bounds().size;
                    if let Some(edge) = resize_edge(event.position, size, tiling) {
                        window.start_window_resize(edge);
                        cx.stop_propagation();
                    }
                }),
        })
        .child(
            div()
                .size_full()
                .overflow_hidden()
                .border_1()
                .border_color(cx.theme().colors().border)
                .rounded_client_corners(tiling)
                .child(content),
        )
        .when(matches!(decorations, Decorations::Client { .. }), |frame| {
            frame.child(
                canvas(
                    |_bounds, window, _| {
                        window.insert_hitbox(
                            Bounds::new(
                                point(px(0.), px(0.)),
                                window.window_bounds().get_bounds().size,
                            ),
                            HitboxBehavior::Normal,
                        )
                    },
                    move |_bounds, hitbox, window, _| {
                        let Some(edge) = resize_edge(
                            window.mouse_position(),
                            window.window_bounds().get_bounds().size,
                            tiling,
                        ) else {
                            return;
                        };
                        let cursor = match edge {
                            ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
                            ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
                            ResizeEdge::TopLeft | ResizeEdge::BottomRight => {
                                CursorStyle::ResizeUpLeftDownRight
                            }
                            ResizeEdge::TopRight | ResizeEdge::BottomLeft => {
                                CursorStyle::ResizeUpRightDownLeft
                            }
                        };
                        window.set_cursor_style(cursor, &hitbox);
                    },
                )
                .absolute()
                .size_full(),
            )
        })
}

pub(crate) fn resize_edge(
    position: Point<Pixels>,
    window_size: Size<Pixels>,
    tiling: Tiling,
) -> Option<ResizeEdge> {
    let corner = RESIZE_HANDLE * 2.;
    let left = position.x < corner;
    let right = position.x > window_size.width - corner;
    let top = position.y < corner;
    let bottom = position.y > window_size.height - corner;

    if top && left && !tiling.top && !tiling.left {
        Some(ResizeEdge::TopLeft)
    } else if top && right && !tiling.top && !tiling.right {
        Some(ResizeEdge::TopRight)
    } else if bottom && left && !tiling.bottom && !tiling.left {
        Some(ResizeEdge::BottomLeft)
    } else if bottom && right && !tiling.bottom && !tiling.right {
        Some(ResizeEdge::BottomRight)
    } else if position.y < RESIZE_HANDLE && !tiling.top {
        Some(ResizeEdge::Top)
    } else if position.y > window_size.height - RESIZE_HANDLE && !tiling.bottom {
        Some(ResizeEdge::Bottom)
    } else if position.x < RESIZE_HANDLE && !tiling.left {
        Some(ResizeEdge::Left)
    } else if position.x > window_size.width - RESIZE_HANDLE && !tiling.right {
        Some(ResizeEdge::Right)
    } else {
        None
    }
}

#[cfg(test)]
#[path = "tests/window_frame.rs"]
mod tests;
