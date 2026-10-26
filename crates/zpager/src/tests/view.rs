use super::*;

#[derive(Default)]
struct RecordingOutput {
    writes: Vec<Vec<u8>>,
    flushes: usize,
    fail_flush: bool,
}

impl Write for RecordingOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.writes.push(bytes.to_vec());
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flushes += 1;
        if self.fail_flush {
            Err(io::Error::other("simulated output failure"))
        } else {
            Ok(())
        }
    }
}

fn state(color: bool) -> ViewState {
    let mut input = String::from("diff --git a/file b/file\n@@ -1,40 +1,40 @@\n");
    for index in 0..40 {
        input.push_str(&format!(" line {index}\n"));
    }
    ViewState {
        document: DiffDocument::parse(input.as_bytes()),
        scroll: 0,
        cursor: 0,
        color,
        columns: 20,
        width: None,
        layout_width: None,
        layout_columns: None,
        geometry: None,
        rows: Vec::new(),
        files: Vec::new(),
        drawn_view: None,
    }
}

fn assert_synchronized_frame(frame: &[u8]) {
    let text = std::str::from_utf8(frame).unwrap();
    assert!(text.starts_with("\x1b[?2026h"));
    assert!(text.ends_with("\x1b[?2026l"));
    assert_eq!(text.matches("\x1b[?2026h").count(), 1);
    assert_eq!(text.matches("\x1b[?2026l").count(), 1);
    assert!(text.contains("\x1b[2J"));
    assert!(text.contains("zpager"));
    assert!(text.contains("line 0"));
    assert!(
        text.contains("\x1b[24;1H"),
        "frame must include the status row"
    );
}

#[test]
fn complete_repaints_are_buffered_and_synchronized_in_both_layouts() {
    for color in [false, true] {
        for width in [40, 120] {
            let mut state = state(color);
            let mut output = RecordingOutput::default();
            state.draw(&mut output, width, 24).unwrap();
            assert_eq!(output.writes.len(), 1, "frame must be assembled first");
            assert_eq!(output.flushes, 1);
            assert_synchronized_frame(&output.writes[0]);
        }
    }
}

#[test]
fn idle_polls_and_navigation_at_the_boundary_emit_no_output() {
    let mut state = state(true);
    let mut output = RecordingOutput::default();
    state.draw(&mut output, 120, 24).unwrap();
    for _ in 0..8 {
        // This is also the result of Up/Home while already on the first row.
        state.cursor = state.cursor.saturating_sub(1);
        state.draw(&mut output, 120, 24).unwrap();
    }
    assert_eq!(output.writes.len(), 1);
    assert_eq!(output.flushes, 1);
}

#[test]
fn cursor_scroll_and_each_resize_dimension_trigger_a_repaint() {
    let mut state = state(true);
    let mut output = RecordingOutput::default();
    state.draw(&mut output, 120, 24).unwrap();
    state.cursor = 1;
    state.draw(&mut output, 120, 24).unwrap();
    assert_eq!(output.writes.len(), 2);
    assert_ne!(output.writes[0], output.writes[1]);

    state.cursor = 30;
    state.draw(&mut output, 120, 24).unwrap();
    assert!(state.scroll > 0);
    assert_eq!(output.writes.len(), 3);
    // Scrolling up while keeping the same cursor visible changes only scroll.
    state.scroll -= 1;
    state.draw(&mut output, 120, 25).unwrap();
    let scroll = state.scroll;
    state.scroll += 1;
    state.draw(&mut output, 120, 25).unwrap();
    assert_eq!(state.scroll, scroll + 1);
    assert_eq!(output.writes.len(), 5);

    state.draw(&mut output, 40, 25).unwrap();
    assert_eq!(output.writes.len(), 6);
    assert_eq!(state.geometry.unwrap().mode, LayoutMode::Stacked);
    state.draw(&mut output, 40, 10).unwrap();
    assert_eq!(output.writes.len(), 7);
    state.draw(&mut output, 40, 10).unwrap();
    assert_eq!(output.writes.len(), 7);
}

#[test]
fn failed_output_does_not_mark_the_view_as_drawn() {
    let mut state = state(true);
    let mut output = RecordingOutput {
        fail_flush: true,
        ..RecordingOutput::default()
    };
    assert!(state.draw(&mut output, 120, 24).is_err());
    assert!(state.drawn_view.is_none());
    output.fail_flush = false;
    state.draw(&mut output, 120, 24).unwrap();
    assert_eq!(output.writes.len(), 2);
    assert_eq!(output.writes[0], output.writes[1]);
    state.draw(&mut output, 120, 24).unwrap();
    assert_eq!(output.writes.len(), 2);
}
