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
        resize_edge(point(px(9.), px(300.)), window, untiled),
        Some(ResizeEdge::Left)
    );
    assert_eq!(resize_edge(point(px(11.), px(300.)), window, untiled), None);
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
}
