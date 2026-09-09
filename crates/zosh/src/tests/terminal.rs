use super::*;

#[test]
fn close_sequence_resets_remote_terminal_modes() {
    let close = String::from_utf8_lossy(CLOSE_SEQUENCE);
    for mode in [
        "1000", "1001", "1002", "1003", "1004", "1005", "1006", "1015", "2004",
    ] {
        assert!(close.contains(&format!("\x1b[?{mode}l")));
    }
    assert!(close.contains("\x1b[0m"));
    assert!(close.contains("\x1b[?25h"));
    assert!(!close.contains("\x1b[?1049l"));
}

#[test]
fn no_init_close_preserves_the_normal_screen() {
    let mut output = Vec::new();
    write_close_sequence(&mut output, false).unwrap();
    assert!(
        !output
            .windows("\x1b[?1049l".len())
            .any(|window| { window == b"\x1b[?1049l" })
    );
}

#[test]
fn initialized_close_leaves_the_alternate_screen() {
    let mut output = Vec::new();
    write_close_sequence(&mut output, true).unwrap();
    assert!(output.ends_with(b"\x1b[?1049l"));
}

#[test]
fn terminal_guard_restore_is_idempotent() {
    let mut guard = TerminalGuard {
        initialized: false,
        alternate_screen: false,
        restored: false,
        #[cfg(unix)]
        saved_mode: None,
    };
    guard.restore();
    assert!(guard.restored);
    guard.restore();
    assert!(guard.restored);
}

#[test]
fn unusable_window_dimensions_use_moshs_fallback() {
    assert_eq!(usable_size((0, 0)), (80, 24));
    assert_eq!(usable_size((80, 0)), (80, 24));
    assert_eq!(usable_size((0, 24)), (80, 24));
    assert_eq!(usable_size((120, 40)), (120, 40));
}

#[test]
fn color_capability_prefers_truecolor_and_keeps_term_fallbacks() {
    assert_eq!(
        color_count_from_environment(Some("xterm-256color"), None),
        256
    );
    assert_eq!(color_count_from_environment(Some("xterm-color"), None), 8);
    assert_eq!(color_count_from_environment(Some("dumb"), None), 0);
    assert_eq!(
        color_count_from_environment(Some("xterm-256color"), Some("truecolor")),
        1 << 15
    );
    assert_eq!(
        color_count_from_environment(Some("xterm"), Some("24bit")),
        1 << 15
    );
    assert_eq!(
        color_count_from_environment(Some("xterm-256color"), Some("ansi")),
        256
    );
}
