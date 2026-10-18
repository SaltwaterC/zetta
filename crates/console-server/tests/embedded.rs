//! Real Win32 clients running against the reusable in-process Session API.
#![cfg(windows)]
use console_server::{ConsoleOutput, Launch, OutputMode, Session};
use std::io;
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};
#[derive(Debug)]
enum Event {
    Bytes(Vec<u8>),
    Title(String),
    End,
}
struct Output(Sender<Event>);

#[test]
fn powershell_prompts_recover_ctrl_c_without_changing_editing_redraws() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for integration in [
        root.join("../../src/shell_integration/powershell.ps1"),
        root.join("../zetta_profiles/src/shell_integration/powershell_cwd_tracker.ps1"),
    ] {
        assert_shell_ctrl_c_recovery(&integration, false);
        if integration
            .file_name()
            .is_some_and(|name| name == "powershell.ps1")
        {
            assert_shell_ctrl_c_recovery(&integration, true);
        }
    }
}

fn assert_shell_ctrl_c_recovery(integration: &std::path::Path, upgrade: bool) {
    let path = std::env::temp_dir().join(format!("zetta-tail-{}.log", std::process::id()));
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/shell_ctrl_c_recovery.ps1");
    std::fs::write(&path, "TAIL_READY\n").unwrap();
    let (sender, receiver) = mpsc::channel();
    let mut session = Session::spawn(
        Launch {
            command: format!(
                "powershell.exe -NoLogo -NoProfile -NoExit -File \"{}\" \"{}\" {}",
                fixture.display(),
                integration.display(),
                if upgrade { "upgrade" } else { "fresh" },
            ),
            environment: [
                ("TERM".to_owned(), "xterm-256color".to_owned()),
                ("ZETTA_PANE_ID".to_owned(), "ctrl-c-regression".to_owned()),
                ("ZETTA_HOST_EXECUTABLE".to_owned(), String::new()),
            ]
            .into(),
            ..Default::default()
        },
        Output(sender),
    )
    .unwrap();
    let command = format!("& tail.exe -f '{}'\r", path.display());
    // A prompt after actual tail output proves interruption. Echoed command
    // text cannot satisfy either marker.
    let steps: &[(&[u8], &[u8])] = &[
        (b"", b"TAIL_PROMPT>"),
        (command.as_bytes(), b"TAIL_READY"),
        (b"\x03", b"TAIL_PROMPT>"),
        (b"__zetta_leak_mode\r", b"LEAK_READY"),
        (command.as_bytes(), b"TAIL_READY"),
        (b"\x1b[67;46;3;1;8;1_", b"TAIL_PROMPT>"),
    ];
    let mut failure = None;
    for (input, marker) in steps {
        session.input(input).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut bytes = Vec::new();
        while Instant::now() < deadline {
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(Event::Bytes(data)) => bytes.extend_from_slice(&data),
                Ok(Event::End) => break,
                _ => {}
            }
            if bytes.windows(marker.len()).any(|bytes| bytes == *marker) {
                break;
            }
        }
        if input.is_empty() && !bytes.windows(14).any(|bytes| bytes == b"RECOVERY_READY") {
            failure = Some(("startup recovery assertions".to_owned(), bytes));
            break;
        }
        if !bytes.windows(marker.len()).any(|bytes| bytes == *marker) {
            failure = Some((String::from_utf8_lossy(marker).into_owned(), bytes));
            break;
        }
    }
    session.close();
    let _ = std::fs::remove_file(path);
    if let Some((marker, bytes)) = failure {
        panic!(
            "{}: missing {marker}: {}",
            integration.display(),
            String::from_utf8_lossy(&bytes)
        );
    }
}

impl ConsoleOutput for Output {
    fn write(&mut self, bytes: &[u8], _: OutputMode) -> io::Result<()> {
        self.0
            .send(Event::Bytes(bytes.to_vec()))
            .map_err(io::Error::other)
    }
    fn set_title(&mut self, title: &str) -> io::Result<()> {
        self.0
            .send(Event::Title(title.to_owned()))
            .map_err(io::Error::other)
    }
    fn ended(&mut self) {
        let _ = self.0.send(Event::End);
    }
}
#[test]
fn embedded_console_delivers_input_titles_and_final_output() {
    let (sender, receiver) = mpsc::channel();
    let mut session = Session::spawn(
        Launch {
            command: r#"cmd.exe /d /c "title Embedded title & set /p answer=READY & echo FINAL""#
                .to_owned(),
            environment: Default::default(),
            directory: None,
            ..Default::default()
        },
        Output(sender),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut bytes = Vec::new();
    let mut title = None;
    let mut sent = false;
    let mut ended = false;
    while Instant::now() < deadline {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::Bytes(data)) => bytes.extend_from_slice(&data),
            Ok(Event::Title(value)) => title = Some(value),
            Ok(Event::End) => {
                ended = true;
                break;
            }
            Err(_) => {}
        }
        if !sent && bytes.windows(5).any(|bytes| bytes == b"READY") {
            assert_eq!(session.input(b"hello\r").unwrap(), 6);
            sent = true;
        }
    }
    session.close();
    assert!(sent, "{:?}", String::from_utf8_lossy(&bytes));
    assert!(ended, "{:?}", String::from_utf8_lossy(&bytes));
    // cmd prefixes "Administrator: " to the titles it sets when elevated.
    let title = title.unwrap_or_default();
    assert_eq!(
        title.trim_start_matches("Administrator:  "),
        "Embedded title "
    );
    assert!(
        bytes.windows(5).any(|bytes| bytes == b"FINAL"),
        "{:?}",
        String::from_utf8_lossy(&bytes)
    );
}

#[test]
fn native_input_records_preserve_resize_unicode_modifiers_and_key_up() {
    let (sender, receiver) = mpsc::channel();
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/interactive_input.py");
    let mut session = Session::spawn(
        Launch {
            command: format!("python.exe \"{}\"", script.display()),
            environment: Default::default(),
            directory: None,
            ..Default::default()
        },
        Output(sender),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut bytes = Vec::new();
    let mut sent = false;
    while Instant::now() < deadline {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::Bytes(data)) => bytes.extend_from_slice(&data),
            Ok(Event::End) => break,
            _ => {}
        }
        if !sent && bytes.windows(13).any(|bytes| bytes == b"RECORDS_READY") {
            session.resize(90, 35);
            let keys = b"\x1b[65;30;955;1;16;3_\x1b[65;30;955;0;16;3_";
            assert_eq!(session.input(keys).unwrap(), keys.len());
            sent = true;
        }
    }
    session.close();
    assert!(
        sent && bytes.windows(10).any(|bytes| bytes == b"RECORDS_OK"),
        "{:?}",
        String::from_utf8_lossy(&bytes)
    );
}
#[test]
fn a_raw_read_ends_at_a_leading_control_z_whatever_the_input_mode() {
    let (sender, receiver) = mpsc::channel();
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/raw_read_control_z.py");
    let mut session = Session::spawn(
        Launch {
            command: format!("python.exe \"{}\"", script.display()),
            environment: Default::default(),
            directory: None,
            ..Default::default()
        },
        Output(sender),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut bytes = Vec::new();
    let mut sent = false;
    while Instant::now() < deadline {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::Bytes(data)) => bytes.extend_from_slice(&data),
            Ok(Event::End) => break,
            _ => {}
        }
        if !sent && bytes.windows(15).any(|bytes| bytes == b"CONTROL_Z_READY") {
            assert_eq!(session.input(b"\x1a").unwrap(), 1);
            sent = true;
        }
    }
    session.close();
    assert!(
        sent && bytes.windows(16).any(|bytes| bytes == b"CONTROL_Z_READ 0"),
        "{:?}",
        String::from_utf8_lossy(&bytes)
    );
}
#[test]
fn close_cancels_a_parked_read_without_waiting_for_keyboard_input() {
    let (sender, receiver) = mpsc::channel();
    let mut session = Session::spawn(
        Launch {
            command: r#"cmd.exe /d /c "set /p answer=READY""#.into(),
            environment: Default::default(),
            directory: None,
            ..Default::default()
        },
        Output(sender),
    )
    .unwrap();
    let mut bytes = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && !bytes.windows(5).any(|bytes| bytes == b"READY") {
        if let Ok(Event::Bytes(data)) = receiver.recv_timeout(Duration::from_millis(100)) {
            bytes.extend_from_slice(&data);
        }
    }
    let started = Instant::now();
    session.close();
    assert!(started.elapsed() < Duration::from_secs(2));
    session.close();
    assert!(bytes.windows(5).any(|bytes| bytes == b"READY"));
}

#[cfg(feature = "harness")]
#[test]
fn encoded_ctrl_c_reaches_the_embedded_child_handler() {
    let (sender, receiver) = mpsc::channel();
    let mut session = Session::spawn(
        Launch {
            command: format!(
                "\"{}\" --control-child",
                env!("CARGO_BIN_EXE_console-server")
            ),
            environment: Default::default(),
            directory: None,
            ..Default::default()
        },
        Output(sender),
    )
    .unwrap();
    let mut bytes = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut sent = false;
    while Instant::now() < deadline {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::Bytes(data)) => bytes.extend_from_slice(&data),
            Ok(Event::End) => break,
            _ => {}
        }
        if !sent && bytes.windows(16).any(|bytes| bytes == b"ZETTA_CTRL_READY") {
            let key = b"\x1b[67;46;3;1;8;1_";
            assert_eq!(session.input(key).unwrap(), key.len());
            sent = true;
        }
    }
    session.close();
    assert!(
        sent && bytes
            .windows(18)
            .any(|bytes| bytes == b"ZETTA_CTRL_HANDLED"),
        "{:?}",
        String::from_utf8_lossy(&bytes)
    );
}
