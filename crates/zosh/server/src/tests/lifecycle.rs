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
