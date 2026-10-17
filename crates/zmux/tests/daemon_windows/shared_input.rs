//! Shared input delivery and idle CPU coverage with a gated console reader.
//!
//! The child uses this test executable so shell startup and line-editing echo
//! cannot dominate the paste deadline when crate tests run in parallel.

use super::*;

const LINES: usize = 4096;
const HELD_INPUT_ENV: &str = "ZMUX_TEST_HELD_INPUT";

fn wait_for_file(path: &Path, message: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !path.exists() {
        assert!(Instant::now() < deadline, "{message}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn held_input_reader_subprocess() {
    use windows::Win32::System::Console::{
        CONSOLE_MODE, ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, GetConsoleMode, GetStdHandle,
        STD_INPUT_HANDLE, SetConsoleMode,
    };

    let Some(directory) = std::env::var_os(HELD_INPUT_ENV).map(PathBuf::from) else {
        return;
    };
    // SAFETY: the handle belongs to this child process's console, and the mode
    // points to initialized, writable storage. This child owns its console mode.
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) }.expect("opening console input");
    let mut mode = CONSOLE_MODE(0);
    unsafe { GetConsoleMode(input, &mut mode) }.expect("reading console input mode");
    // Echoing 256 KiB makes delivery depend on ConPTY rendering throughput.
    unsafe { SetConsoleMode(input, (mode | ENABLE_LINE_INPUT) & !ENABLE_ECHO_INPUT) }
        .expect("disabling console input echo");
    std::fs::write(directory.join("input-ready"), b"ready").unwrap();
    wait_for_file(
        &directory.join("release-input"),
        "the parent never released input",
    );

    let expected = "x".repeat(63);
    let mut line = String::new();
    for index in 0..LINES {
        line.clear();
        std::io::stdin()
            .read_line(&mut line)
            .expect("reading held input");
        assert_eq!(
            line.trim_end_matches(['\r', '\n']),
            expected,
            "line {index}"
        );
    }
    println!("got-lines:{LINES}");
    // Keep the pane alive until its parent has observed the delivery marker.
    std::thread::sleep(Duration::from_secs(30));
}

/// CPU time a process has used, user and kernel.
fn process_cpu_time(process: &Child) -> Duration {
    use windows::Win32::Foundation::{FILETIME, HANDLE};
    use windows::Win32::System::Threading::GetProcessTimes;

    let mut times = [FILETIME::default(); 4];
    let [created, exited, kernel, user] = &mut times;
    unsafe {
        GetProcessTimes(
            HANDLE(process.as_raw_handle()),
            created,
            exited,
            kernel,
            user,
        )
    }
    .expect("reading the daemon's CPU time");
    let hundreds_of_nanoseconds =
        |time: &FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    Duration::from_nanos((hundreds_of_nanoseconds(kernel) + hundreds_of_nanoseconds(user)) * 100)
}

/// A paste into a program that is not reading yet arrives in full once it reads,
/// without the daemon burning CPU in the meantime.
///
/// This does not reproduce the Unix spin: the console host kept taking input
/// from its pipe while the program waited, so the drain's write never came back
/// empty, and the test passed against a drain that retried without waiting. It
/// guards delivery and the CPU ceiling, not the poller wakeup itself.
#[test]
fn shared_input_the_child_does_not_read_leaves_the_drain_idle() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let gate = daemon.config.join("release-input");
    let ready = daemon.config.join("input-ready");
    let mut env = HashMap::new();
    env.insert(
        HELD_INPUT_ENV.to_owned(),
        daemon.config.to_string_lossy().into_owned(),
    );
    let created = client
        .create_shared(CreateSharedRequest {
            operation_id: client.next_shared_operation_id(),
            title: "held input".to_owned(),
            replacement: SharedDraftLayout::Draft { draft_id: 1 },
            panes: vec![SharedPaneDraft {
                draft_id: 1,
                profile: "held input reader".to_owned(),
                command: Some(zetta_profiles::ProfileCommand::with_args(
                    std::env::current_exe()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    vec![
                        "--exact".to_owned(),
                        "shared_input::held_input_reader_subprocess".to_owned(),
                        "--nocapture".to_owned(),
                    ],
                )),
                env,
                working_directory: None,
                inherit_working_directory_from: None,
                load_shell_integration: false,
                size: spawn_request().size,
                console_palette: ConsolePalette::default(),
                metadata: BackgroundPaneSummary {
                    id: 0,
                    label: "pane".to_owned(),
                    profile: "held input reader".to_owned(),
                    configured_command: String::new(),
                    application: "held input reader".to_owned(),
                    foreground_command: None,
                    terminal_title: None,
                    working_directory: None,
                    state: BackgroundPaneState::Starting,
                    exit: None,
                },
            }],
            active_pane: Some(SharedPaneRef::Draft { draft_id: 1 }),
            verifier: None,
        })
        .expect("creating a shared pseudoconsole session");
    let session_id = created.session_id;
    let pane_id = created.state.operation_receipts[0].draft_mappings[0].pane_id;
    let AttachOutcome::SharedAttached { pane, .. } = client
        .attach_shared_with_secret(session_id, pane_id, None)
        .expect("attaching to the shared pane")
    else {
        panic!("a shared session's pane must attach shared")
    };
    let mut reader = pane.reader();
    wait_for_file(&ready, "the console reader never became ready");

    let line = format!("{}\r", "x".repeat(63));
    let payload = line.repeat(LINES);
    for chunk in payload.as_bytes().chunks(16 * 1024) {
        pane.send_input(chunk).expect("queueing shared input");
    }
    // Let the daemon finish queueing the paste before measuring its idle CPU.
    std::thread::sleep(Duration::from_millis(200));
    let before = process_cpu_time(&daemon.process);
    std::thread::sleep(Duration::from_secs(1));
    let used = process_cpu_time(&daemon.process) - before;
    assert!(
        used < Duration::from_millis(250),
        "the daemon used {used:?} of CPU in a second holding input the pane would not take"
    );

    // The child cannot start reading until after the CPU sample, even when
    // parallel crate tests delay startup or scheduling.
    let released = Instant::now();
    std::fs::write(&gate, b"read").expect("releasing the console reader");
    let deadline = Instant::now() + Duration::from_secs(30);
    let marker = format!("got-lines:{LINES}");
    let mut output = Vec::new();
    while Instant::now() < deadline
        && !output
            .windows(marker.len())
            .any(|bytes| bytes == marker.as_bytes())
    {
        pane.release_size_hold();
        let mut buffer = [0; 4096];
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(length) => output.extend_from_slice(&buffer[..length]),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) => {}
            Err(error) => panic!("reading the shared pane: {error}"),
        }
    }
    assert!(
        output
            .windows(marker.len())
            .any(|bytes| bytes == marker.as_bytes()),
        "the held paste never arrived in full: {:?}",
        String::from_utf8_lossy(&output[output.len().saturating_sub(512)..])
    );
    eprintln!(
        "held paste delivered {:?} after release",
        released.elapsed()
    );

    drop(reader);
    drop(pane);
    let _ = client.kill(session_id);
    let _ = client.shutdown();
}
