//! Real PSReadLine editing across the width change caused by splitting a pane.
#![cfg(all(windows, feature = "harness"))]

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::term::{Config, Term, test::TermSize};
use console_server::{ConsoleOutput, Launch, OutputMode, Screen, Session, grid};
use std::io;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use vte::ansi::{Processor, StdSyncHandler};

struct State {
    term: Term<VoidListener>,
    parser: Processor<StdSyncHandler>,
}

#[derive(Clone)]
struct Output(Arc<Mutex<State>>);

impl Output {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn text(&self) -> String {
        let state = self.lock();
        let screen = grid::screen(&state.term);
        grid::read_cells(&state.term, 0..screen.rows, 0..screen.columns)
            .into_iter()
            .map(|cell| cell.ch)
            .collect()
    }

    fn wait_for(&self, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if self.text().contains(text) && self.screen().cursor_visible {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("missing {text:?}: {:?}", self.text());
    }
}

impl ConsoleOutput for Output {
    fn write(&mut self, bytes: &[u8], mode: OutputMode) -> io::Result<()> {
        let mut state = self.lock();
        let State { term, parser } = &mut *state;
        term.set_output_mode(alacritty_terminal::hosted_console::OutputMode {
            lf_implies_cr: mode.lf_implies_cr,
            wrap: mode.wrap,
        });
        parser.advance(term, bytes);
        Ok(())
    }

    fn screen(&self) -> Screen {
        grid::screen(&self.lock().term)
    }
}

#[test]
fn powershell_keeps_the_pending_prompt_when_a_pane_is_split() {
    assert_pending_prompt_survives_split("powershell.exe");
}

#[test]
fn powershell_7_keeps_the_pending_prompt_when_a_pane_is_split() {
    if std::process::Command::new("pwsh.exe")
        .args(["-NoProfile", "-Command", "exit"])
        .status()
        .is_err()
    {
        eprintln!("skipped: PowerShell 7 is not installed");
        return;
    }
    assert_pending_prompt_survives_split("pwsh.exe");
}

fn assert_pending_prompt_survives_split(program: &str) {
    const PROMPT: &str = "PS C:\\Users\\saltw\\source\\repos\\zetta> ";
    const COMMAND: &str = "zetta benchmark output --output-type unique --size 1000";
    let output = Output(Arc::new(Mutex::new(State {
        term: Term::new(Config::default(), &TermSize::new(180, 24), VoidListener),
        parser: Processor::new(),
    })));
    let mut session = Session::spawn(
        Launch {
            command: format!("{program} -NoLogo -NoProfile -NoExit -Command \"Import-Module PSReadLine; Set-PSReadLineOption -HistorySaveStyle SaveNothing; function prompt {{ '{PROMPT}' }}\""),
            ..Default::default()
        },
        output.clone(),
    )
    .unwrap();
    output.wait_for("PS C:");
    session.input(COMMAND.as_bytes()).unwrap();
    output.wait_for("--size 1000");
    output.lock().term.resize(TermSize::new(90, 24));
    session.resize(90, 24);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        output.text().contains(&format!("{PROMPT}{COMMAND}")),
        "before key: {:?}",
        output.text()
    );
    let cursor_offset = PROMPT.len() + COMMAND.len();
    assert_eq!(
        output.screen().cursor,
        ((cursor_offset % 90) as i16, (cursor_offset / 90) as i16)
    );
    // A key forces PSReadLine to redraw after observing the changed width.
    session.input(b"x").unwrap();
    output.wait_for("000x");
    let text = output.text();
    session.close();
    assert!(text.contains(&format!("{PROMPT}{COMMAND}x")), "{text:?}");
}
