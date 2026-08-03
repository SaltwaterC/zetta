use super::*;

#[test]
fn auxiliary_close_buttons_follow_window_close_button_side() {
    let left = WindowButtonLayout {
        left: [Some(WindowButton::Close), None, None],
        right: [
            Some(WindowButton::Minimize),
            Some(WindowButton::Maximize),
            None,
        ],
    };
    let right = WindowButtonLayout {
        left: [None; MAX_BUTTONS_PER_SIDE],
        right: [
            Some(WindowButton::Minimize),
            Some(WindowButton::Maximize),
            Some(WindowButton::Close),
        ],
    };

    assert!(window_close_button_on_left(left));
    assert!(!window_close_button_on_left(right));
}

#[test]
fn custom_window_border_matches_platform_conventions() {
    assert_eq!(custom_window_border_enabled(), !cfg!(target_os = "windows"));
}

#[test]
fn window_controls_respect_window_capabilities() {
    let supported = WindowControls::default();

    assert!(window_button_enabled(
        WindowButton::Minimize,
        supported,
        true,
        true,
    ));
    assert!(window_button_enabled(
        WindowButton::Maximize,
        supported,
        true,
        true,
    ));
    assert!(!window_button_enabled(
        WindowButton::Minimize,
        supported,
        true,
        false,
    ));
    assert!(!window_button_enabled(
        WindowButton::Maximize,
        supported,
        false,
        true,
    ));
    assert!(window_button_enabled(
        WindowButton::Close,
        supported,
        false,
        false,
    ));

    let unsupported = WindowControls {
        minimize: false,
        maximize: false,
        ..supported
    };
    assert!(!window_button_enabled(
        WindowButton::Minimize,
        unsupported,
        true,
        true,
    ));
    assert!(!window_button_enabled(
        WindowButton::Maximize,
        unsupported,
        true,
        true,
    ));
}

#[test]
#[cfg(target_os = "linux")]
fn parses_quoted_gsettings_button_layout() {
    let layout = parse_gsettings_button_layout("'close,minimize,maximize:'\n").unwrap();
    assert_eq!(
        layout.left,
        [
            Some(WindowButton::Close),
            Some(WindowButton::Minimize),
            Some(WindowButton::Maximize),
        ]
    );
    assert_eq!(layout.right, [None; MAX_BUTTONS_PER_SIDE]);
}

#[test]
fn resize_handles_cover_edges_and_respect_tiling() {
    let window = size(px(800.), px(600.));
    let untiled = Tiling::default();
    assert_eq!(
        resize_edge(point(px(1.), px(1.)), window, untiled),
        Some(ResizeEdge::TopLeft)
    );
    assert_eq!(
        resize_edge(point(px(799.), px(300.)), window, untiled),
        Some(ResizeEdge::Right)
    );
    assert_eq!(
        resize_edge(
            point(CLIENT_FRAME_INSET - px(1.), px(300.)),
            window,
            untiled
        ),
        Some(ResizeEdge::Left)
    );
    assert_eq!(
        resize_edge(
            point(CLIENT_FRAME_INSET + px(1.), px(300.)),
            window,
            untiled
        ),
        None
    );
    assert_eq!(
        resize_edge(point(px(400.), px(300.)), window, untiled),
        None
    );

    let tiled_left = Tiling {
        left: true,
        ..Tiling::default()
    };
    assert_eq!(
        resize_edge(point(px(1.), px(300.)), window, tiled_left),
        None
    );

    let maximized = Tiling::tiled();
    assert_eq!(resize_edge(point(px(1.), px(1.)), window, maximized), None);
    assert_eq!(
        resize_edge(point(px(400.), px(1.)), window, maximized),
        None
    );
    assert_eq!(
        resize_edge(point(px(799.), px(599.)), window, maximized),
        None
    );
}
