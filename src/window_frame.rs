use super::*;

const BORDER_SIZE: Pixels = px(1.);
/// The client-decoration inset used for rounded corners and the compositor shadow.
const CLIENT_FRAME_INSET: Pixels = px(10.);

const fn custom_window_border_enabled() -> bool {
    !cfg!(target_os = "windows")
}

pub(crate) fn window_close_button_on_left(layout: WindowButtonLayout) -> bool {
    layout.left.contains(&Some(WindowButton::Close))
        || (!layout.right.contains(&Some(WindowButton::Close)) && cfg!(target_os = "macos"))
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

#[derive(Clone, Copy)]
pub(crate) struct WindowControlState {
    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
    pub(crate) supported_controls: WindowControls,
    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
    pub(crate) is_maximized: bool,
    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
    pub(crate) is_resizable: bool,
    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
    pub(crate) is_minimizable: bool,
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub(crate) client_decorations: bool,
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
fn window_button_enabled(
    button: WindowButton,
    supported_controls: WindowControls,
    is_resizable: bool,
    is_minimizable: bool,
) -> bool {
    match button {
        WindowButton::Minimize => supported_controls.minimize && is_minimizable,
        WindowButton::Maximize => supported_controls.maximize && is_resizable,
        WindowButton::Close => true,
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn has_enabled_window_button(
    buttons: [Option<WindowButton>; MAX_BUTTONS_PER_SIDE],
    state: WindowControlState,
) -> bool {
    buttons.into_iter().flatten().any(|button| {
        window_button_enabled(
            button,
            state.supported_controls,
            state.is_resizable,
            state.is_minimizable,
        )
    })
}

#[cfg(target_os = "windows")]
pub(crate) fn render_window_controls(
    buttons: [Option<WindowButton>; MAX_BUTTONS_PER_SIDE],
    state: WindowControlState,
    right_aligned: bool,
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
        .flex_none()
        .font_family("Segoe Fluent Icons")
        .when(
            window_button_enabled(
                WindowButton::Minimize,
                state.supported_controls,
                state.is_resizable,
                state.is_minimizable,
            ),
            |controls| {
                controls.child(caption_button(
                    "minimize",
                    "\u{e921}",
                    WindowControlArea::Min,
                    false,
                ))
            },
        )
        .when(
            window_button_enabled(
                WindowButton::Maximize,
                state.supported_controls,
                state.is_resizable,
                state.is_minimizable,
            ),
            |controls| {
                controls.child(caption_button(
                    if state.is_maximized {
                        "restore"
                    } else {
                        "maximize"
                    },
                    if state.is_maximized {
                        "\u{e923}"
                    } else {
                        "\u{e922}"
                    },
                    WindowControlArea::Max,
                    false,
                ))
            },
        )
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
    state: WindowControlState,
    right_aligned: bool,
    cx: &App,
) -> AnyElement {
    if !state.client_decorations {
        return div().into_any_element();
    }

    let colors = cx.theme().colors();
    if !has_enabled_window_button(buttons, state) {
        return div().into_any_element();
    }

    let controls = buttons
        .into_iter()
        .flatten()
        .filter_map(|button| {
            let (icon, area) = match button {
                WindowButton::Minimize => (IconName::GenericMinimize, WindowControlArea::Min),
                WindowButton::Maximize => (
                    if state.is_maximized {
                        IconName::GenericRestore
                    } else {
                        IconName::GenericMaximize
                    },
                    WindowControlArea::Max,
                ),
                WindowButton::Close => (IconName::GenericClose, WindowControlArea::Close),
            };
            window_button_enabled(
                button,
                state.supported_controls,
                state.is_resizable,
                state.is_minimizable,
            )
            .then(|| {
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
        })
        .collect::<Vec<_>>();

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
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .children(controls)
        .into_any_element()
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "freebsd")))]
pub(crate) fn render_window_controls(
    _buttons: [Option<WindowButton>; MAX_BUTTONS_PER_SIDE],
    _state: WindowControlState,
    _right_aligned: bool,
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
    let is_resizable = window.is_resizable();
    let tiling = match decorations {
        Decorations::Server => Tiling::default(),
        Decorations::Client { tiling } => tiling,
    };
    match decorations {
        Decorations::Client { .. } => window.set_client_inset(CLIENT_FRAME_INSET),
        Decorations::Server => window.set_client_inset(px(0.)),
    }

    div()
        .id("window-frame")
        .size_full()
        .bg(transparent_black())
        .map(|frame| match decorations {
            Decorations::Server => frame,
            Decorations::Client { .. } => frame
                .when(custom_window_border_enabled(), |frame| {
                    frame.rounded_client_corners(tiling)
                })
                .when(!tiling.top, |frame| frame.pt(CLIENT_FRAME_INSET))
                .when(!tiling.bottom, |frame| frame.pb(CLIENT_FRAME_INSET))
                .when(!tiling.left, |frame| frame.pl(CLIENT_FRAME_INSET))
                .when(!tiling.right, |frame| frame.pr(CLIENT_FRAME_INSET))
                .when(is_resizable, |frame| {
                    frame.on_mouse_down(MouseButton::Left, move |event, window, cx| {
                        let size = window.window_bounds().get_bounds().size;
                        if let Some(edge) = resize_edge(event.position, size, tiling) {
                            window.start_window_resize(edge);
                            cx.stop_propagation();
                        }
                    })
                }),
        })
        .child(
            div()
                .cursor(CursorStyle::Arrow)
                .map(|content| match decorations {
                    Decorations::Server => content,
                    Decorations::Client { .. } => {
                        content.when(custom_window_border_enabled(), |content| {
                            content
                                .border_color(cx.theme().colors().border)
                                .rounded_client_corners(tiling)
                                .when(!tiling.top, |content| content.border_t(BORDER_SIZE))
                                .when(!tiling.bottom, |content| content.border_b(BORDER_SIZE))
                                .when(!tiling.left, |content| content.border_l(BORDER_SIZE))
                                .when(!tiling.right, |content| content.border_r(BORDER_SIZE))
                                .when(!tiling.is_tiled(), |content| {
                                    content.shadow(vec![
                                        gpui::BoxShadow::new(
                                            px(0.),
                                            px(0.),
                                            gpui::Hsla {
                                                h: 0.,
                                                s: 0.,
                                                l: 0.,
                                                a: 0.4,
                                            },
                                        )
                                        .blur_radius(CLIENT_FRAME_INSET / 2.),
                                    ])
                                })
                        })
                    }
                })
                .on_mouse_move(|_, _, cx| cx.stop_propagation())
                .size_full()
                .child(content),
        )
        .when(
            matches!(decorations, Decorations::Client { .. }) && is_resizable,
            |frame| {
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
                                ResizeEdge::Left | ResizeEdge::Right => {
                                    CursorStyle::ResizeLeftRight
                                }
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
            },
        )
}

pub(crate) fn resize_edge(
    position: Point<Pixels>,
    window_size: Size<Pixels>,
    tiling: Tiling,
) -> Option<ResizeEdge> {
    let resize_bounds = Bounds::new(Point::default(), window_size).inset(CLIENT_FRAME_INSET * 1.5);
    if resize_bounds.contains(&position) {
        return None;
    }

    let corner_size = size(CLIENT_FRAME_INSET * 1.5, CLIENT_FRAME_INSET * 1.5);
    let top_left = Bounds::new(Point::default(), corner_size);
    if !tiling.top && top_left.contains(&position) {
        Some(ResizeEdge::TopLeft)
    } else if !tiling.top
        && Bounds::new(
            point(window_size.width - corner_size.width, px(0.)),
            corner_size,
        )
        .contains(&position)
    {
        Some(ResizeEdge::TopRight)
    } else if !tiling.bottom
        && Bounds::new(
            point(px(0.), window_size.height - corner_size.height),
            corner_size,
        )
        .contains(&position)
    {
        Some(ResizeEdge::BottomLeft)
    } else if !tiling.bottom
        && Bounds::new(
            point(
                window_size.width - corner_size.width,
                window_size.height - corner_size.height,
            ),
            corner_size,
        )
        .contains(&position)
    {
        Some(ResizeEdge::BottomRight)
    } else if !tiling.top && position.y < CLIENT_FRAME_INSET {
        Some(ResizeEdge::Top)
    } else if !tiling.bottom && position.y > window_size.height - CLIENT_FRAME_INSET {
        Some(ResizeEdge::Bottom)
    } else if !tiling.left && position.x < CLIENT_FRAME_INSET {
        Some(ResizeEdge::Left)
    } else if !tiling.right && position.x > window_size.width - CLIENT_FRAME_INSET {
        Some(ResizeEdge::Right)
    } else {
        None
    }
}

#[cfg(test)]
#[path = "tests/window_frame.rs"]
mod tests;
