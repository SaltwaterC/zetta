//! Serial parser used by the standalone probe, with byte-preserving capture.
//! Embedded panes use the existing reader/parser overlap instead.
use crate::output::{ConsoleOutput, OutputMode};
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::term::{Config, Term, test::TermSize};
use std::fs::File;
use std::io::{self, Write};
use vte::ansi::{Processor, StdSyncHandler};

pub struct Sink {
    term: Option<Term<VoidListener>>,
    processor: Processor<StdSyncHandler>,
    capture: Option<File>,
    pub bytes: u64,
}
impl Sink {
    pub fn new(drain: bool, capture: Option<&std::path::Path>, history: usize) -> io::Result<Self> {
        let term = if drain {
            None
        } else {
            Some(Term::new(
                Config {
                    scrolling_history: history,
                    ..Config::default()
                },
                &TermSize::new(80, 24),
                VoidListener,
            ))
        };
        Ok(Self {
            term,
            processor: Processor::new(),
            capture: capture.map(File::create).transpose()?,
            bytes: 0,
        })
    }
    #[cfg(test)]
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        ConsoleOutput::write(self, bytes, OutputMode::from_console_flags(7))
    }
}
impl ConsoleOutput for Sink {
    fn write(&mut self, bytes: &[u8], mode: OutputMode) -> io::Result<()> {
        if let Some(capture) = &mut self.capture {
            capture.write_all(bytes)?;
        }
        if let Some(term) = &mut self.term {
            term.set_output_mode(alacritty_terminal::hosted_console::OutputMode {
                lf_implies_cr: mode.lf_implies_cr,
                wrap: mode.wrap,
            });
            self.processor.advance(term, bytes);
        }
        self.bytes += bytes.len() as u64;
        Ok(())
    }
    fn cursor(&self) -> (i16, i16) {
        self.term.as_ref().map_or((0, 0), |term| {
            let point = term.grid().cursor.point;
            (point.column.0 as i16, point.line.0 as i16)
        })
    }
    fn screen(&self) -> crate::output::Screen {
        self.term.as_ref().map_or_else(
            || crate::output::Screen {
                columns: 80,
                rows: 24,
                cursor: (0, 0),
                cursor_visible: true,
                pen: None,
                wrap_pending: false,
            },
            crate::grid::screen,
        )
    }
    fn title(&self) -> Option<String> {
        self.term
            .as_ref()
            .and_then(|term| term.title().map(str::to_owned))
    }
    fn set_title(&mut self, title: &str) -> io::Result<()> {
        if let Some(term) = &mut self.term {
            vte::ansi::Handler::set_title(term, Some(title.to_owned()));
        }
        Ok(())
    }
    fn bytes(&self) -> u64 {
        self.bytes
    }
}
#[cfg(test)]
#[path = "tests/sink.rs"]
mod tests;
