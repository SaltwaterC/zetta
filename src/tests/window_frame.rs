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
#[cfg(any(target_os = "windows", linux_like))]
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

#[cfg(linux_like)]
#[test]
fn empty_client_window_button_sides_do_not_reserve_space() {
    let state = WindowControlState {
        supported_controls: WindowControls::default(),
        is_maximized: false,
        is_resizable: true,
        is_minimizable: true,
        client_decorations: true,
    };

    assert!(!has_enabled_window_button(
        [None; MAX_BUTTONS_PER_SIDE],
        state
    ));
    assert!(has_enabled_window_button(
        [Some(WindowButton::Close), None, None],
        state
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

#[test]
fn linux_corner_state_follows_client_decorations_and_tiling() {
    let untiled = linux_corner_state(true, Tiling::default());
    assert!(untiled.top_left);
    assert!(untiled.top_right);
    assert!(untiled.bottom_left);
    assert!(untiled.bottom_right);

    let maximized = linux_corner_state(true, Tiling::tiled());
    assert!(!maximized.top_left);
    assert!(!maximized.top_right);
    assert!(!maximized.bottom_left);
    assert!(!maximized.bottom_right);

    let left_tiled = linux_corner_state(
        true,
        Tiling {
            left: true,
            ..Tiling::default()
        },
    );
    assert!(!left_tiled.top_left);
    assert!(left_tiled.top_right);
    assert!(!left_tiled.bottom_left);
    assert!(left_tiled.bottom_right);

    let server_decorations = linux_corner_state(false, Tiling::default());
    assert!(!server_decorations.top_left);
    assert!(!server_decorations.top_right);
    assert!(!server_decorations.bottom_left);
    assert!(!server_decorations.bottom_right);
}

#[test]
fn macos_corner_state_is_square_only_in_fullscreen() {
    let windowed = macos_corner_state(false);
    assert!(windowed.top_left);
    assert!(windowed.top_right);
    assert!(windowed.bottom_left);
    assert!(windowed.bottom_right);

    let fullscreen = macos_corner_state(true);
    assert!(!fullscreen.top_left);
    assert!(!fullscreen.top_right);
    assert!(!fullscreen.bottom_left);
    assert!(!fullscreen.bottom_right);
}

#[test]
fn windows_corner_state_rejects_maximized_fullscreen_and_snapped_bounds() {
    let work_area = Bounds::new(point(px(0.), px(0.)), size(px(1920.), px(1040.)));
    let restored = Bounds::new(point(px(240.), px(120.)), size(px(900.), px(700.)));
    let half_snap = Bounds::new(point(px(0.), px(0.)), size(px(960.), px(1040.)));
    let quarter_snap = Bounds::new(point(px(0.), px(0.)), size(px(960.), px(520.)));

    let restored_state = windows_corner_state(false, false, restored, Some(work_area));
    assert!(restored_state.top_left);
    assert!(restored_state.bottom_right);
    assert!(!windows_corner_state(false, false, half_snap, Some(work_area)).top_left);
    assert!(!windows_corner_state(false, false, quarter_snap, Some(work_area)).bottom_right);
    assert!(!windows_corner_state(true, false, restored, Some(work_area)).top_left);
    assert!(!windows_corner_state(false, true, restored, Some(work_area)).top_left);
    assert!(windows_corner_state(false, false, restored, None).bottom_left);
}

#[test]
fn windows_snap_detection_requires_two_work_area_edges() {
    let work_area = Bounds::new(point(px(0.), px(0.)), size(px(1920.), px(1040.)));
    let one_edge = Bounds::new(point(px(0.), px(120.)), size(px(900.), px(700.)));
    let two_edges = Bounds::new(point(px(0.), px(0.)), size(px(900.), px(700.)));

    assert!(!bounds_touch_at_least_two_edges(one_edge, work_area));
    assert!(bounds_touch_at_least_two_edges(two_edges, work_area));
}
