//! Embeds console-server behind the terminal's existing PTY and parser pipeline.
//! Only this adapter translates console-server hooks into terminal-engine state.
use crate::alacritty::AlacrittyTermLock;
use alacritty_terminal::event::WindowSize;
use alacritty_terminal::hosted_console::queue::{self, OutputWriter};
use alacritty_terminal::hosted_console::{HostedConsole, HostedPty, OutputMode};
use alacritty_terminal::tty;
use console_server::output::Cell;
use console_server::{ConsoleOutput, Launch, Screen, Session, grid};
use std::io;
use std::sync::Arc;
use vte::ansi::Handler;

struct Output {
    writer: OutputWriter,
    term: Arc<AlacrittyTermLock>,
}
impl ConsoleOutput for Output {
    fn write(&mut self, bytes: &[u8], mode: console_server::OutputMode) -> io::Result<()> {
        self.writer.write(
            bytes,
            OutputMode {
                lf_implies_cr: mode.lf_implies_cr,
                wrap: mode.wrap,
            },
        )
    }
    fn flush(&self) -> io::Result<()> {
        self.writer.flush()
    }
    fn cursor(&self) -> (i16, i16) {
        self.screen().cursor
    }
    fn size(&self) -> (i16, i16) {
        let screen = self.screen();
        (screen.columns, screen.rows)
    }
    fn screen(&self) -> Screen {
        // Console screen coordinates describe the live viewport, never the
        // scrollback archive. Waiting for history here also blocks PSReadLine's
        // cursor queries and makes typing appear lost during a large reflow.
        grid::screen(&self.term.lock())
    }
    fn read_cells(
        &self,
        rows: std::ops::Range<i16>,
        columns: std::ops::Range<i16>,
    ) -> Option<Vec<Cell>> {
        Some(grid::read_cells(&self.term.lock(), rows, columns))
    }
    fn title(&self) -> Option<String> {
        self.term.lock().title().map(str::to_owned)
    }
    fn set_title(&mut self, title: &str) -> io::Result<()> {
        self.term.lock().set_title(Some(title.to_owned()));
        Ok(())
    }
    fn cancellation(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        let writer = self.writer.clone();
        Some(Arc::new(move || writer.cancel()))
    }
    fn ended(&mut self) {
        self.writer.end();
    }
    fn input_readiness(&self) -> Option<Arc<dyn Fn(bool) + Send + Sync>> {
        let writer = self.writer.clone();
        Some(Arc::new(move |ready| writer.set_input_ready(ready)))
    }
}
struct Console {
    session: Session,
    output: OutputWriter,
}
impl HostedConsole for Console {
    fn child_pid(&self) -> u32 {
        self.session.child_pid()
    }
    fn child_handle(&self) -> io::Result<std::os::windows::io::OwnedHandle> {
        self.session.child_handle()
    }
    fn input(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.session.input(bytes)
    }
    fn resize(&mut self, size: WindowSize) -> io::Result<()> {
        self.session
            .resize(size.num_cols.max(1), size.num_lines.max(1));
        Ok(())
    }
    fn close(&mut self) {
        self.output.cancel();
        self.session.close();
    }
}
pub(super) fn open(options: &tty::Options, term: &Arc<AlacrittyTermLock>) -> io::Result<tty::Pty> {
    let (writer, reader) = queue::channel();
    let (_, command) = tty::windows::command(options)?;
    let session = Session::spawn(
        Launch {
            command,
            environment: options
                .env
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
            directory: options
                .working_directory
                .as_ref()
                .map(|path| tty::windows::normalize_working_directory(path)),
            palette: Some(grid::console_palette(options.console_palette)),
            code_page: None,
        },
        Output {
            writer: writer.clone(),
            term: Arc::clone(term),
        },
    )?;
    tty::windows::from_hosted(HostedPty {
        console: Box::new(Console {
            session,
            output: writer,
        }),
        output: Box::new(reader),
    })
}

#[cfg(test)]
#[path = "tests/hosted_console.rs"]
mod tests;
