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
    messages::{SpawnRequest, TerminalSize},
    protocol::{BackgroundPaneLayout, BackgroundSessionSummary},
};

struct TestDaemon {
    process: Child,
    _directory: tempfile::TempDir,
    config: PathBuf,
}

impl TestDaemon {
    fn start() -> Self {
        // uds_windows has a short socket-name limit. Keeping the temporary
        // directory beside this checkout also makes the test work on machines
        // whose system temporary directory has a long user profile path.
        let directory = tempfile::Builder::new()
            .prefix("z")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .expect("creating a short Windows test directory");
        let config = directory.path().to_path_buf();
        let process = Command::new(daemon_binary())
            .arg("--daemon")
            .env("APPDATA", &config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(config.join("daemon.log")).unwrap())
            .spawn()
            .expect("starting zmux");

        let daemon = Self {
            process,
            _directory: directory,
            config,
        };
        daemon.wait_for_endpoint();
        daemon
    }

    fn sessions_dir(&self) -> PathBuf {
        self.config.join("Zetta").join(format!(
            "sessions-debug-v{}",
            zmux::messages::PROTOCOL_VERSION
        ))
    }

    fn wait_for_endpoint(&self) {
        let endpoint = self.sessions_dir().join("zmux.json");
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if endpoint.is_file()
                && Client::connect_existing_at(&self.sessions_dir())
                    .ok()
                    .flatten()
                    .is_some()
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the daemon never published {}", endpoint.display());
    }

    fn client(&self) -> Client {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match Client::connect_existing_at(&self.sessions_dir()) {
                Ok(Some(client)) => return client,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Ok(None) => panic!(
                    "the daemon endpoint is not live (process: {}; log: {})",
                    self.process.id(),
                    std::fs::read_to_string(self.config.join("daemon.log")).unwrap_or_default()
                ),
                Err(error) => panic!("looking for the daemon: {error:#}"),
            }
        }
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
        let _ = zmux::pty_host::stop(&self.sessions_dir(), true);
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

fn detach_restore_request() -> SpawnRequest {
    let mut request = spawn_request();
    request.program = Some("powershell.exe".to_owned());
    request.args = vec![
        "-NoLogo".to_owned(),
        "-NoProfile".to_owned(),
        "-Command".to_owned(),
        "Write-Output before; Start-Sleep -Seconds 3; Write-Output after; Start-Sleep -Seconds 10"
            .to_owned(),
    ];
    request
}

fn palette_probe_request(palette: ConsolePalette) -> SpawnRequest {
    let mut request = spawn_request();
    request.console_palette = palette;
    request.program = Some(
        daemon_binary()
            .with_file_name("zmux-pty.exe")
            .to_string_lossy()
            .into_owned(),
    );
    request.args.clear();
    request.env.insert(
        "ZETTA_INTERNAL_CONSOLE_PALETTE_PROBE_V1".to_owned(),
        "1".to_owned(),
    );
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
        let peek_succeeded = unsafe {
            windows::Win32::System::Pipes::PeekNamedPipe(
                windows::Win32::Foundation::HANDLE(output.as_raw_handle()),
                None,
                0,
                None,
                Some(&mut available),
                None,
            )
            .is_ok()
        };
        if !peek_succeeded {
            break;
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

#[test]
fn conpty_palette_is_ready_before_the_child_and_updates_while_attached() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let mut initial = ConsolePalette::default();
    initial.colors[3] = [0x01, 0x02, 0x03];
    initial.foreground_index = 3;
    initial.background_index = 4;
    let pane = client
        .spawn(palette_probe_request(initial))
        .expect("creating the palette probe pane");
    let session_id = pane.session_id;
    let pane_id = pane.pane_id;
    let mut output = std::fs::File::from(pane.conout);
    let mut input = std::fs::File::from(pane.conin);

    let first = read_until(
        &mut output,
        &mut input,
        b"PALETTE:00030201,43",
        Duration::from_secs(10),
    );
    assert!(
        first
            .windows(b"PALETTE:00030201,43".len())
            .any(|bytes| bytes == b"PALETTE:00030201,43"),
        "the child did not observe the initial palette: {:?}",
        String::from_utf8_lossy(&first)
    );

    let mut updated = initial;
    updated.colors[3] = [0xaa, 0xbb, 0xcc];
    updated.foreground_index = 5;
    updated.background_index = 6;
    client
        .set_console_palette(session_id, pane_id, updated)
        .expect("updating the attached pseudoconsole palette");
    input.write_all(b"\r\n").expect("releasing the probe");
    let second = read_until(
        &mut output,
        &mut input,
        b"PALETTE:00ccbbaa,65",
        Duration::from_secs(5),
    );
    assert!(
        second
            .windows(b"PALETTE:00ccbbaa,65".len())
            .any(|bytes| bytes == b"PALETTE:00ccbbaa,65"),
        "the child did not observe the updated palette: {:?}",
        String::from_utf8_lossy(&second)
    );

    drop(output);
    drop(input);
    let _ = client.kill(session_id);
    let _ = client.shutdown();
}

#[test]
fn shared_conpty_accepts_palette_updates_in_viewer_order() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().expect("subscribing to pane revokes");
    let _exit_reporters = subscription.exits.clone();
    let revokes = subscription.revokes.clone();
    let mut initial = ConsolePalette::default();
    initial.colors[3] = [0x01, 0x02, 0x03];
    let pane = client
        .spawn(palette_probe_request(initial))
        .expect("creating the shared palette probe pane");
    let session_id = pane.session_id;
    let pane_id = pane.pane_id;
    let mut output = std::fs::File::from(pane.conout);
    let mut input = std::fs::File::from(pane.conin);
    let first = read_until(
        &mut output,
        &mut input,
        b"PALETTE:00030201",
        Duration::from_secs(10),
    );
    assert!(
        first
            .windows(b"PALETTE:00030201".len())
            .any(|bytes| bytes == b"PALETTE:00030201"),
        "the child did not observe the shared pane's initial palette: {:?}",
        String::from_utf8_lossy(&first)
    );

    client
        .share(
            session_id,
            session_summary(session_id, pane_id),
            serde_json::Value::Null,
            None,
            true,
        )
        .expect("offering the session for shared attachment");
    let (revoke_tx, revoke_rx) = async_channel::unbounded();
    revokes.register(pane_id, revoke_tx);
    let holder = std::thread::spawn({
        let client = daemon.client();
        move || {
            revoke_rx
                .recv_blocking()
                .expect("the holder must receive a revoke");
            drop(output);
            drop(input);
            client
                .send_snapshot(session_id, pane_id, first, 80, 24)
                .expect("handing the probe pane to the shared relay");
            match client
                .attach_as(session_id, pane_id, std::process::id(), None)
                .expect("reattaching the original viewer")
            {
                AttachOutcome::SharedAttached { pane, .. } => pane,
                _ => panic!("the original viewer must reattach in shared mode"),
            }
        }
    });

    let mut second_process = Command::new("cmd.exe")
        .args(["/D", "/C", "ping 127.0.0.1 -n 60 >NUL"])
        .spawn()
        .expect("starting the second viewer stand-in");
    let second_process_id = second_process.id();
    let second = match client
        .attach_as(session_id, pane_id, second_process_id, None)
        .expect("joining the pane as a second viewer")
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _ => panic!("the second viewer must attach in shared mode"),
    };
    let holder = holder.join().expect("the holder handover failed");

    let mut second_palette = initial;
    second_palette.colors[3] = [0x11, 0x22, 0x33];
    client
        .set_console_palette_for_process(session_id, pane_id, second_palette, second_process_id)
        .expect("applying the second viewer's palette");
    let mut latest = second_palette;
    latest.colors[3] = [0xaa, 0xbb, 0xcc];
    latest.foreground_index = 5;
    latest.background_index = 6;
    client
        .set_console_palette(session_id, pane_id, latest)
        .expect("applying the original viewer's newer palette");

    let mut reader = holder.reader();
    holder.send_input(b"\r\n").expect("releasing the probe");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut second_output = Vec::new();
    while Instant::now() < deadline
        && !second_output
            .windows(b"PALETTE:00ccbbaa,65".len())
            .any(|bytes| bytes == b"PALETTE:00ccbbaa,65")
    {
        let mut buffer = [0; 512];
        match reader.read(&mut buffer) {
            Ok(length) => second_output.extend_from_slice(&buffer[..length]),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) => {}
            Err(error) => panic!("reading the shared palette probe: {error}"),
        }
    }
    assert!(
        second_output
            .windows(b"PALETTE:00ccbbaa,65".len())
            .any(|bytes| bytes == b"PALETTE:00ccbbaa,65"),
        "the child did not observe the latest shared viewer's palette: {:?}",
        String::from_utf8_lossy(&second_output)
    );

    drop(second);
    drop(holder);
    let _ = second_process.kill();
    let _ = second_process.wait();
    let _ = client.kill(session_id);
    let _ = client.shutdown();
}

#[test]
fn cli_upgrade_keeps_a_live_windows_pseudoconsole() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let old_process_id = client.process_id();
    let pane = client
        .spawn(spawn_request())
        .expect("creating a pseudoconsole pane");
    let session_id = pane.session_id;
    let pane_id = pane.pane_id;
    drop(pane);

    let upgrade = Command::new(daemon_binary())
        .arg("--upgrade")
        .env("APPDATA", &daemon.config)
        .output()
        .expect("running zmux --upgrade");
    assert!(
        upgrade.status.success(),
        "zmux --upgrade failed: {}",
        String::from_utf8_lossy(&upgrade.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&upgrade.stdout).trim(),
        "Replaced the multiplexer; its sessions were kept."
    );

    let replacement = daemon.client();
    assert_ne!(replacement.process_id(), old_process_id);
    replacement
        .resize(session_id, pane_id, 100, 30)
        .expect("resizing the adopted pseudoconsole");
    let attached = replacement
        .attach(session_id, Some(pane_id), None)
        .expect("attaching the adopted pseudoconsole");
    match attached {
        AttachOutcome::Attached { pane, .. } => {
            assert_eq!(pane.session_id, session_id);
            assert_eq!(pane.pane_id, pane_id);
        }
        _ => panic!("expected an exclusive pane after upgrade"),
    }

    replacement
        .kill(session_id)
        .expect("cleaning up the upgraded pseudoconsole");
    replacement
        .shutdown()
        .expect("stopping the replacement daemon");
}

#[test]
fn attached_client_receives_conpty_output_without_host_competing_for_it() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(output_request())
        .expect("creating a pseudoconsole pane");
    let session_id = pane.session_id;

    let mut output = std::fs::File::from(pane.conout);
    let mut input = std::fs::File::from(pane.conin);
    let mut sent_device_attributes = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut bytes = Vec::new();
    while Instant::now() < deadline {
        let mut available = 0;
        let peek_succeeded = unsafe {
            windows::Win32::System::Pipes::PeekNamedPipe(
                windows::Win32::Foundation::HANDLE(output.as_raw_handle()),
                None,
                0,
                None,
                Some(&mut available),
                None,
            )
            .is_ok()
        };
        if !peek_succeeded {
            break;
        }
        if available > 0 {
            let start = bytes.len();
            bytes.resize(start + available as usize, 0);
            match output.read(&mut bytes[start..]) {
                Ok(length) => bytes.truncate(start + length),
                Err(_) => bytes.truncate(start),
            }
            if bytes
                .windows(b"zetta-upgrade".len())
                .any(|window| window == b"zetta-upgrade")
            {
                break;
            }
            if !sent_device_attributes && bytes.windows(3).any(|window| window == b"\x1b[c") {
                input
                    .write_all(b"\x1b[?6c")
                    .expect("answering the pseudoconsole device query");
                sent_device_attributes = true;
            }
        }

        std::thread::sleep(Duration::from_millis(10));
    }
    drop(output);
    drop(input);

    let _ = client.kill(session_id);
    let _ = client.shutdown();

    assert!(!bytes.is_empty(), "the attached client received no output");
    assert!(
        String::from_utf8_lossy(&bytes).contains("zetta-upgrade"),
        "unexpected pseudoconsole output: {:?}",
        String::from_utf8_lossy(&bytes)
    );
}

#[test]
fn detached_conpty_reader_stops_before_exclusive_reattach() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(detach_restore_request())
        .expect("creating a pseudoconsole pane");
    let session_id = pane.session_id;
    let pane_id = pane.pane_id;
    let mut output = std::fs::File::from(pane.conout);
    let mut input = std::fs::File::from(pane.conin);

    let initial = read_until(&mut output, &mut input, b"before", Duration::from_secs(2));
    assert!(
        String::from_utf8_lossy(&initial).contains("before"),
        "the attached client received no initial output: {:?}",
        String::from_utf8_lossy(&initial)
    );
    drop(output);
    drop(input);

    client
        .detach_as(
            session_id,
            BackgroundSessionSummary {
                id: session_id,
                title: "test".to_owned(),
                authentication_required: false,
                active_pane: pane_id,
                layout: BackgroundPaneLayout::Pane { pane_id },
                panes: Vec::new(),
                held: false,
                scoped_to: Some(std::process::id()),
            },
            serde_json::Value::Null,
            None,
            std::process::id(),
        )
        .expect("detaching the pane");
    // Let the daemon enter its detached read before handing the console back;
    // otherwise the test could pass without exercising the reader that must
    // be stopped at reattach.
    std::thread::sleep(Duration::from_millis(250));

    let attached = client
        .attach(session_id, Some(pane_id), None)
        .expect("reattaching the pane");
    let AttachOutcome::Attached { pane, .. } = attached else {
        panic!("expected an exclusive pane after detaching");
    };
    let mut output = std::fs::File::from(pane.conout);
    let mut input = std::fs::File::from(pane.conin);
    let after = read_until(&mut output, &mut input, b"after", Duration::from_secs(5));
    drop(output);
    drop(input);

    assert!(
        String::from_utf8_lossy(&after).contains("after"),
        "output produced after exclusive reattach was consumed by the daemon: {:?}",
        String::from_utf8_lossy(&after)
    );

    let _ = client.kill(session_id);
    let _ = client.shutdown();
}

#[test]
fn stopping_an_idle_daemon_stops_its_pseudoconsole_host() {
    let daemon = TestDaemon::start();
    let host_endpoint =
        zmux::transport::Endpoint::read(&zmux::pty_host::endpoint_path(&daemon.sessions_dir()))
            .expect("reading the pseudoconsole host endpoint");
    let host_process_id = host_endpoint.process_id;
    assert!(
        process_is_alive(host_process_id),
        "the pseudoconsole host must be alive before stopping the daemon"
    );

    assert_eq!(
        zmux::stop(&daemon.sessions_dir(), false).expect("stopping the idle daemon"),
        zmux::StopOutcome::Stopped
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    while process_is_alive(host_process_id) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !process_is_alive(host_process_id),
        "stopping zmux must not leave zmux-pty process {host_process_id} running"
    );
    assert!(
        !zmux::pty_host::endpoint_path(&daemon.sessions_dir()).exists(),
        "stopping zmux must remove the stale zmux-pty endpoint"
    );
}

#[test]
fn force_stopping_a_daemon_with_a_session_stops_its_pseudoconsole_host() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request())
        .expect("creating a pseudoconsole pane");
    drop(pane);

    let host_endpoint =
        zmux::transport::Endpoint::read(&zmux::pty_host::endpoint_path(&daemon.sessions_dir()))
            .expect("reading the pseudoconsole host endpoint");
    let host_process_id = host_endpoint.process_id;
    let daemon_process_id = client.process_id();
    assert!(process_is_alive(host_process_id));

    assert_eq!(
        zmux::stop(&daemon.sessions_dir(), true).expect("force-stopping the daemon"),
        zmux::StopOutcome::Signalled {
            process_id: daemon_process_id
        }
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    while process_is_alive(host_process_id) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !process_is_alive(host_process_id),
        "force-stopping zmux must not leave zmux-pty process {host_process_id} running"
    );
}
