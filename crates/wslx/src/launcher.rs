//! The `wslx.exe` process: `wsl.exe`, with the agent relay brought up first
//! when it applies.

use std::{
    env,
    ffi::{OsStr, OsString},
    fs::{File, OpenOptions},
    io::{self, BufReader, Read, Write},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use crate::{
    args::{self, Invocation},
    bootstrap::{self, RelayImage, RelayImages},
    bridge,
    environment::{self, AGENT_VARIABLE},
    help, lock,
};

static RELAY_X86_64: &[u8] = include_bytes!(env!("WSLX_RELAY_X86_64"));
static RELAY_AARCH64: &[u8] = include_bytes!(env!("WSLX_RELAY_AARCH64"));

/// How long the relay may take to come up before the session starts without
/// it. Generous, because the first `wsl.exe` after a shutdown also boots the
/// virtual machine, which the session would have waited for anyway.
const RELAY_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a busy agent pipe is retried. A pipe server re-creates its
/// listening instance after each connection, so busy is brief.
const PIPE_BUSY_TIMEOUT: Duration = Duration::from_secs(2);

const ERROR_PIPE_BUSY: i32 = 231;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CTRL_C_EVENT: u32 = 0;
const CTRL_BREAK_EVENT: u32 = 1;

/// How much of the relay's stderr is kept to explain a failure.
const MAX_DIAGNOSTICS: usize = 4096;

/// Runs `wsl.exe` with this process's arguments and returns its exit code.
pub fn run() -> i32 {
    ignore_console_interrupts();
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let wsl = wsl_executable();
    let mut command = Command::new(&wsl);
    command.args(&args);
    match args::classify(&args) {
        Invocation::Help => print_help(&mut command),
        Invocation::Session { target } => {
            if let Some(socket) = start_agent_relay(&wsl, &target) {
                let wslenv = env::var_os("WSLENV");
                let wslenv = wslenv.as_deref().map(OsStr::to_string_lossy);
                command
                    .env(AGENT_VARIABLE, socket)
                    .env("WSLENV", environment::wslenv_with_agent(wslenv.as_deref()));
            }
        }
        Invocation::Management => {}
    }
    run_wsl(&mut command, &wsl)
    // The relay's stdin closes with this process, which is what stops it.
}

/// Prints what `wslx.exe` adds, ahead of the help `command` prints.
fn print_help(command: &mut Command) {
    let mut stdout = io::stdout().lock();
    let _ = stdout
        .write_all(help::HELP.as_bytes())
        .and_then(|()| stdout.flush());
    // Redirected, `wsl.exe` writes UTF-16 unless told otherwise, which would
    // garble `wslx --help | more` after the UTF-8 above.
    command.env("WSL_UTF8", "1");
}

fn run_wsl(command: &mut Command, wsl: &Path) -> i32 {
    match command.status() {
        // `wsl.exe` reports the Linux command's status, which can use all 32
        // bits; `std::process::exit` passes them on unchanged.
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            eprintln!("wslx: could not start {}: {error}", wsl.display());
            1
        }
    }
}

/// Brings the relay up for a session started with `target` and returns its
/// socket, or `None` — after saying why, when it was attempted — so the
/// session starts without an agent.
fn start_agent_relay(wsl: &Path, target: &[&OsStr]) -> Option<String> {
    let pipe = environment::agent_pipe(&env::var_os(AGENT_VARIABLE)?)?;
    match spawn_relay(wsl, target, pipe) {
        Ok(socket) => Some(socket),
        Err(error) => {
            eprintln!("wslx: the SSH agent is not available inside WSL: {error}");
            None
        }
    }
}

fn spawn_relay(wsl: &Path, target: &[&OsStr], pipe: String) -> io::Result<String> {
    let images = RelayImages {
        x86_64: RelayImage::new(RELAY_X86_64),
        aarch64: RelayImage::new(RELAY_AARCH64),
    };
    // No console: the relay must neither draw on the session's console nor
    // receive its Ctrl+C.
    let mut helper = Command::new(wsl)
        .args(target)
        .args(["--cd", "~", "--exec", "/bin/sh", "-c", bootstrap::SCRIPT])
        .args(images.script_arguments())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()?;
    let (Some(mut stdin), Some(stdout), Some(stderr)) = (
        helper.stdin.take(),
        helper.stdout.take(),
        helper.stderr.take(),
    ) else {
        unreachable!("every stream was requested as a pipe");
    };
    let diagnostics = Arc::new(Mutex::new(String::new()));
    let stderr_reader = collect_diagnostics(stderr, Arc::clone(&diagnostics));

    let (ready, outcome) = mpsc::channel();
    thread::Builder::new()
        .name("wslx-relay".to_owned())
        .spawn(move || {
            let mut stdout = BufReader::new(stdout);
            match bootstrap::handshake(&mut stdout, &mut stdin, &images) {
                Ok(socket) => {
                    let _ = ready.send(Ok(socket));
                    let _ = bridge::run(stdout, stdin, move || open_pipe(&pipe));
                }
                Err(error) => {
                    let _ = ready.send(Err(error));
                }
            }
        })?;

    match outcome.recv_timeout(RELAY_TIMEOUT) {
        Ok(Ok(socket)) => Ok(socket),
        Ok(Err(error)) => Err(explain(error, &mut helper, stderr_reader, &diagnostics)),
        Err(_) => {
            let timed_out = io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "the relay did not start within {}s",
                    RELAY_TIMEOUT.as_secs()
                ),
            );
            Err(explain(timed_out, &mut helper, stderr_reader, &diagnostics))
        }
    }
}

/// Stops the helper and adds what it printed on stderr to `error`.
fn explain(
    error: io::Error,
    helper: &mut Child,
    stderr_reader: Option<thread::JoinHandle<()>>,
    diagnostics: &Mutex<String>,
) -> io::Error {
    let _ = helper.kill();
    let _ = helper.wait();
    if let Some(reader) = stderr_reader {
        let _ = reader.join();
    }
    let diagnostics = lock(diagnostics);
    let diagnostics = diagnostics.trim();
    if diagnostics.is_empty() {
        error
    } else {
        io::Error::new(error.kind(), format!("{error}\n{diagnostics}"))
    }
}

/// Reads the helper's stderr for as long as it is open — so a chatty helper
/// can never block on a full pipe — keeping the start of it.
fn collect_diagnostics(
    mut stderr: impl Read + Send + 'static,
    diagnostics: Arc<Mutex<String>>,
) -> Option<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("wslx-relay-stderr".to_owned())
        .spawn(move || {
            let mut buffer = [0_u8; 1024];
            while let Ok(read) = stderr.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                let mut kept = lock(&diagnostics);
                if kept.len() < MAX_DIAGNOSTICS {
                    kept.push_str(&String::from_utf8_lossy(&buffer[..read]));
                }
            }
        })
        .ok()
}

fn open_pipe(pipe: &str) -> io::Result<File> {
    let deadline = Instant::now() + PIPE_BUSY_TIMEOUT;
    loop {
        match OpenOptions::new().read(true).write(true).open(pipe) {
            Err(error)
                if error.raw_os_error() == Some(ERROR_PIPE_BUSY) && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(10));
            }
            opened => return opened,
        }
    }
}

/// `wsl.exe` from the system directory, so one beside `wslx.exe` or early on
/// `PATH` is not picked up instead; `PATH` is the fallback for a system that
/// has moved it.
fn wsl_executable() -> PathBuf {
    env::var_os("SystemRoot")
        .map(|root| PathBuf::from(root).join("System32").join("wsl.exe"))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("wsl.exe"))
}

/// Ctrl+C and Ctrl+Break reach every process on the console. `wsl.exe` turns
/// them into a signal for the Linux command; `wslx.exe` has to outlive that
/// command to report its status, so it ignores them. A handler that claims
/// the event is used rather than ignoring interrupts outright, because the
/// latter is inherited by `wsl.exe`.
fn ignore_console_interrupts() {
    unsafe extern "system" fn handler(event: u32) -> i32 {
        i32::from(event == CTRL_C_EVENT || event == CTRL_BREAK_EVENT)
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetConsoleCtrlHandler(
            handler: Option<unsafe extern "system" fn(u32) -> i32>,
            add: i32,
        ) -> i32;
    }
    // SAFETY: registers a handler that only reads its argument, for the life
    // of the process.
    unsafe {
        SetConsoleCtrlHandler(Some(handler), 1);
    }
}
