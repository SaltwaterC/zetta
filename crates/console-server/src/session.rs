//! Per-console lifetime for embedding: spawn once, feed output, cancel and join.
//! No helper executable or user-mode transport process is started.
use crate::input::{Input, SharedOutput};
use crate::native::{Child, Console, JobScope};
use crate::output::{ConsoleOutput, OutputMode};
use crate::server::Server;
use crate::state::{ConsoleState, Palette};
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Launch {
    pub command: String,
    pub environment: HashMap<String, String>,
    pub directory: Option<PathBuf>,
    /// The color table and default colors; ConPTY's Campbell scheme if None.
    pub palette: Option<Palette>,
    /// The console code page; the OEM code page, as ConPTY inherits, if None.
    pub code_page: Option<u32>,
}
struct SharedSink(SharedOutput);
impl ConsoleOutput for SharedSink {
    fn write(&mut self, bytes: &[u8], mode: OutputMode) -> io::Result<()> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .write(bytes, mode)
    }
    fn flush(&self) -> io::Result<()> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .flush()
    }
    fn cursor(&self) -> (i16, i16) {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .cursor()
    }
    fn size(&self) -> (i16, i16) {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .size()
    }
    fn read_cells(
        &self,
        rows: std::ops::Range<i16>,
        columns: std::ops::Range<i16>,
    ) -> Option<Vec<crate::output::Cell>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .read_cells(rows, columns)
    }
    fn screen(&self) -> crate::output::Screen {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .screen()
    }
    fn title(&self) -> Option<String> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .title()
    }
    fn set_title(&mut self, title: &str) -> io::Result<()> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .set_title(title)
    }
    fn ended(&mut self) {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .ended();
    }
}
pub struct Session {
    cancellation: Option<Arc<dyn Fn() + Send + Sync>>,
    pid: u32,
    process: crate::native::Handle,
    input: Input,
    palette: Arc<Mutex<Option<Palette>>>,
    /// What close hands to the reaper: the child's job and the threads.
    child: Option<Child>,
    service: Option<JoinHandle<io::Result<()>>>,
    input_thread: Option<JoinHandle<()>>,
}
/// How long clients get to handle CTRL_CLOSE_EVENT: CSRSS's own five
/// seconds, and a little more before we end the ones still attached.
const CLOSE_GRACE: Duration = Duration::from_millis(5500);
impl Session {
    pub fn spawn(launch: Launch, output: impl ConsoleOutput + 'static) -> io::Result<Self> {
        let cancellation = output.cancellation();
        let readiness = output.input_readiness();
        let console = Console::new()?;
        let child = Child::spawn_with_launch(&console, &launch, JobScope::Child)?;
        let shared: SharedOutput = Arc::new(Mutex::new(Box::new(output)));
        let input_event = console._input_event.duplicate()?;
        let server_handle = Arc::new(console.server);
        let state = ConsoleState::new(
            launch.code_page.unwrap_or_else(crate::codepage::default),
            launch.palette.unwrap_or_default(),
        );
        let mut server = Server::with_state(SharedSink(Arc::clone(&shared)), false, state);
        server.window_output = Some(Arc::clone(&shared));
        let input = Input::new(
            Arc::clone(&server_handle),
            input_event,
            shared,
            readiness,
            Arc::clone(&server.editing),
        );
        server.input = Some(input.clone());
        server.modes_changed();
        let palette = Arc::clone(&server.pending_palette);
        // Keyboard records are an input contract of this backend, not a
        // request that depends on the UI servicing startup output.
        server.start_terminal_input()?;
        let worker_input = input.clone();
        let input_thread = std::thread::Builder::new()
            .name("console-input".into())
            .spawn(move || worker_input.run())?;
        drop(console.reference);
        let service_input = input.clone();
        let service = std::thread::Builder::new()
            .name("console-service".into())
            .spawn(move || {
                let _event = console._input_event;
                let result = server.serve(&server_handle);
                service_input.close();
                if result.is_err() {
                    server.terminate_clients();
                    server.sink.ended();
                }
                result
            });
        let service = match service {
            Ok(service) => service,
            Err(error) => {
                input.close();
                let _ = input_thread.join();
                return Err(error);
            }
        };
        Ok(Self {
            cancellation,
            pid: child.pid,
            process: child.process.duplicate()?,
            input,
            palette,
            child: Some(child),
            service: Some(service),
            input_thread: Some(input_thread),
        })
    }
    pub fn child_pid(&self) -> u32 {
        self.pid
    }
    pub fn child_handle(&self) -> io::Result<std::os::windows::io::OwnedHandle> {
        self.process.duplicate_owned()
    }
    pub fn input(&self, bytes: &[u8]) -> io::Result<usize> {
        self.input.write(bytes)
    }
    pub fn resize(&self, columns: u16, rows: u16) {
        self.input.resize(columns, rows);
    }
    /// The pane's colors changed; the console reports the new table.
    pub fn set_palette(&self, palette: Palette) {
        *self
            .palette
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(palette);
    }
    /// Closes the console as closing its window does: output stops, every
    /// client gets CTRL_CLOSE_EVENT, and this returns at once. A reaper
    /// waits out the clients' grace period, ends the ones still attached,
    /// then joins the threads and closes the child's job.
    pub fn close(&mut self) {
        let Some(child) = self.child.take() else {
            return;
        };
        if let Some(cancel) = self.cancellation.take() {
            cancel();
        }
        self.input.close();
        for pid in self.input.client_pids() {
            let _ = crate::control::send_ctrl_event(pid, crate::control::CTRL_CLOSE_EVENT, 0);
        }
        let service = self.service.take();
        let input_thread = self.input_thread.take();
        let input = self.input.clone();
        let reap = move || {
            let deadline = Instant::now() + CLOSE_GRACE;
            while service.as_ref().is_some_and(|thread| !thread.is_finished())
                && Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            if service.as_ref().is_some_and(|thread| !thread.is_finished()) {
                for pid in input.client_pids() {
                    crate::native::terminate_process(pid);
                }
            }
            if let Some(thread) = input_thread {
                let _ = thread.join();
            }
            if let Some(thread) = service {
                let _ = thread.join();
            }
            drop(child);
        };
        if std::thread::Builder::new()
            .name("console-close".into())
            .spawn(reap)
            .is_err()
        {
            eprintln!("console-server: could not start the close reaper");
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}
