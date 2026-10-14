//! Console lifecycle against real clients: close events, programs started
//! from the console outliving it, generated control events, and the window.
#![cfg(windows)]
use console_server::{ConsoleOutput, Launch, OutputMode, Session};
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

struct Output(Sender<Vec<u8>>);
impl ConsoleOutput for Output {
    fn write(&mut self, bytes: &[u8], _: OutputMode) -> io::Result<()> {
        let _ = self.0.send(bytes.to_vec());
        Ok(())
    }
}
fn spawn(check: &str, marker: Option<&PathBuf>) -> (Session, Receiver<Vec<u8>>) {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/lifecycle.py");
    let mut command = format!("python.exe \"{}\" {check}", script.display());
    if let Some(marker) = marker {
        command.push_str(&format!(" \"{}\"", marker.display()));
    }
    let (sender, receiver) = mpsc::channel();
    let session = Session::spawn(
        Launch {
            command,
            ..Default::default()
        },
        Output(sender),
    )
    .unwrap();
    (session, receiver)
}
fn wait_for(receiver: &Receiver<Vec<u8>>, marker: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut text = String::new();
    while Instant::now() < deadline && !text.contains(marker) {
        if let Ok(bytes) = receiver.recv_timeout(Duration::from_millis(100)) {
            text.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
    assert!(text.contains(marker), "{text:?}");
    text
}
fn marker(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("console-server-{name}-{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

#[test]
fn closing_sends_ctrl_close_to_clients_and_returns_at_once() {
    let marker = marker("close");
    let (mut session, output) = spawn("close", Some(&marker));
    wait_for(&output, "READY");
    let started = Instant::now();
    session.close();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "close must not block"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap_or_default(),
        "closed"
    );
    let _ = std::fs::remove_file(&marker);
}

#[test]
fn programs_started_from_the_console_outlive_it() {
    let marker = marker("detach");
    let (mut session, output) = spawn("detach", Some(&marker));
    wait_for(&output, "READY");
    let pid: u32 = std::fs::read_to_string(&marker)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    session.close();
    drop(session);
    std::thread::sleep(Duration::from_secs(7));
    let alive = std::process::Command::new("tasklist")
        .args(["/fi", &format!("pid eq {pid}"), "/nh"])
        .output()
        .unwrap();
    let listed = String::from_utf8_lossy(&alive.stdout).contains(&pid.to_string());
    let _ = std::process::Command::new("taskkill")
        .args(["/pid", &pid.to_string(), "/f"])
        .output();
    let _ = std::fs::remove_file(&marker);
    assert!(listed, "the detached program died with the console");
}

#[test]
fn generate_console_ctrl_event_reaches_the_console_group() {
    let (mut session, output) = spawn("generate", None);
    let text = wait_for(&output, "GENERATED");
    session.close();
    assert!(text.contains("GENERATED 1 [1]"), "{text:?}");
}

#[test]
fn get_console_window_is_a_real_hidden_window() {
    let (mut session, output) = spawn("window", None);
    let text = wait_for(&output, "WINDOW");
    session.close();
    assert!(text.contains("WINDOW True 1 0"), "{text:?}");
}
