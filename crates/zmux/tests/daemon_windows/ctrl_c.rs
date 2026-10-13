//! Cancellation coverage, including inheriting a caller's disabled Ctrl+C flag.

use super::*;
use std::os::windows::process::CommandExt as _;
use windows::Win32::System::{Console::SetConsoleCtrlHandler, Threading::CREATE_NO_WINDOW};

const IGNORE_CTRL_C_ENV: &str = "ZMUX_TEST_IGNORE_CTRL_C";

#[test]
fn conpty_ctrl_c_cancels_a_command_without_ending_the_shell() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    assert_returns_to_prompt(&client);
    let _ = client.shutdown();
}

#[test]
fn conpty_ctrl_c_works_when_the_caller_ignores_it() {
    // The flag is process-wide. Set it in a subprocess with its own hidden
    // console so this test cannot affect the other tests or their caller.
    let result = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "ctrl_c::ignored_ctrl_c_subprocess",
            "--nocapture",
        ])
        .env(IGNORE_CTRL_C_ENV, "1")
        .creation_flags(CREATE_NO_WINDOW.0)
        .output()
        .expect("starting the isolated Ctrl+C test");
    assert!(
        result.status.success(),
        "inherited Ctrl+C test failed:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn ignored_ctrl_c_subprocess() {
    if std::env::var(IGNORE_CTRL_C_ENV).as_deref() != Ok("1") {
        return;
    }
    unsafe { SetConsoleCtrlHandler(None, true) }.expect("disabling the caller's Ctrl+C handling");
    let daemon = TestDaemon::start();
    assert_returns_to_prompt(&daemon.client());
}

pub(super) fn assert_returns_to_prompt(client: &Client) {
    for program in ["powershell.exe", "pwsh.exe"] {
        if Command::new(program)
            .args(["-NoLogo", "-NoProfile", "-Command", "exit"])
            .output()
            .is_err()
        {
            continue;
        }
        let mut request = output_request();
        request.program = Some(program.to_owned());
        request.args = vec![
            "-NoLogo".to_owned(),
            "-NoProfile".to_owned(),
            "-NoExit".to_owned(),
            "-Command".to_owned(),
            "function global:prompt { 'ZETTA_READY> ' }".to_owned(),
        ];
        let pane = client
            .spawn(request)
            .expect("starting an interactive shell");
        let session_id = pane.session_id;
        let mut output = std::fs::File::from(pane.conout);
        let mut input = std::fs::File::from(pane.conin);
        let ready = read_until(
            &mut output,
            &mut input,
            b"ZETTA_READY>",
            Duration::from_secs(10),
        );
        assert!(ready.windows(12).any(|bytes| bytes == b"ZETTA_READY>"));

        for _ in 0..2 {
            // Split the address so echoed input cannot satisfy the assertion.
            input
                .write_all(include_str!("ctrl_c_command.ps1").trim().as_bytes())
                .unwrap();
            input.write_all(b"\r").unwrap();
            let running = read_until(
                &mut output,
                &mut input,
                b"127.0.0.1",
                Duration::from_secs(10),
            );
            assert!(running.windows(9).any(|bytes| bytes == b"127.0.0.1"));
            // The address comes from ping itself, so Ctrl+C cannot race its
            // startup even when the other test suites are busy.
            input.write_all(b"\x03").unwrap();
            let returned = read_until(
                &mut output,
                &mut input,
                b"ZETTA_READY>",
                Duration::from_secs(5),
            );
            assert!(
                returned.windows(12).any(|bytes| bytes == b"ZETTA_READY>"),
                "{program} did not return to its prompt after Ctrl+C: {:?}",
                String::from_utf8_lossy(&returned)
            );
            assert!(
                process_is_alive(pane.child_pid),
                "the bootstrap exited after Ctrl+C"
            );
        }
        input.write_all(b"exit\r").unwrap();
        drop(output);
        drop(input);
        let _ = client.kill(session_id);
    }
}
