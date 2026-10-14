//! Optimized headless probe of the embedded transport and real parser thread.
//! The GUI adapter has its own native integration test; this isolates overlap.
use crate::options::Options;
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, ReplayBarrier};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::hosted_console::queue::{self, OutputWriter};
use alacritty_terminal::hosted_console::{HostedConsole, HostedPty, OutputMode};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term, test::TermSize};
use console_server::{ConsoleOutput, Launch, Session};
use std::io;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};
use vte::ansi::Handler;

/// Child exit code, and whether the backend failed: a shutdown must fail the
/// probe rather than pass as a fast run.
#[derive(Clone)]
struct Listener(Arc<AtomicI32>, Arc<AtomicBool>);
impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::ChildExit(code) => self
                .0
                .store(code.code().unwrap_or(i32::MIN), Ordering::Release),
            Event::BackendShutdown => self.1.store(true, Ordering::Release),
            _ => {}
        }
    }
}
struct Output {
    writer: OutputWriter,
    term: Arc<FairMutex<Term<Listener>>>,
    bytes: Arc<AtomicU64>,
    initial: bool,
}
impl ConsoleOutput for Output {
    fn write(&mut self, bytes: &[u8], mode: console_server::OutputMode) -> io::Result<()> {
        self.writer.write(
            bytes,
            OutputMode {
                lf_implies_cr: mode.lf_implies_cr,
                wrap: mode.wrap,
            },
        )?;
        if !self.initial {
            self.bytes.fetch_add(bytes.len() as u64, Ordering::Relaxed);
        }
        self.initial = false;
        Ok(())
    }
    fn flush(&self) -> io::Result<()> {
        self.writer.flush()
    }
    fn cursor(&self) -> (i16, i16) {
        let point = self.term.lock().grid().cursor.point;
        (point.column.0 as i16, point.line.0 as i16)
    }
    fn size(&self) -> (i16, i16) {
        let term = self.term.lock();
        (term.columns() as i16, term.screen_lines() as i16)
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
    fn input_readiness(&self) -> Option<Arc<dyn Fn(bool) + Send + Sync>> {
        let writer = self.writer.clone();
        Some(Arc::new(move |ready| writer.set_input_ready(ready)))
    }
    fn cancellation(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        let writer = self.writer.clone();
        Some(Arc::new(move || writer.cancel()))
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
pub fn run(options: Options) -> io::Result<()> {
    if options.drain || options.capture.is_some() || options.ctrl_c || options.control_child {
        return Err(io::Error::other(
            "--pipeline requires --command and does not support drain, capture or control probes",
        ));
    }
    let command = options
        .command
        .ok_or_else(|| io::Error::other("--pipeline requires --command"))?;
    let started = Instant::now();
    let listener = Listener(
        Arc::new(AtomicI32::new(i32::MIN)),
        Arc::new(AtomicBool::new(false)),
    );
    let bytes = Arc::new(AtomicU64::new(0));
    let term = Arc::new(FairMutex::new(Term::new(
        Config {
            scrolling_history: options.history,
            ..Default::default()
        },
        &TermSize::new(80, 24),
        listener.clone(),
    )));
    let (writer, reader) = queue::channel();
    let session = Session::spawn(
        Launch {
            command,
            environment: Default::default(),
            directory: None,
            ..Default::default()
        },
        Output {
            writer,
            term: term.clone(),
            bytes: bytes.clone(),
            initial: true,
        },
    )?;
    let pty = alacritty_terminal::tty::windows::from_hosted(HostedPty {
        console: Box::new(Console(session)),
        output: Box::new(reader),
    })?;
    let event_loop = EventLoop::new(
        term,
        listener.clone(),
        pty,
        true,
        false,
        ReplayBarrier::open(),
    )?;
    let sender = event_loop.channel();
    let join = event_loop.spawn();
    let mut timed_out = false;
    while !join.is_finished() {
        if started.elapsed() >= Duration::from_secs(options.timeout) {
            let _ = sender.send(Msg::Shutdown);
            timed_out = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let result = join
        .join()
        .map_err(|_| io::Error::other("parser event loop panicked"))?;
    drop(result);
    let elapsed = started.elapsed().as_secs_f64();
    let code = listener.0.load(Ordering::Acquire);
    if timed_out {
        return Err(io::Error::other("pipeline child timed out"));
    }
    if listener.1.load(Ordering::Acquire) {
        return Err(io::Error::other("hosted backend shut down"));
    }
    if code != 0 {
        return Err(io::Error::other(format!(
            "pipeline child exited with {code}"
        )));
    }
    let count = bytes.load(Ordering::Relaxed);
    let report = format!(
        "{{\n  \"version\": 1,\n  \"backend\": \"embedded-parser-thread\",\n  \"history\": {},\n  \"source_bytes\": {count},\n  \"elapsed_seconds\": {elapsed:.6},\n  \"child_exit_code\": {code}\n}}\n",
        options.history
    );
    if let Some(path) = options.report {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, &report)?;
    }
    print!("{report}");
    Ok(())
}
