//! Stand-in for OpenConsole.exe in Microsoft's console feature tests
//! (src/host/ft_host), which `conformance/conformance.ps1` runs. Their
//! ModuleSetup starts `OpenConsole.exe Nihilist.exe` from the deployment
//! directory and attaches to Nihilist's console, so this hosts that client in
//! a 120x30 terminal on the backend `backend.txt` beside it names:
//! `console-server` (the default) or `conpty`, for comparison.
//!
//! A Windows-subsystem program, so that the CREATE_NEW_CONSOLE the tests start
//! it with does not give it a console of its own.
#![windows_subsystem = "windows"]

#[cfg(windows)]
mod host {
    use alacritty_terminal::event::{Event, EventListener, WindowSize};
    use alacritty_terminal::event_loop::{EventLoop, Msg, ReplayBarrier};
    use alacritty_terminal::hosted_console::queue::{self, OutputWriter};
    use alacritty_terminal::hosted_console::{HostedConsole, HostedPty, OutputMode};
    use alacritty_terminal::sync::FairMutex;
    use alacritty_terminal::term::{Config, Term, test::TermSize};
    use alacritty_terminal::tty;
    use alacritty_terminal::vte::ansi::{Handler, Rgb};
    use console_server::output::Cell;
    use console_server::{ConsoleOutput, Launch, Screen, Session, grid};
    use std::io::{self, Write};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, mpsc};

    const COLUMNS: u16 = 120;
    const LINES: u16 = 30;

    #[derive(Clone)]
    struct Listener(mpsc::Sender<Event>);
    impl EventListener for Listener {
        fn send_event(&self, event: Event) {
            let _ = self.0.send(event);
        }
    }

    /// The pane side of the hosted console: output into the terminal's parser
    /// thread, screen queries answered from its grid, as Zetta's panes do.
    struct Output {
        writer: OutputWriter,
        term: Arc<FairMutex<Term<Listener>>>,
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
        fn ended(&mut self) {
            self.writer.end();
        }
        fn cancellation(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
            let writer = self.writer.clone();
            Some(Arc::new(move || writer.cancel()))
        }
        fn input_readiness(&self) -> Option<Arc<dyn Fn(bool) + Send + Sync>> {
            let writer = self.writer.clone();
            Some(Arc::new(move |ready| writer.set_input_ready(ready)))
        }
    }

    struct Console(Session);
    impl HostedConsole for Console {
        fn child_pid(&self) -> u32 {
            self.0.child_pid()
        }
        fn child_handle(&self) -> io::Result<std::os::windows::io::OwnedHandle> {
            self.0.child_handle()
        }
        fn input(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.input(bytes)
        }
        fn resize(&mut self, size: WindowSize) -> io::Result<()> {
            self.0.resize(size.num_cols, size.num_lines);
            Ok(())
        }
        fn close(&mut self) {
            self.0.close();
        }
    }

    /// There is no console to report to, so a log beside the executable.
    fn log(directory: &Path, line: &str) {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(directory.join("fthost.log"))
        {
            let _ = writeln!(file, "{line}");
        }
    }

    pub fn main() {
        let Some(directory) = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
        else {
            return;
        };
        if let Err(error) = run(&directory) {
            log(&directory, &format!("error: {error}"));
        }
    }

    fn run(directory: &Path) -> io::Result<()> {
        let backend = std::fs::read_to_string(directory.join("backend.txt")).map_or_else(
            |_| "console-server".to_owned(),
            |text| text.trim().to_owned(),
        );
        let mut words: Vec<String> = std::env::args().skip(1).collect();
        if words.is_empty() {
            return Err(io::Error::other("no client named"));
        }
        // The tests name the client relative to the deployment directory.
        let program = words.remove(0);
        let resolved: PathBuf = directory.join(&program);
        let program = if resolved.exists() {
            resolved.to_string_lossy().into_owned()
        } else {
            program
        };
        log(
            directory,
            &format!("starting {program} {words:?} on {backend}"),
        );

        let (sender, events) = mpsc::channel();
        let listener = Listener(sender);
        let size = WindowSize {
            num_lines: LINES,
            num_cols: COLUMNS,
            cell_width: 8,
            cell_height: 16,
        };
        // conhost's default history, which the buffer tests size against.
        let config = Config {
            scrolling_history: 9001,
            ..Config::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(
            config,
            &TermSize::new(usize::from(COLUMNS), usize::from(LINES)),
            listener.clone(),
        )));
        let options = tty::Options {
            shell: Some(tty::Shell::new(program, words)),
            escape_args: true,
            ..tty::Options::default()
        };
        let pty = if backend == "conpty" {
            tty::new(&options, size, 0)?
        } else {
            let (writer, reader) = queue::channel();
            let (_, command) = tty::windows::command(&options)?;
            let session = Session::spawn(
                Launch {
                    command,
                    environment: Default::default(),
                    directory: None,
                    ..Default::default()
                },
                Output {
                    writer,
                    term: Arc::clone(&term),
                },
            )?;
            tty::windows::from_hosted(HostedPty {
                console: Box::new(Console(session)),
                output: Box::new(reader),
            })?
        };
        let event_loop = EventLoop::new(term, listener, pty, true, false, ReplayBarrier::open())?;
        let input = event_loop.channel();
        let _worker = event_loop.spawn();
        while let Ok(event) = events.recv() {
            match event {
                Event::ChildExit(status) => {
                    log(directory, &format!("client exited {:?}", status.code()));
                    break;
                }
                Event::BackendShutdown | Event::ChildExitStatusUnavailable => break,
                Event::PtyWriteRaw(reply) => {
                    let _ = input.send(Msg::Input(reply.into()));
                }
                Event::PtyWrite(reply) => {
                    let _ = input.send(Msg::Input(reply.into_bytes().into()));
                }
                Event::TextAreaSizeRequest(reply) => {
                    let _ = input.send(Msg::Input(reply(size).into_bytes().into()));
                }
                Event::ColorRequest(_, reply) => {
                    let gray = Rgb {
                        r: 128,
                        g: 128,
                        b: 128,
                    };
                    let _ = input.send(Msg::Input(reply(gray).into_bytes().into()));
                }
                _ => {}
            }
        }
        Ok(())
    }
}

fn main() {
    #[cfg(windows)]
    host::main();
}
