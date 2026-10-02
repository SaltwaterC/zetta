use super::*;
use crate::TerminalBuilder;
use crate::terminal_settings::{AlternateScroll, CursorShape};
use gpui::{AppContext as _, Entity, TestAppContext};
use std::io::{Read, Write};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};
use util::paths::PathStyle;

const BUDGET: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(10);

fn init(cx: &mut TestAppContext) {
    cx.update(|cx| {
        crate::terminal_settings::TerminalSettings::init(cx);
        theme_settings::init(theme::LoadThemes::JustBase, cx);
    });
    cx.executor().allow_parking();
}

/// A reader that yields its bytes only once released, then ends — standing in
/// for a relay whose last bytes are still in flight when the handover starts.
#[derive(Clone, Default)]
struct Gate(Arc<(Mutex<bool>, Condvar)>);

impl Gate {
    fn release(&self) {
        let (released, changed) = &*self.0;
        *released.lock().unwrap() = true;
        changed.notify_all();
    }

    fn wait(&self) {
        let (released, changed) = &*self.0;
        let mut released = released.lock().unwrap();
        while !*released {
            released = changed.wait(released).unwrap();
        }
    }
}

struct GatedReader {
    gate: Gate,
    bytes: Vec<u8>,
}

impl Read for GatedReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.gate.wait();
        let count = self.bytes.len().min(buffer.len());
        buffer[..count].copy_from_slice(&self.bytes[..count]);
        self.bytes.drain(..count);
        Ok(count)
    }
}

#[derive(Clone, Default)]
struct Recorded(Arc<Mutex<Vec<u8>>>);

impl Recorded {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl Write for Recorded {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn byte_stream_terminal(
    reader: impl Read + Send + 'static,
    writer: impl Write + Send + 'static,
    cx: &mut TestAppContext,
) -> Entity<Terminal> {
    cx.new(|cx| {
        TerminalBuilder::new_byte_stream(
            Box::new(reader),
            Box::new(writer),
            String::new(),
            CursorShape::default(),
            AlternateScroll::On,
            None,
            0,
            &cx.background_executor().clone(),
            PathStyle::local(),
        )
        .subscribe(cx)
    })
}

async fn eventually(cx: &mut TestAppContext, mut done: impl FnMut(&mut TestAppContext) -> bool) {
    let deadline = Instant::now() + BUDGET;
    while !done(cx) {
        assert!(
            Instant::now() < deadline,
            "condition not reached in {BUDGET:?}"
        );
        cx.background_executor.timer(POLL).await;
        std::thread::sleep(POLL);
    }
}

fn content(terminal: &Entity<Terminal>, cx: &mut TestAppContext) -> String {
    terminal.update(cx, |terminal, _| terminal.get_content())
}

#[test]
fn what_a_worker_waits_on_can_be_sent_to_it() {
    fn assert_send<T: Send + 'static>() {}
    assert_send::<RetiredReader>();
    assert_send::<GridSnapshotSource>();
}

/// The barrier the whole module exists for: bytes the retired relay still had
/// in flight reach the grid before `finish` returns, and so before anything a
/// later reader parses.
#[gpui::test]
async fn a_retired_streams_last_bytes_land_before_finish_returns(cx: &mut TestAppContext) {
    init(cx);
    let gate = Gate::default();
    let terminal = byte_stream_terminal(
        GatedReader {
            gate: gate.clone(),
            bytes: b"older-bytes\r\n".to_vec(),
        },
        std::io::sink(),
        cx,
    );
    let retired = terminal.update(cx, |terminal, _| terminal.retire_byte_stream());
    assert!(!retired.is_empty());

    // Retiring returned while the reader was still blocked mid-read, which is
    // the whole point: the window's thread did not wait for it.
    let finished = std::thread::spawn(move || retired.finish());
    gate.release();
    finished.join().unwrap().unwrap();
    assert!(
        content(&terminal, cx).contains("older-bytes"),
        "the drained bytes must be in the grid once finish returns"
    );

    terminal.update(cx, |terminal, _| {
        terminal
            .attach_byte_stream(
                Box::new(std::io::Cursor::new(b"newer-bytes\r\n".to_vec())),
                Box::new(std::io::sink()),
            )
            .unwrap();
    });
    eventually(cx, |cx| content(&terminal, cx).contains("newer-bytes")).await;
    let content = content(&terminal, cx);
    assert!(
        content.find("older-bytes") < content.find("newer-bytes"),
        "the retired reader's bytes must come first: {content}"
    );
}

/// Keystrokes typed while a pane is between readers used to be dropped, or —
/// before the wait moved off the window's thread — queued behind it. They go
/// to whichever backend is attached next, and only to it.
#[gpui::test]
async fn input_typed_between_readers_goes_to_the_next_backend(cx: &mut TestAppContext) {
    init(cx);
    let retired_writer = Recorded::default();
    let terminal =
        byte_stream_terminal(std::io::Cursor::new(Vec::new()), retired_writer.clone(), cx);
    let retired = terminal.update(cx, |terminal, _| terminal.retire_byte_stream());
    terminal.update(cx, |terminal, _| {
        terminal.input(b"typed-mid-handover".to_vec())
    });
    retired.finish().unwrap();

    let next_writer = Recorded::default();
    terminal.update(cx, |terminal, _| {
        terminal
            .attach_byte_stream(
                Box::new(std::io::Cursor::new(Vec::new())),
                Box::new(next_writer.clone()),
            )
            .unwrap();
        terminal.input(b"|typed-after".to_vec());
    });
    eventually(cx, |_| next_writer.text().contains("typed-after")).await;
    assert_eq!(next_writer.text(), "typed-mid-handover|typed-after");
    assert!(
        retired_writer.text().is_empty(),
        "nothing may reach the retired relay: {:?}",
        retired_writer.text()
    );
}

/// A handover that will not complete discards what it held, so a later
/// backend is not sent keystrokes aimed at a pane that went dead.
#[gpui::test]
async fn discarded_input_is_not_replayed(cx: &mut TestAppContext) {
    init(cx);
    let terminal = byte_stream_terminal(std::io::Cursor::new(Vec::new()), std::io::sink(), cx);
    let retired = terminal.update(cx, |terminal, _| terminal.retire_byte_stream());
    terminal.update(cx, |terminal, _| {
        terminal.input(b"stale".to_vec());
        terminal.discard_held_input();
        terminal.input(b"also-dropped".to_vec());
    });
    retired.finish().unwrap();

    let next_writer = Recorded::default();
    terminal.update(cx, |terminal, _| {
        terminal
            .attach_byte_stream(
                Box::new(std::io::Cursor::new(Vec::new())),
                Box::new(next_writer.clone()),
            )
            .unwrap();
        terminal.input(b"fresh".to_vec());
    });
    eventually(cx, |_| next_writer.text().contains("fresh")).await;
    assert_eq!(next_writer.text(), "fresh");
}

/// An unfinished retirement that is dropped must not leave its reader writing
/// into a grid a new reader may already own.
#[gpui::test]
async fn dropping_an_unfinished_retirement_abandons_its_reader(cx: &mut TestAppContext) {
    init(cx);
    let gate = Gate::default();
    let terminal = byte_stream_terminal(
        GatedReader {
            gate: gate.clone(),
            bytes: b"abandoned-bytes\r\n".to_vec(),
        },
        std::io::sink(),
        cx,
    );
    let retired = terminal.update(cx, |terminal, _| terminal.retire_byte_stream());
    drop(retired);
    gate.release();
    std::thread::sleep(Duration::from_millis(100));
    cx.run_until_parked();
    assert!(
        !content(&terminal, cx).contains("abandoned-bytes"),
        "an abandoned reader wrote into the grid"
    );
}

/// The worker's snapshot is the same picture the terminal itself renders.
#[gpui::test]
async fn a_snapshot_source_renders_what_the_terminal_does(cx: &mut TestAppContext) {
    init(cx);
    let terminal = byte_stream_terminal(
        std::io::Cursor::new(b"\x1b[1mbold\x1b[0m plain\r\n".to_vec()),
        std::io::sink(),
        cx,
    );
    eventually(cx, |cx| content(&terminal, cx).contains("plain")).await;
    let retired = terminal.update(cx, |terminal, _| terminal.retire_byte_stream());
    retired.finish().unwrap();
    let (source, direct) = terminal.update(cx, |terminal, _| {
        (terminal.grid_snapshot_source(), terminal.ansi_snapshot(100))
    });
    let from_worker = std::thread::spawn(move || source.ansi_snapshot(100))
        .join()
        .unwrap();
    assert_eq!(from_worker, direct);
}

/// Retiring a pty loop is the same barrier as stopping it: once finished,
/// nothing the loop read reaches the grid, and the child keeps running.
#[cfg(unix)]
#[gpui::test]
async fn a_finished_pty_loop_no_longer_feeds_the_grid(cx: &mut TestAppContext) {
    init(cx);
    let shell = (
        "/bin/sh".to_owned(),
        vec![
            "-c".to_owned(),
            "i=0; while :; do i=$((i+1)); echo line-$i; sleep 0.01; done".to_owned(),
        ],
    );
    let options = crate::alacritty::pty_options(
        Some(shell),
        None,
        std::iter::empty::<(String, String)>(),
        None,
    );
    let pty = alacritty_terminal::tty::new(
        &options,
        alacritty_terminal::event::WindowSize {
            num_lines: 24,
            num_cols: 80,
            cell_width: 1,
            cell_height: 1,
        },
        0,
    )
    .unwrap();
    let child_pid = pty.child_pid();
    let attached = TerminalBuilder::new_attached(
        crate::PtyHandover {
            descriptor: pty.file().try_clone().unwrap().into(),
            child_pid,
            replay: Vec::new(),
            control: Arc::new(NoopControl),
        },
        crate::AttachedOptions {
            shell: task::Shell::System,
            env: Default::default(),
            cursor_shape: CursorShape::default(),
            alternate_scroll: AlternateScroll::On,
            max_scroll_history_lines: None,
            path_hyperlink_regexes: Vec::new(),
            path_hyperlink_timeout_ms: 0,
            window_id: 0,
        },
        &cx.background_executor,
        PathStyle::local(),
    )
    .unwrap();
    let terminal = cx.new(|cx| attached.builder.subscribe(cx));
    eventually(cx, |cx| content(&terminal, cx).contains("line-3")).await;

    let retired = terminal.update(cx, |terminal, _| terminal.retire_pty_loop());
    assert!(!retired.is_empty());
    std::thread::spawn(move || retired.finish())
        .join()
        .unwrap()
        .unwrap();
    let settled = content(&terminal, cx);
    std::thread::sleep(Duration::from_millis(100));
    cx.run_until_parked();
    assert_eq!(
        content(&terminal, cx),
        settled,
        "a finished loop went on reading"
    );
    let alive = unsafe { libc::kill(child_pid as libc::pid_t, 0) } == 0;
    assert!(alive, "retiring the loop must not end the pty's child");
    unsafe { libc::kill(-(child_pid as libc::pid_t), libc::SIGKILL) };
    drop(pty);
}

#[cfg(unix)]
struct NoopControl;

#[cfg(unix)]
impl crate::PtyControl for NoopControl {
    fn resize(&self, _: u16, _: u16) {}
    fn set_console_palette(&self, _: crate::ConsolePalette) {}
}
