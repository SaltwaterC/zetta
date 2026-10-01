use super::*;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::time::Instant;

/// The child, and the PTY master that has to outlive it: closing the master
/// hangs the child up, and a hangup is not the exit being tested.
fn spawn_exiting_with(
    code: u32,
) -> (
    Box<dyn Child + Send + Sync>,
    Box<dyn portable_pty::MasterPty + Send>,
) {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    #[cfg(unix)]
    let command = {
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", &format!("sleep 0.2; exit {code}")]);
        command
    };
    #[cfg(windows)]
    let command = {
        let mut command = CommandBuilder::new("cmd.exe");
        command.args([
            "/d",
            "/c",
            &format!("ping -n 1 127.0.0.1 >nul & exit {code}"),
        ]);
        command
    };
    (pair.slave.spawn_command(command).unwrap(), pair.master)
}

#[test]
fn the_watch_wakes_the_loop_and_leaves_the_exit_status_to_its_owner() {
    let (mut child, _master) = spawn_exiting_with(3);
    let watch = ChildExitWatch::start(child.as_ref(), thread::current())
        .expect("this platform can watch a PTY child");
    assert!(!watch.exited(), "the child has not exited yet");

    let started = Instant::now();
    while !watch.exited() {
        thread::park_timeout(Duration::from_secs(1));
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the watch never reported the exit"
        );
    }
    // Observing the exit must not have reaped it: the owner still collects
    // the real status.
    let status = child
        .try_wait()
        .expect("the child is still the owner's to reap")
        .expect("an exited child has a status");
    assert_eq!(status.exit_code(), 3);
}
