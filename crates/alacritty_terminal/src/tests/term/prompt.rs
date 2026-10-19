use super::*;
use crate::snapshot::ansi_snapshot;
use crate::vte::ansi::{Processor, StdSyncHandler};
use std::time::{Duration, Instant};

/// `VoidListener` is not `Clone`, which history reflow needs.
#[derive(Clone, Copy)]
struct Quiet;

impl EventListener for Quiet {}

const PROMPT_START: &[u8] = b"\x1b]133;A\x1b\\";
const COMMAND_START: &[u8] = b"\x1b]133;C\x07";

fn term(columns: usize, lines: usize) -> Term<Quiet> {
    Term::new(Config::default(), &test::TermSize::new(columns, lines), Quiet)
}

fn feed(term: &mut Term<Quiet>, bytes: &[u8]) {
    Processor::<StdSyncHandler>::new().advance(term, bytes);
}

fn screen(term: &Term<Quiet>) -> Vec<String> {
    (0..term.screen_lines() as i32)
        .map(|line| {
            let row = &term.grid()[Line(line)];
            let text: String = (0..term.columns()).map(|column| row[Column(column)].c).collect();
            text.trim_end().to_owned()
        })
        .collect()
}

/// A two-line prompt whose first line ends in a right-aligned segment, the
/// way zsh draws `RPROMPT`, so it fills the whole width.
fn prompt_first_line(columns: usize) -> String {
    let left = "~/source/zetta testing";
    let right = "4s";
    format!("{left}{}{right}", " ".repeat(columns - left.len() - right.len()))
}

/// What zsh does on `SIGWINCH`: up the rows it drew above the cursor, clear
/// to the end of the screen, and draw the prompt again.
fn zsh_redraw(rows_above_cursor: usize, prompt: &str) -> Vec<u8> {
    let mut bytes = b"\r".to_vec();
    if rows_above_cursor > 0 {
        bytes.extend_from_slice(format!("\x1b[{rows_above_cursor}A").as_bytes());
    }
    bytes.extend_from_slice(b"\x1b[J");
    bytes.extend_from_slice(prompt.as_bytes());
    bytes
}

fn narrowed_rprompt_screen(marked: bool) -> Vec<String> {
    let mut term = term(80, 8);
    feed(&mut term, format!("{}\r\n", "o".repeat(70)).as_bytes());
    if marked {
        feed(&mut term, PROMPT_START);
    }
    feed(&mut term, format!("{}\r\n> ", prompt_first_line(80)).as_bytes());
    term.resize(test::TermSize::new(60, 8));
    feed(&mut term, &zsh_redraw(1, &format!("{}\r\n> ", prompt_first_line(60))));
    screen(&term)
}

#[test]
fn narrowing_keeps_a_full_width_prompt_line_on_one_row_for_the_shell_to_redraw() {
    let screen = narrowed_rprompt_screen(true);

    // The output above the prompt still reflows.
    assert_eq!(
        screen[..4],
        ["o".repeat(60), "o".repeat(10), prompt_first_line(60), ">".to_owned()]
    );
    assert!(screen[4..].iter().all(String::is_empty), "{screen:#?}");
    // Without the mark the wrapped first line is left behind above the redraw.
    let unmarked = narrowed_rprompt_screen(false);
    assert_eq!(unmarked[2], "~/source/zetta testing");
    assert_eq!(unmarked[3], prompt_first_line(60));
}

fn widened_wrapped_input_screen(marked: bool) -> Vec<String> {
    let mut term = term(40, 6);
    feed(&mut term, b"keep me\r\n");
    if marked {
        feed(&mut term, PROMPT_START);
    }
    let input = format!("> {}", "x".repeat(50));
    feed(&mut term, input.as_bytes());
    term.resize(test::TermSize::new(80, 6));
    // The input wrapped onto a second row at 40 columns, so zsh moves up one.
    feed(&mut term, &zsh_redraw(1, &input));
    screen(&term)
}

#[test]
fn widening_keeps_wrapped_input_on_its_rows_so_the_redraw_cannot_overshoot() {
    let screen = widened_wrapped_input_screen(true);

    assert_eq!(screen[0], "keep me");
    assert_eq!(screen[1], format!("> {}", "x".repeat(50)));
    // Without the mark the join moved the input up a row and the redraw
    // cleared the output above it.
    assert_eq!(widened_wrapped_input_screen(false)[0], format!("> {}", "x".repeat(50)));
}

#[test]
fn the_prompt_follows_scrolling_and_clears_and_ends_with_the_command() {
    let mut term = term(20, 4);
    feed(&mut term, b"one\r\ntwo\r\nthree\r\n");
    feed(&mut term, PROMPT_START);
    assert_eq!(term.prompt_line(), Some(Line(3)));

    // The prompt's own second line scrolls the screen.
    feed(&mut term, b"first\r\n> ");
    assert_eq!(term.prompt_line(), Some(Line(2)));

    // `clear-screen` draws it again from the top.
    feed(&mut term, b"\x1b[H\x1b[2J");
    assert_eq!(term.prompt_line(), Some(Line(0)));

    feed(&mut term, COMMAND_START);
    assert_eq!(term.prompt_line(), None);
}

#[test]
fn a_mark_after_unterminated_output_starts_the_prompt_on_the_next_line() {
    let mut term = term(20, 4);
    feed(&mut term, b"no newline");
    feed(&mut term, PROMPT_START);

    assert_eq!(term.prompt_line(), Some(Line(1)));
}

#[test]
fn a_scroll_region_or_the_alternate_screen_drops_the_prompt() {
    let mut term = term(20, 6);
    feed(&mut term, PROMPT_START);
    feed(&mut term, b"\x1b[1;4r\x1b[4;1H\n");
    assert_eq!(term.prompt_line(), None);

    feed(&mut term, b"\x1b[r");
    feed(&mut term, PROMPT_START);
    feed(&mut term, b"\x1b[?1049h");
    assert_eq!(term.prompt_line(), None);
}

#[test]
fn history_reflow_off_the_ui_thread_keeps_the_prompt_rows_the_same_way() {
    let mut live = term(80, 8);
    for line in 0..2_000 {
        feed(&mut live, format!("{line:05}: {}\r\n", "history ".repeat(9)).as_bytes());
    }
    feed(&mut live, PROMPT_START);
    feed(&mut live, format!("{}\r\n> ", prompt_first_line(80)).as_bytes());
    let mut expected = live.clone();

    live.resize_with_async_history(test::TermSize::new(60, 8)).unwrap();
    expected.resize(test::TermSize::new(60, 8));
    let redraw = zsh_redraw(1, &format!("{}\r\n> ", prompt_first_line(60)));
    feed(&mut live, &redraw);
    feed(&mut expected, &redraw);
    let deadline = Instant::now() + Duration::from_secs(30);
    while !live.finish_pending_reflow() {
        assert!(Instant::now() < deadline, "history reflow did not finish");
        thread::yield_now();
    }

    assert_eq!(ansi_snapshot(&live, 1_000_000), ansi_snapshot(&expected, 1_000_000));
    assert_eq!(live.prompt_line(), expected.prompt_line());
    assert_eq!(screen(&live)[6..], [prompt_first_line(60), ">".to_owned()]);
}
