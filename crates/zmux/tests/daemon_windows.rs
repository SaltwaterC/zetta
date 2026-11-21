//! End-to-end Windows coverage for the daemon/host upgrade boundary.

#![cfg(windows)]

use std::{
    collections::HashMap,
    io::{Read, Write},
    os::windows::io::AsRawHandle,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use alacritty_terminal::tty::ConsolePalette;

use zmux::{
    client::{AttachOutcome, Client},
    messages::{
        CreateSharedRequest, SharedDraftLayout, SharedPaneDraft, SharedPaneRef, SpawnRequest,
        TerminalSize,
    },
    protocol::{
        BackgroundPaneLayout, BackgroundPaneState, BackgroundPaneSummary, BackgroundSessionSummary,
    },
};

#[path = "daemon_windows/cleanup.rs"]
mod cleanup;

#[path = "daemon_windows/ctrl_c.rs"]
mod ctrl_c;

#[path = "daemon_windows/console_server.rs"]
mod console_server;

#[path = "daemon_windows/cwd.rs"]
mod cwd;

#[path = "daemon_windows/shared_input.rs"]
mod shared_input;

#[path = "daemon_windows/palette.rs"]
mod palette;

#[path = "daemon_windows/pseudoconsole.rs"]
mod pseudoconsole;

#[path = "daemon_windows/protection.rs"]
mod protection;

#[path = "daemon_windows/host_replacement.rs"]
mod host_replacement;

struct TestDaemon {
    process: Child,
    /// Taken by `drop`, which removes it once the daemon has let go of it.
    directory: Option<tempfile::TempDir>,
    config: PathBuf,
}

impl TestDaemon {
    fn start() -> Self {
        Self::start_binary(&daemon_binary())
    }

    fn start_binary(binary: &Path) -> Self {
        let directory = short_test_directory();
        let config = directory.path().to_path_buf();
        let process = Command::new(binary)
            .arg("--daemon")
            .env("APPDATA", &config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(config.join("daemon.log")).unwrap())
            .spawn()
            .expect("starting zmux");

        let daemon = Self {
            process,
            directory: Some(directory),
            config,
        };
        daemon.wait_for_endpoint();
        daemon
    }

    fn sessions_dir(&self) -> PathBuf {
        sessions_dir_under(&self.config)
    }

    fn wait_for_endpoint(&self) {
        let _ = self.wait_for_ready();
    }

    fn client(&self) -> Client {
        self.wait_for_ready()
    }

    fn wait_for_ready(&self) -> Client {
        let deadline = Instant::now() + Duration::from_secs(10);
        let endpoint = self.sessions_dir().join("zmux.json");
        let mut last_error = None;
        loop {
            match Client::connect_ready_at(&self.sessions_dir()) {
                Ok(Some(client)) => return client,
                Ok(None) => {}
                Err(error) => last_error = Some(format!("{error:#}")),
            }
            if Instant::now() >= deadline {
                panic!(
                    "the daemon was not ready within 10s (process: {}; endpoint: {}; last error: {})\ndaemon log:\n{}",
                    self.process.id(),
                    endpoint.display(),
                    last_error.as_deref().unwrap_or("none"),
                    std::fs::read_to_string(self.config.join("daemon.log")).unwrap_or_default()
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        // An upgrade replaces `process`. Stop the daemon at this fixture's
        // private endpoint before removing its directory, including on panic:
        // a surviving replacement can keep the stop hook's output pipes open.
        let _ = zmux::stop(&self.sessions_dir(), true);
        let _ = self.process.kill();
        let _ = self.process.wait();
        let _ = zmux::pty_host::stop(&self.sessions_dir(), true);
        if let Some(directory) = self.directory.take() {
            remove_test_directory(directory);
        }
    }
}

/// Where a daemon given `config` as its `APPDATA` keeps its sessions.
fn sessions_dir_under(config: &Path) -> PathBuf {
    config.join("Zetta").join(format!(
        "sessions-debug-v{}",
        zmux::messages::PROTOCOL_VERSION
    ))
}

/// The longest name of a socket a daemon binds in its sessions directory
/// (`zmux::paths::forwarded_agent_socket`), which with the directory is what
/// `uds_windows` has to fit in its `sun_path`.
const LONGEST_SOCKET_NAME: &str = "forwarded-agent.sock";

/// What `uds_windows` accepts: the path and its terminator in 108 bytes.
const SOCKET_PATH_LIMIT: usize = 107;

/// A private `APPDATA` for one daemon, in the system temporary directory.
///
/// Not in this checkout: a directory a test leaves there shows up as untracked
/// work. The system temporary directory is also the shorter of the two in the
/// default profile layout, where a Windows user name is at most 20 characters.
/// A relocated `TMP` that leaves no room for the daemon's sockets is reported
/// as such, rather than as a daemon that never became ready.
fn short_test_directory() -> tempfile::TempDir {
    let directory = tempfile::Builder::new()
        .prefix("zmux")
        .tempdir()
        .expect("creating a Windows test directory");
    let socket = sessions_dir_under(directory.path()).join(LONGEST_SOCKET_NAME);
    assert!(
        socket.as_os_str().len() <= SOCKET_PATH_LIMIT,
        "{} is {} bytes, longer than the {SOCKET_PATH_LIMIT} a Windows socket path can be; \
         point TMP at a shorter directory",
        socket.display(),
        socket.as_os_str().len()
    );
    directory
}

/// Removes a fixture's directory once nothing holds a file in it open.
///
/// `zmux::stop` returns when the daemon stops answering, not when it has
/// exited, and the replacement an upgrade starts is not `process`, so nothing
/// above waits for it. Until it exits its `zmux.lock`, opened with no sharing,
/// cannot be deleted, and `TempDir` ignores a failed removal: that is what left
/// `z*` directories behind. A directory still held after the deadline means a
/// daemon outlived its test, which fails the test rather than passing quietly.
fn remove_test_directory(directory: tempfile::TempDir) {
    let path = directory.keep();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let error = match std::fs::remove_dir_all(&path) {
            Ok(()) => return,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => error,
        };
        if Instant::now() >= deadline {
            let message = format!(
                "{} was still in use 10s after its daemon was stopped: {error}",
                path.display()
            );
            // A second panic while unwinding would abort the whole test binary.
            if std::thread::panicking() {
                eprintln!("{message}");
            } else {
                panic!("{message}");
            }
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn process_is_alive(process_id: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let Ok(process) =
        (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) })
    else {
        return false;
    };
    let mut exit_code = 0;
    let alive = unsafe { GetExitCodeProcess(process, &mut exit_code).is_ok() }
        && exit_code == STILL_ACTIVE.0 as u32;
    unsafe {
        let _ = CloseHandle(process);
    }
    alive
}

fn daemon_binary() -> PathBuf {
    let mut path = std::env::current_exe().expect("locating the test binary");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    let binary = path.join("zmux.exe");
    assert!(
        binary.is_file(),
        "{} is missing; run `cargo build --bin zmux --bin zmux-pty` first",
        binary.display()
    );
    let daemon_source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/server.rs");
    let built = std::fs::metadata(&binary)
        .expect("reading the daemon timestamp")
        .modified()
        .expect("reading the daemon timestamp");
    let edited = std::fs::metadata(&daemon_source)
        .expect("reading the daemon source timestamp")
        .modified()
        .expect("reading the daemon source timestamp");
    assert!(
        built >= edited,
        "{} is older than {}; run `cargo build --bin zmux --bin zmux-pty` so this test exercises the current daemon",
        binary.display(),
        daemon_source.display()
    );
    binary
}

fn spawn_request() -> SpawnRequest {
    let mut env = HashMap::new();
    env.insert("TERM".to_owned(), "xterm-256color".to_owned());
    SpawnRequest {
        session_id: None,
        client_process_id: std::process::id(),
        program: Some("cmd.exe".to_owned()),
        args: vec![
            "/D".to_owned(),
            "/C".to_owned(),
            r#""echo zetta-upgrade & ping 127.0.0.1 -n 60 >NUL""#.to_owned(),
        ],
        env,
        working_directory: None,
        size: TerminalSize {
            columns: 80,
            lines: 24,
            cell_width: 8,
            cell_height: 16,
        },
        console_palette: Default::default(),
        console_backend: Default::default(),
    }
}

fn output_request() -> SpawnRequest {
    let mut request = spawn_request();
    request.program = Some("powershell.exe".to_owned());
    request.args = vec![
        "-NoLogo".to_owned(),
        "-NoProfile".to_owned(),
        "-Command".to_owned(),
        "Write-Output zetta-upgrade; Start-Sleep -Seconds 60".to_owned(),
    ];
    request
}

fn session_summary(session_id: u64, pane_id: u64) -> BackgroundSessionSummary {
    BackgroundSessionSummary {
        id: session_id,
        title: "palette test".to_owned(),
        authentication_required: false,
        active_pane: pane_id,
        layout: BackgroundPaneLayout::Pane { pane_id },
        panes: Vec::new(),
        held: false,
        scoped_to: None,
        key_envelope: None,
    }
}

fn read_until(
    output: &mut std::fs::File,
    input: &mut std::fs::File,
    marker: &[u8],
    timeout: Duration,
) -> Vec<u8> {
    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::new();
    let mut sent_device_attributes = false;
    while Instant::now() < deadline {
        let mut available = 0;
        let peek_result = unsafe {
            windows::Win32::System::Pipes::PeekNamedPipe(
                windows::Win32::Foundation::HANDLE(output.as_raw_handle()),
                None,
                0,
                None,
                Some(&mut available),
                None,
            )
        };
        if peek_result.is_err() {
            // ConPTY can briefly report an unavailable pipe while its
            // attached process is being brought up. Treat that as a retry,
            // not EOF; a genuinely closed pipe still fails the marker
            // assertion once the overall deadline expires.
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        if available > 0 {
            let start = bytes.len();
            bytes.resize(start + available as usize, 0);
            match output.read(&mut bytes[start..]) {
                Ok(length) => bytes.truncate(start + length),
                Err(_) => bytes.truncate(start),
            }
            if !sent_device_attributes && bytes.windows(3).any(|window| window == b"\x1b[c") {
                input
                    .write_all(b"\x1b[?6c")
                    .expect("answering the pseudoconsole device query");
                sent_device_attributes = true;
            }
            if bytes.windows(marker.len()).any(|window| window == marker) {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    bytes
}
