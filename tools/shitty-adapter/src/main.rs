//! Headless bridge from the imported conformance harness to Zetta's terminal core.
//!
//! This executable deliberately has no GUI or production CLI entry point. Each
//! process owns one terminal; JSON lines keep arbitrary terminal bytes off the
//! control channel. It uses the same local Alacritty and VTE forks as Zetta.

use std::cell::{Cell, RefCell};
use std::io::{self, BufRead, Write};
use std::rc::Rc;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::{Config, Term, TermMode, test::TermSize};
use alacritty_terminal::vte::ansi::Processor;
use serde_json::{Value, json};

mod snapshot;

#[derive(Clone, Default)]
struct Events {
    bytes: Rc<RefCell<Vec<u8>>>,
    unavailable: Rc<RefCell<Vec<&'static str>>>,
    /// Grid size for pixel replies, updated before the engine resizes so a
    /// report sent during the resize describes the new grid.
    grid: Rc<Cell<(u16, u16)>>,
}

impl EventListener for Events {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => self.bytes.borrow_mut().extend_from_slice(text.as_bytes()),
            Event::PtyWriteRaw(bytes) => self.bytes.borrow_mut().extend_from_slice(&bytes),
            Event::ColorRequest(..) => self.unavailable.borrow_mut().push("frontend color reply"),
            // The harness models a headless frontend with one-pixel glyphs
            // and no border (`glyph_px`/`glyph_py` default to 1).
            Event::TextAreaSizeRequest(format) => {
                let (num_cols, num_lines) = self.grid.get();
                let size = WindowSize {
                    num_lines,
                    num_cols,
                    cell_width: 1,
                    cell_height: 1,
                };
                self.bytes
                    .borrow_mut()
                    .extend_from_slice(format(size).as_bytes());
            }
            Event::ClipboardLoad(..) => self
                .unavailable
                .borrow_mut()
                .push("frontend clipboard reply"),
            Event::ResizeRequest { .. } => {
                self.unavailable.borrow_mut().push("frontend window resize")
            }
            _ => (),
        }
    }
}

struct Adapter {
    terminal: Term<Events>,
    parser: Processor,
    events: Events,
}

impl Adapter {
    fn new(columns: usize, rows: usize, history: usize) -> Self {
        let events = Events::default();
        events.grid.set((columns as u16, rows as u16));
        // As a default Zetta build configures a pty's terminal. Assembled
        // notifications go to the frontend, which this adapter does not model.
        let config = Config {
            scrolling_history: history,
            desktop_notifications: true,
            ..Config::default()
        };
        Self {
            terminal: Term::new(config, &TermSize::new(columns, rows), events.clone()),
            parser: Processor::new(),
            events,
        }
    }

    fn request(&mut self, request: &Value) -> Result<Value, String> {
        match request["op"].as_str().ok_or("missing operation")? {
            "write" => self.write(request),
            "snapshot" => Ok(snapshot::snapshot(&self.terminal)),
            "hyperlink" => self.hyperlink(request),
            "read_input" => Ok(json!({
                "bytes": std::mem::take(&mut *self.events.bytes.borrow_mut()),
                "unavailable": std::mem::take(&mut *self.events.unavailable.borrow_mut()),
            })),
            "resize" => {
                let size = size(request)?;
                self.events
                    .grid
                    .set((size.columns as u16, size.screen_lines as u16));
                self.terminal.resize(size);
                Ok(Value::Null)
            }
            "scroll" => {
                if self.terminal.mode().contains(TermMode::ALT_SCREEN) {
                    return Ok(
                        json!({"unavailable": "wheel input on an alternate screen requires the frontend input path"}),
                    );
                }
                let delta = request["delta"].as_i64().ok_or("missing scroll delta")?;
                let delta = i32::try_from(delta).map_err(|error| error.to_string())?;
                self.terminal.scroll_display(Scroll::Delta(delta));
                Ok(Value::Null)
            }
            operation => Err(format!("unknown operation: {operation}")),
        }
    }

    fn hyperlink(&self, request: &Value) -> Result<Value, String> {
        let grid = self.terminal.grid();
        let column = request["column"].as_u64().ok_or("invalid column")? as usize;
        let row = request["row"].as_u64().ok_or("invalid row")? as usize;
        if column >= grid.columns() || row >= grid.screen_lines() {
            return Err("cell coordinate is outside the page".into());
        }
        let cell = &grid[Line(row as i32 - grid.display_offset() as i32)][Column(column)];
        Ok(json!(cell.hyperlink().map(|link| link.uri().to_owned())))
    }

    fn write(&mut self, request: &Value) -> Result<Value, String> {
        let data: Vec<u8> =
            serde_json::from_value(request["data"].clone()).map_err(|error| error.to_string())?;
        self.parser.advance(&mut self.terminal, &data);
        Ok(Value::Null)
    }
}

fn size(request: &Value) -> Result<TermSize, String> {
    let dimension = |key: &str| {
        request[key]
            .as_u64()
            .filter(|value| (1..=65_535).contains(value))
            .map(|value| value as usize)
            .ok_or_else(|| format!("invalid {key}"))
    };
    let columns = dimension("columns")?;
    let rows = dimension("rows")?;
    if columns * rows > 1_000_000 {
        return Err("terminal exceeds one million cells".into());
    }
    Ok(TermSize::new(columns, rows))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut adapter = None;
    let mut output = io::stdout().lock();
    for line in io::stdin().lock().lines() {
        let request: Value = serde_json::from_str(&line?)?;
        let result = if request["op"] == "new" {
            size(&request).map(|size| {
                adapter = Some(Adapter::new(
                    size.columns,
                    size.screen_lines,
                    request["history"].as_u64().unwrap_or(500).min(100_000) as usize,
                ));
                Value::Null
            })
        } else {
            adapter
                .as_mut()
                .ok_or_else(|| "terminal not initialized".to_owned())
                .and_then(|adapter| adapter.request(&request))
        };
        let response = match result {
            Ok(value) => json!({"result": value}),
            Err(error) => json!({"error": error}),
        };
        writeln!(output, "{response}")?;
        output.flush()?;
    }
    Ok(())
}
