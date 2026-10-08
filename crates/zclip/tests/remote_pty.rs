//! SSH-style helper exchange with a separate controlling terminal.

#![cfg(unix)]

use std::{
    io::{Read as _, Write as _},
    os::{fd::FromRawFd as _, unix::process::CommandExt as _},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
};
use zclip::{
    host::Host,
    protocol::{Message, Scanner},
};

fn controlling_pty() -> (std::fs::File, std::fs::File) {
    let mut master = -1;
    let mut slave = -1;
    // SAFETY: both pointers refer to writable file-descriptor slots.
    assert_eq!(
        unsafe {
            libc::openpty(
                &raw mut master,
                &raw mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        },
        0
    );
    // SAFETY: openpty returned two owned file descriptors.
    unsafe {
        (
            std::fs::File::from_raw_fd(master),
            std::fs::File::from_raw_fd(slave),
        )
    }
}

/// Starts a helper whose controlling terminal is the returned master.
fn spawn_helper(name: &str, input: &[u8]) -> (std::process::Child, std::fs::File) {
    let (master, slave) = controlling_pty();
    let slave_fd = std::os::fd::AsRawFd::as_raw_fd(&slave);
    let executable = match name {
        "zcopy" => env!("CARGO_BIN_EXE_zcopy"),
        "zpaste" => env!("CARGO_BIN_EXE_zpaste"),
        _ => unreachable!(),
    };
    let mut command = Command::new(executable);
    command
        .env("SSH_CONNECTION", "test")
        .env_remove("ZCLIP_HOST_BACKEND")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // SAFETY: setsid and ioctl are async-signal-safe and run before exec.
    unsafe {
        command.pre_exec(move || {
            // Darwin exposes this constant as u32, but ioctl takes c_ulong.
            if libc::setsid() == -1 || libc::ioctl(slave_fd, libc::TIOCSCTTY as _, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    drop(slave);
    let input = input.to_vec();
    let mut child_stdin = child.stdin.take().unwrap();
    thread::spawn(move || {
        child_stdin.write_all(&input).unwrap();
    });
    (child, master)
}

fn run_helper(
    name: &str,
    input: &[u8],
    clipboard: &str,
    allowed: bool,
) -> (std::process::Output, String) {
    let (child, mut master) = spawn_helper(name, input);
    let copied = Arc::new(Mutex::new(String::new()));
    let copied_by_host = Arc::clone(&copied);
    let clipboard = clipboard.to_owned();
    let host_thread = thread::spawn(move || {
        let mut scanner = Scanner::default();
        let mut host = Host::default();
        let mut buffer = [0; 8192];
        loop {
            let read = master.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            let mut frames = Vec::new();
            scanner.filter(&buffer[..read], |frame| frames.push(frame));
            for frame in frames {
                let response = host
                    .handle(
                        frame,
                        allowed,
                        |text| {
                            *copied_by_host.lock().unwrap() = text.to_owned();
                            Ok(())
                        },
                        || Ok(Some(clipboard.clone())),
                    )
                    .unwrap();
                let done = matches!(response.message, Message::Done | Message::Error(_));
                master.write_all(&response.encode()).unwrap();
                if done {
                    return;
                }
            }
        }
    });
    let output = child.wait_with_output().unwrap();
    host_thread.join().unwrap();
    let copied = copied.lock().unwrap().clone();
    (output, copied)
}

#[test]
fn piped_copy_and_paste_use_the_displaying_clipboard() {
    let text = "héllo 🍋".repeat(10_000);
    let (copy, copied) = run_helper("zcopy", text.as_bytes(), "", false);
    assert!(
        copy.status.success(),
        "{}",
        String::from_utf8_lossy(&copy.stderr)
    );
    assert_eq!(copied, text);
    assert!(copy.stdout.is_empty());

    let (paste, _) = run_helper("zpaste", b"", &text, true);
    assert!(
        paste.status.success(),
        "{}",
        String::from_utf8_lossy(&paste.stderr)
    );
    assert_eq!(paste.stdout, text.as_bytes());
}

#[test]
fn disabled_remote_paste_is_an_error_with_no_stdout() {
    let (output, _) = run_helper("zpaste", b"", "private", false);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("disabled"));
}

#[test]
fn empty_clipboards_and_empty_copy_are_successful() {
    let (copy, copied) = run_helper("zcopy", b"", "old", false);
    assert!(copy.status.success());
    assert!(copied.is_empty());
    let (paste, _) = run_helper("zpaste", b"", "", true);
    assert!(paste.status.success());
    assert!(paste.stdout.is_empty());
}

/// A helper that abandons a transfer tells the host, which then drops the
/// transfer's data instead of holding it until the transfer times out.
#[test]
fn an_abandoned_copy_cancels_its_host_transfer() {
    let (child, mut master) = spawn_helper("zcopy", b"abandoned");
    let host_thread = thread::spawn(move || {
        let mut scanner = Scanner::default();
        let mut host = Host::default();
        let mut buffer = [0; 8192];
        loop {
            let read = master.read(&mut buffer).unwrap();
            assert_ne!(read, 0, "the helper left without cancelling");
            let mut frames = Vec::new();
            scanner.filter(&buffer[..read], |frame| frames.push(frame));
            for frame in frames {
                let cancelled = matches!(frame.message, Message::Error(_));
                let data = matches!(frame.message, Message::Data { .. });
                let id = frame.id;
                let response = host.handle(frame, false, |_| Ok(()), || Ok(None));
                if cancelled {
                    assert!(response.is_none());
                    return host.handle(
                        zclip::protocol::Frame {
                            id,
                            message: Message::End,
                        },
                        false,
                        |_| panic!("a cancelled copy reached the clipboard"),
                        || Ok(None),
                    );
                }
                let mut response = response.unwrap();
                if data {
                    // An acknowledgement the helper cannot accept, so it gives up.
                    response.message = Message::Ack { next_sequence: 99 };
                }
                master.write_all(&response.encode()).unwrap();
            }
        }
    });
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    let after_cancel = host_thread.join().unwrap().unwrap();
    assert!(matches!(after_cancel.message, Message::Error(_)));
}
