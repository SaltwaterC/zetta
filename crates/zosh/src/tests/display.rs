use super::*;

#[test]
fn a_clear_is_applied_once_when_its_protocol_state_is_displayed() {
    use mosh_rs::{ClientTerminal, screen::OverlayCursor};
    let mut terminal = ClientTerminal::new(DisplayScreen::new(5, 20));
    let marker = b"\x1b]777;zosh-clear-scrollback;1\x07";
    terminal.apply_diff(0, 1, b"old TUI");
    terminal.render(&[], OverlayCursor::Unchanged);
    let clear = [b"\x1b[H\x1b[2Jprompt".as_slice(), marker].concat();
    terminal.apply_diff(1, 3, &clear);
    let output = terminal.render(&[], OverlayCursor::Unchanged);
    assert!(output.windows(4).any(|bytes| bytes == CLEAR_SCROLLBACK));
    let mut physical = DisplayScreen::new(5, 20);
    physical.feed(&output);
    assert_eq!(physical.text().trim(), "prompt");
    assert!(!output.windows(3).any(|bytes| bytes == b"777"));

    // An old arrival and a new diff against the same unacknowledged base
    // must both preserve scrollback created after the first clear.
    terminal.apply_diff(1, 2, &clear);
    assert!(terminal.render(&[], OverlayCursor::Unchanged).is_empty());
    terminal.apply_diff(1, 4, &clear);
    assert!(terminal.render(&[], OverlayCursor::Unchanged).is_empty());
    terminal.apply_diff(1, 5, b"\x1b]777;zosh-clear-scrollback;2\x07");
    assert!(
        terminal
            .render(&[], OverlayCursor::Unchanged)
            .windows(4)
            .any(|b| b == CLEAR_SCROLLBACK)
    );
}

#[test]
fn an_ordinary_redraw_does_not_clear_scrollback() {
    let mut previous = DisplayScreen::new(5, 20);
    previous.feed(b"old");
    let mut next = previous.clone();
    next.feed(b"\x1b[H\x1b[2Jprompt\x0c");
    assert!(
        !next
            .diff_from(&previous)
            .windows(4)
            .any(|b| b == CLEAR_SCROLLBACK)
    );
}

#[test]
fn a_pending_clear_survives_a_resize_repaint_without_repeating() {
    use mosh_rs::ClientTerminal;
    let mut terminal = ClientTerminal::new(DisplayScreen::new(5, 20));
    terminal.apply_diff(0, 1, b"prompt\x1b]777;zosh-clear-scrollback;1\x07");
    terminal.resize(10, 40);
    let previous = terminal.displayed().clone();
    let repaint = terminal.repaint();
    assert!(terminal.displayed().clears_scrollback_since(&previous));
    let mut output = Vec::new();
    repaint_after_resize(&mut output, &repaint, b"", CLEAR_SCROLLBACK).unwrap();
    assert!(output.ends_with(CLEAR_SCROLLBACK));

    let previous = terminal.displayed().clone();
    terminal.resize(5, 20);
    terminal.repaint();
    assert!(!terminal.displayed().clears_scrollback_since(&previous));
}

#[test]
fn opening_clears_the_visible_page_without_entering_alternate_screen() {
    let mut output = Vec::new();
    open(&mut output).unwrap();
    assert_eq!(output, DISPLAY_OPEN);
    assert!(
        !output
            .windows(b"\x1b[?1049h".len())
            .any(|window| window == b"\x1b[?1049h")
    );
}

#[test]
fn resizing_clears_before_writing_the_full_repaint() {
    let mut output = Vec::new();
    repaint_after_resize(&mut output, b"screen", b"modes", b"scrollback").unwrap();
    assert_eq!(&output[..DISPLAY_RESIZE.len()], DISPLAY_RESIZE);
    assert_eq!(&output[DISPLAY_RESIZE.len()..], b"screenmodesscrollback");
}

#[test]
fn a_scrolled_screen_uses_physical_newlines() {
    let mut previous = DisplayScreen::new(5, 20);
    previous.feed(b"zero\r\none\r\ntwo\r\nthree\r\nfour");
    let mut current = DisplayScreen::new(5, 20);
    current.feed(b"two\r\nthree\r\nfour\r\nfive\r\nsix");

    assert_eq!(
        scroll_plan(&previous, &current),
        Some(ScrollPlan {
            lines_scrolled: 2,
            scroll_height: 3
        })
    );
    let output = current.diff_from(&previous);
    assert!(output.starts_with(b"\r\n\n"));

    let mut physical = previous.clone();
    physical.feed(&output);
    assert_eq!(physical.text(), current.text());
    assert_eq!(physical.cursor(), current.cursor());
}

#[test]
fn a_non_scrolling_redraw_does_not_scroll_the_terminal() {
    let mut previous = DisplayScreen::new(5, 20);
    previous.feed(b"one\r\ntwo");
    let mut current = previous.clone();
    current.feed(b"\rX");

    let output = current.diff_from(&previous);
    assert!(!output.starts_with(b"\r\n"));
    assert!(scroll_plan(&previous, &current).is_none());
}

#[test]
fn an_unchanged_top_row_is_not_mistaken_for_a_scroll() {
    let mut previous = DisplayScreen::new(5, 20);
    previous.feed(b"top\r\n");
    let mut current = previous.clone();
    current.feed(b"next");

    assert!(scroll_plan(&previous, &current).is_none());
    assert!(!current.diff_from(&previous).starts_with(b"\r\n"));
}

#[test]
fn the_first_frame_restores_the_protocol_cursor_state() {
    let previous = DisplayScreen::new(5, 20);
    let mut current = previous.clone();
    current.feed(b"hello");

    assert!(
        current
            .diff_from(&previous)
            .windows(6)
            .any(|window| { window == b"\x1b[?25h" })
    );
}

#[test]
fn a_resize_clears_and_repaints_the_new_geometry() {
    let mut previous = DisplayScreen::new(3, 10);
    previous.feed(b"old screen");
    let mut current = previous.clone();
    current.resize(5, 20);
    current.feed(b"new screen");

    let output = current.diff_from(&previous);
    assert!(output.starts_with(DISPLAY_RESIZE));

    // A real terminal has already applied the geometry change before this
    // output arrives. Replaying the output on a blank screen models that
    // terminal after the clear sequence.
    let mut physical = DisplayScreen::new(5, 20);
    physical.feed(&output);
    assert_eq!(physical.text(), current.text());
    assert_eq!(physical.cursor(), current.cursor());
}

#[test]
fn display_diff_carries_input_modes_from_the_protocol_screen() {
    let mut previous = DisplayScreen::new(5, 20);
    previous.feed(b"\x1b[?2004l");
    let mut current = previous.clone();
    current.feed(b"\x1b[?2004h");

    assert!(
        current
            .diff_from(&previous)
            .windows(8)
            .any(|window| { window == b"\x1b[?2004h" })
    );
}

#[test]
fn a_repaint_preserves_input_modes_already_set_on_the_terminal() {
    let mut screen = DisplayScreen::new(5, 20);
    screen.feed(b"\x1b[?1h\x1b[?2004h\x1b[?1000hhello");

    let repaint = screen.repaint();

    assert!(!repaint.windows(5).any(|window| window == b"\x1b[?1l"));
    assert!(!repaint.windows(8).any(|window| window == b"\x1b[?2004l"));
    assert!(!repaint.windows(8).any(|window| window == b"\x1b[?1000l"));
    assert!(repaint.windows(5).any(|window| window == b"hello"));
}

#[test]
fn repeated_output_keeps_the_physical_screen_in_sync() {
    let mut previous = DisplayScreen::new(24, 80);
    previous.feed(b"banner one\r\nbanner two\r\nbanner three\r\n> ");
    let mut physical = previous.clone();
    for number in 1..=30 {
        let mut current = previous.clone();
        current.feed(format!("LINE{number}\r\n").as_bytes());
        let output = current.diff_from(&previous);
        physical.feed(&output);
        assert_eq!(physical.text(), current.text(), "line {number}");
        previous = current;
    }
}
