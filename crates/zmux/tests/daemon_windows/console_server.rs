//! Daemon-owned console-server panes: the console host makes their output a
//! self-contained stream, answers screen queries from its own grid, and
//! keeps them alive while the daemon is replaced.
use super::*;

/// A pane running a Python fixture on console-server.
fn console_server_request(script: &str) -> SpawnRequest {
    let mut request = spawn_request();
    request.console_backend = zetta_profiles::ConsoleBackend::ConsoleServer;
    request.program = Some("python.exe".to_owned());
    request.args = vec!["-c".to_owned(), script.to_owned()];
    request
}

const PROBE: &str = r#"
import ctypes, sys
from ctypes import wintypes as W
k = ctypes.WinDLL("kernel32", use_last_error=True)
out = k.GetStdHandle(-11)
k.SetConsoleTitleW("zetta-title")
print("one\ntwo")
sys.stdout.write("abc"); sys.stdout.flush()
class CSBI(ctypes.Structure):
    _fields_ = [("size", ctypes.c_short * 2), ("cursor", ctypes.c_short * 2), ("attr", W.WORD),
                ("window", ctypes.c_short * 4), ("max", ctypes.c_short * 2)]
info = CSBI()
k.GetConsoleScreenBufferInfo(out, ctypes.byref(info))
print("\ncursor=%d,%d size=%d,%d" % (info.cursor[0], info.cursor[1], info.size[0], info.size[1]))
print("got=" + repr(input("Q: ")))
"#;

#[test]
fn a_console_server_pane_streams_self_contained_output_and_answers_queries() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(console_server_request(PROBE))
        .expect("creating a console-server pane");
    let mut output = std::fs::File::from(pane.conout);
    let mut input = std::fs::File::from(pane.conin);
    let bytes = read_until(&mut output, &mut input, b"Q: ", Duration::from_secs(15));
    let text = String::from_utf8_lossy(&bytes);
    // LF reaches the daemon as CRLF, as ConPTY's output does.
    assert!(text.contains("one\r\ntwo\r\n"), "{text:?}");
    assert!(text.contains("\u{1b}]0;zetta-title\u{1b}\\"), "{text:?}");
    // The host's grid saw "abc" on the third row before answering.
    assert!(text.contains("cursor=3,2 size=80,24"), "{text:?}");
    input
        .write_all(b"hello\r")
        .expect("typing into the console");
    // Wait for the complete answer in one read. A second unconditional read
    // waits out its deadline when this read already consumed the answer.
    let bytes = read_until(&mut output, &mut input, b"'hello'", Duration::from_secs(10));
    let more = String::from_utf8_lossy(&bytes);
    assert!(
        more.contains("got=") && more.contains("'hello'"),
        "{more:?}"
    );
    client.kill(pane.session_id).expect("cleaning up");
    client.shutdown().expect("stopping the daemon");
}

#[test]
fn a_console_server_pane_survives_a_daemon_upgrade() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(console_server_request(
            "import time\nfor i in range(600):\n    print('tick %d' % i, flush=True)\n    time.sleep(0.1)",
        ))
        .expect("creating a console-server pane");
    let (session_id, pane_id) = (pane.session_id, pane.pane_id);
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

    let replacement = daemon.client();
    replacement
        .resize(session_id, pane_id, 100, 30)
        .expect("resizing the adopted console");
    let attached = replacement
        .attach(session_id, Some(pane_id), None)
        .expect("attaching the adopted console");
    let AttachOutcome::Attached { pane, .. } = attached else {
        panic!("expected an exclusive pane after upgrade");
    };
    let mut output = std::fs::File::from(pane.conout);
    let mut input = std::fs::File::from(pane.conin);
    // Output keeps flowing from the console the host kept: two different
    // ticks prove the child is live, not a replay of what it printed before.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = std::collections::BTreeSet::new();
    while seen.len() < 2 && Instant::now() < deadline {
        let bytes = read_until(&mut output, &mut input, b"\n", Duration::from_secs(2));
        for line in String::from_utf8_lossy(&bytes).lines() {
            if let Some(number) = line.trim().strip_prefix("tick ") {
                seen.insert(number.trim().to_owned());
            }
        }
    }
    assert!(
        seen.len() >= 2,
        "no live output after the upgrade: {seen:?}"
    );
    replacement.kill(session_id).expect("cleaning up");
    replacement
        .shutdown()
        .expect("stopping the replacement daemon");
}

/// Matches the GUI's interactive Windows PowerShell launch, including its
/// startup integration and inherited environment rather than a short probe.
#[test]
fn a_console_server_pane_starts_powershell_with_shell_integration() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let mut request = spawn_request();
    request.console_backend = zetta_profiles::ConsoleBackend::ConsoleServer;
    request.program = Some("powershell.exe".into());
    let integration = include_str!("../../../../src/shell_integration/powershell.ps1");
    request.args = vec![
        "-NoLogo".into(),
        "-NoProfile".into(),
        "-NoExit".into(),
        "-Command".into(),
        format!("{integration}\nWrite-Output 'zetta-powershell-ready'"),
    ];
    request.env.extend(std::env::vars());
    // Deterministic even when the test runner was not started by CMD.
    request.env.insert("=C:".into(), r"C:\Windows".into());
    request.env.insert("=ExitCode".into(), "00000000".into());
    let pane = client
        .spawn(request)
        .expect("starting PowerShell through zmux");
    let mut output = std::fs::File::from(pane.conout);
    let mut input = std::fs::File::from(pane.conin);
    let bytes = read_until(
        &mut output,
        &mut input,
        b"zetta-powershell-ready",
        Duration::from_secs(15),
    );
    assert!(String::from_utf8_lossy(&bytes).contains("zetta-powershell-ready"));
    client
        .kill(pane.session_id)
        .expect("cleaning up PowerShell");
}
