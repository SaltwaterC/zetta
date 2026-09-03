//! The deterministic producer workloads `zetta benchmark` drives the renderer
//! with.
//!
//! Each workload writes to stdout from the child process a profiling profile
//! spawns, so the same bytes reach the terminal on Linux, macOS, and Windows.
//! Keep them free of platform-conditional output: a report recorded from one
//! platform is only comparable with another if the producer is identical.

use super::*;

fn checkerboard_background(row: usize, column: usize, frame: u64) -> u8 {
    if (row + column + frame as usize).is_multiple_of(2) {
        41
    } else {
        44
    }
}

/// The number of content rows every workload writes, matching the `rows` field
/// the performance report records.
const WORKLOAD_ROWS: usize = 34;
const WORKLOAD_ROW: &str = "0123456789 abcdefghijklmnopqrstuvwxyz ABCDEFGHIJKLMNOPQRSTUVWXYZ │─╭╮╰╯ ✓ rendered cell workload";

struct TerminalStateRestore {
    alternate_screen: bool,
}

impl Drop for TerminalStateRestore {
    fn drop(&mut self) {
        let mut stdout = std::io::stdout();
        let _ = stdout.write_all(b"\x1b[0m\x1b[?25h");
        if self.alternate_screen {
            let _ = stdout.write_all(b"\x1b[?1049l");
        }
        let _ = stdout.write_all(b"\r\n");
        let _ = stdout.flush();
    }
}

/// One line of the synthetic diff the alternate-screen workload scrolls
/// through. The 32-line cycle puts every kind of `git diff` styling — hunk
/// headers, added and removed lines, plain context, and the background-coloured
/// intraline highlight — inside every screenful, so a screen is representative
/// no matter where the scroll offset lands.
///
/// Lines are written without a trailing reset; the caller resets and clears to
/// the end of the row, which is also what leaves rows of genuinely different
/// lengths in the grid.
fn write_alt_screen_diff_line(
    output: &mut impl std::io::Write,
    index: usize,
) -> std::io::Result<()> {
    let hunk = index / 32;
    let step = index % 32;
    // Arbitrary odd multipliers, only so the blob hashes look like blob hashes.
    let before = hunk.wrapping_mul(2_654_435_761) & 0xfff_ffff;
    let after = hunk.wrapping_mul(2_246_822_519) & 0xfff_ffff;
    match step {
        0 => write!(
            output,
            "\x1b[1mdiff --git a/crates/terminal_view/src/layer_{hunk:03}.rs \
             b/crates/terminal_view/src/layer_{hunk:03}.rs"
        ),
        1 => write!(output, "index {before:07x}..{after:07x} 100644"),
        2 => write!(
            output,
            "\x1b[1m--- a/crates/terminal_view/src/layer_{hunk:03}.rs"
        ),
        3 => write!(
            output,
            "\x1b[1m+++ b/crates/terminal_view/src/layer_{hunk:03}.rs"
        ),
        4 => write!(
            output,
            "\x1b[36m@@ -{},{} +{},{} @@\x1b[0m \x1b[1mfn shape_visible_rows(&mut self)",
            hunk * 17 + 1,
            WORKLOAD_ROWS,
            hunk * 17 + 3,
            WORKLOAD_ROWS + 2
        ),
        // An intraline highlight: the only rows that set a background, matching
        // how sparingly a real diff uses one.
        11 | 23 => write!(
            output,
            "\x1b[32m+    let \x1b[42;30mshaped\x1b[0m\x1b[32m = \
             self.shape_line({step}, run.len(), cell_width, minimum_contrast);"
        ),
        5 | 7 | 12 | 15 | 19 | 24 | 27 | 30 => write!(
            output,
            "\x1b[32m+            batched_runs.push(BatchedTextRun::new({step:02}, \
             cell.style(), font_size));"
        ),
        6 | 13 | 20 | 25 | 31 => write!(
            output,
            "\x1b[31m-            batched_runs.push(run.clone_with_text({step:02}, \
             cell.style()));"
        ),
        _ => write!(
            output,
            "             let cell_{step:02} = grid.index({}, {step}).with_contrast(minimum);",
            hunk % 97
        ),
    }
}

fn write_alt_screen_scroll_frame(
    output: &mut impl std::io::Write,
    frame: u64,
) -> std::io::Result<()> {
    // Home the cursor and repaint every row from a one-line-per-step offset:
    // every visible row's content changes on every frame, which is what makes
    // this the pager-scrolling case rather than a partial update.
    output.write_all(b"\x1b[H")?;
    let offset = frame as usize;
    for row in 0..WORKLOAD_ROWS {
        write_alt_screen_diff_line(output, offset + row)?;
        // Pagers clear to end of line rather than padding, so the grid keeps
        // rows of genuinely different lengths.
        output.write_all(b"\x1b[0m\x1b[K\r\n")?;
    }
    write!(
        output,
        "\x1b[7malt-screen scroll · line {offset:07} · 240 Hz producer\x1b[0m\x1b[K"
    )?;
    output.flush()
}

fn write_sparse_update_frame(output: &mut impl std::io::Write, frame: u64) -> std::io::Result<()> {
    let spinner = ['|', '/', '-', '\\'][(frame as usize) % 4];
    write!(
        output,
        "\x1b[2;1H40 Hz sparse producer · processing {spinner} · frame {frame:010}"
    )?;
    output.flush()
}

fn write_sparse_update_backdrop(output: &mut impl std::io::Write) -> std::io::Result<()> {
    output.write_all(
        b"\x1b[H\x1b[1;36mZetta sparse terminal update profiler\x1b[0m\r\n\
          40 Hz producer updating only this status line\r\n\
          Dense unchanged content below models a full-screen TUI.\r\n\r\n",
    )?;
    for row in 0..WORKLOAD_ROWS {
        writeln!(output, "{row:02} {WORKLOAD_ROW}\r")?;
    }
    output.flush()
}

fn write_scrolling_frame(
    output: &mut impl std::io::Write,
    workload: PerformanceWorkload,
    frame: u64,
) -> std::io::Result<()> {
    let workload_description = match workload {
        PerformanceWorkload::CheckerboardBackground => "alternating cell backgrounds",
        _ => "text and line-drawing cells",
    };
    write!(
        output,
        "\x1b[H\x1b[1;36mZetta terminal rendering profiler\x1b[0m\r\n\
         240 Hz producer · {workload_description} · frame {frame:010}\r\n\
         This deterministic workload is identical on Linux, macOS, and Windows.\r\n\r\n"
    )?;
    for row in 0..WORKLOAD_ROWS {
        if workload == PerformanceWorkload::CheckerboardBackground {
            write!(output, "{row:02} ")?;
            for column in 0..96 {
                let background = checkerboard_background(row, column, frame);
                write!(output, "\x1b[{background}m ")?;
            }
            write!(output, "\x1b[0m\r\n")?;
        } else {
            writeln!(output, "{row:02} {WORKLOAD_ROW} {frame:010}\r")?;
        }
    }
    output.flush()
}

pub(super) fn run_terminal_rendering_workload(
    workload: PerformanceWorkload,
    duration: Option<Duration>,
) -> Result<()> {
    const FRAME_INTERVAL: Duration = Duration::from_nanos(4_166_667);
    const SPARSE_UPDATE_INTERVAL: Duration = Duration::from_millis(25);

    let alternate_screen = workload == PerformanceWorkload::AltScreenScroll;
    let _restore_terminal_state = TerminalStateRestore { alternate_screen };
    let stdout = std::io::stdout();
    let mut output = std::io::BufWriter::new(stdout.lock());
    if alternate_screen {
        output.write_all(b"\x1b[?1049h")?;
    }
    output.write_all(b"\x1b[2J\x1b[?25l")?;
    if workload == PerformanceWorkload::SparseUpdates {
        write_sparse_update_backdrop(&mut output)?;
    }

    let frame_interval = match workload {
        PerformanceWorkload::SparseUpdates => SPARSE_UPDATE_INTERVAL,
        _ => FRAME_INTERVAL,
    };
    let mut frame = 0_u64;
    let mut next_frame = Instant::now();
    let deadline = duration.map(|duration| next_frame + duration);
    loop {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            break;
        }
        let written = match workload {
            PerformanceWorkload::SparseUpdates => write_sparse_update_frame(&mut output, frame),
            PerformanceWorkload::AltScreenScroll => {
                write_alt_screen_scroll_frame(&mut output, frame)
            }
            _ => write_scrolling_frame(&mut output, workload, frame),
        };
        // A failed write means the pane consuming this workload has gone away,
        // which ends the run rather than failing it.
        if written.is_err() {
            return Ok(());
        }
        frame = frame.wrapping_add(1);

        next_frame += frame_interval;
        let now = Instant::now();
        let wake_at = deadline.map_or(next_frame, |deadline| next_frame.min(deadline));
        if wake_at > now {
            std::thread::sleep(wake_at - now);
        } else {
            next_frame = now;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/startup/workload.rs"]
mod tests;
