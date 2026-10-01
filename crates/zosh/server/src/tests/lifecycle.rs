use super::*;

#[cfg(windows)]
#[test]
fn detached_child_closes_bootstrap_stdout_before_session_ends() {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    const CHILD_MARKER: &str = "ZOSH_TEST_DETACHED_STDOUT_CHILD";
    if std::env::var_os(CHILD_MARKER).is_some() {
        println!("MOSH CONNECT 60000 AAAAAAAAAAAAAAAAAAAAAA");
        std::io::stdout().flush().unwrap();
        let _null = release_bootstrap_stdout().unwrap();
        std::thread::sleep(Duration::from_secs(10));
        return;
    }

    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "lifecycle::tests::detached_child_closes_bootstrap_stdout_before_session_ends",
            "--nocapture",
        ])
        .env(CHILD_MARKER, "1")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut output = String::new();
        stdout.read_to_string(&mut output).unwrap();
        sender.send(output).unwrap();
    });
    let result = receiver.recv_timeout(Duration::from_secs(3));
    let still_running = child.try_wait().unwrap().is_none();
    let _ = child.kill();
    let _ = child.wait();
    reader.join().unwrap();

    let output = result.expect("detached child kept SSH bootstrap stdout open");
    assert!(output.contains("MOSH CONNECT 60000"), "{output}");
    assert!(still_running, "the child ended before the pipe closed");
}

#[cfg(windows)]
#[test]
fn bootstrap_child_does_not_inherit_the_ssh_output_pipe() {
    use std::process::Command;
    use std::sync::mpsc;
    use std::time::Duration;
    use windows::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
    use windows::Win32::System::Console::{GetStdHandle, STD_OUTPUT_HANDLE};

    const CHILD_MARKER: &str = "ZOSH_TEST_BOOTSTRAP_HANDLE_CHILD";
    if std::env::var_os(CHILD_MARKER).is_some() {
        let stdout = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }.unwrap();
        unsafe { SetHandleInformation(stdout, HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT) }
            .unwrap();
        let args = [
            "--exact",
            "lifecycle::tests::bootstrap_handle_helper",
            "--nocapture",
        ]
        .map(OsString::from);
        let (pipe, _child) =
            spawn_detached_process(&std::env::current_exe().unwrap(), &args).unwrap();
        let mut reader = BufReader::new(pipe);
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap() != 0 && !line.contains("MOSH CONNECT 60000") {
            line.clear();
        }
        assert!(line.contains("MOSH CONNECT 60000"), "{line:?}");
        return;
    }

    let (sender, receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "lifecycle::tests::bootstrap_child_does_not_inherit_the_ssh_output_pipe",
                "--nocapture",
            ])
            .env(CHILD_MARKER, "1")
            // This tests handle inheritance in a local launch. Running the
            // suite through SSH must not select the production SSH-job guard.
            .env_remove("SSH_CONNECTION")
            .output()
            .unwrap();
        sender.send(output).unwrap();
    });
    let result = receiver.recv_timeout(Duration::from_secs(2));
    reader.join().unwrap();
    let output = result.expect("detached child kept the SSH output pipe open");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(windows)]
#[test]
fn bootstrap_handle_helper() {
    if std::env::var_os("ZOSH_TEST_BOOTSTRAP_HANDLE_CHILD").is_some() {
        println!("MOSH CONNECT 60000 test");
        std::io::stdout().flush().unwrap();
        std::thread::sleep(std::time::Duration::from_secs(3));
    }
}

#[cfg(windows)]
#[test]
fn child_command_line_quotes_windows_paths_and_quotes() {
    use std::ffi::OsStr;

    let quoted = |word| String::from_utf16(&quote_windows_word(OsStr::new(word))).unwrap();
    assert_eq!(
        quoted(r"C:\Program Files\Zetta\"),
        r#""C:\Program Files\Zetta\\""#
    );
    assert_eq!(quoted(r#"a"b"#), r#""a\"b""#);
}

/// `ssh -tt` starts the bootstrap as the leader of a session whose controlling
/// terminal is a pty, and the leader exits the moment it has forked. That
/// hangs the terminal up, and the kernel signals its foreground process group
/// — which the forked child is still in until it calls setsid. A server lost
/// that race a few times in a hundred, after MOSH CONNECT had gone out, and
/// the pane it was for stayed blank. Run enough times that the race would show.
#[cfg(unix)]
#[test]
fn a_detached_server_survives_its_bootstrap_session_hanging_up() {
    use std::os::unix::process::CommandExt as _;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const CHILD_MARKER: &str = "ZOSH_TEST_DETACH_SURVIVAL_FILE";
    if let Some(path) = std::env::var_os(CHILD_MARKER) {
        // Only the detached process comes back from this; the leader exits.
        detach_after_connect_line().unwrap();
        std::thread::sleep(Duration::from_millis(200));
        std::fs::write(&path, b"survived").unwrap();
        std::process::exit(0);
    }

    let directory = std::env::temp_dir().join(format!("zosh-detach-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let attempts = 40;
    let mut lost = Vec::new();
    for attempt in 0..attempts {
        let (mut master, mut slave) = (0, 0);
        // SAFETY: openpty fills in both descriptors.
        let opened = unsafe {
            libc::openpty(
                &raw mut master,
                &raw mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        assert_eq!(opened, 0, "openpty: {}", std::io::Error::last_os_error());
        // SAFETY: both descriptors were just opened and are owned here.
        let (master, slave) = unsafe {
            use std::os::fd::FromRawFd as _;
            (
                std::fs::File::from_raw_fd(master),
                std::fs::File::from_raw_fd(slave),
            )
        };
        let marker = directory.join(attempt.to_string());
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "lifecycle::tests::a_detached_server_survives_its_bootstrap_session_hanging_up",
                "--test-threads=1",
            ])
            .env(CHILD_MARKER, &marker)
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave));
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut leader = command.spawn().unwrap();
        // Drained, so the leader's writes to its terminal never block.
        let drain = std::thread::spawn(move || {
            let mut master = master;
            let _ = std::io::copy(&mut master, &mut std::io::sink());
        });
        leader.wait().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        if !marker.exists() {
            lost.push(attempt);
        }
        drop(drain);
    }
    let _ = std::fs::remove_dir_all(&directory);
    assert!(
        lost.is_empty(),
        "{} of {attempts} detached servers died with their bootstrap session: attempts {lost:?}",
        lost.len()
    );
}
