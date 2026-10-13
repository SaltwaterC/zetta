//! Regression coverage for cleaning up upgraded test daemons during unwinding.

use super::*;
use std::panic::{AssertUnwindSafe, catch_unwind};
use windows::Win32::{
    Foundation::{CloseHandle, HANDLE, STILL_ACTIVE},
    System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
        TerminateProcess,
    },
};

/// Pins a process created by this test and stops it if the cleanup assertion
/// fails, so the regression itself cannot leave a hook's output pipe open.
struct TestProcess(HANDLE);

impl TestProcess {
    fn open(process_id: u32) -> Self {
        Self(
            unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
                    false,
                    process_id,
                )
            }
            .expect("opening a process created by the cleanup test"),
        )
    }

    fn is_alive(&self) -> bool {
        let mut exit_code = 0;
        unsafe { GetExitCodeProcess(self.0, &mut exit_code) }
            .expect("reading the cleanup test process status");
        exit_code == STILL_ACTIVE.0 as u32
    }
}

impl Drop for TestProcess {
    fn drop(&mut self) {
        unsafe {
            let _ = TerminateProcess(self.0, 1);
            let _ = CloseHandle(self.0);
        }
    }
}

#[test]
fn a_panicking_upgrade_test_stops_its_replacement_and_pseudoconsole() {
    let mut processes = Vec::new();
    let result = catch_unwind(AssertUnwindSafe(|| {
        let daemon = TestDaemon::start();
        let client = daemon.client();
        let pane = client.spawn(spawn_request()).unwrap();
        let host = zmux::transport::Endpoint::read_host(&zmux::pty_host::endpoint_path(
            &daemon.sessions_dir(),
        ))
        .unwrap();
        let upgrade = Command::new(daemon_binary())
            .arg("--upgrade")
            .env("APPDATA", &daemon.config)
            .output()
            .unwrap();
        assert!(
            upgrade.status.success(),
            "{}",
            String::from_utf8_lossy(&upgrade.stderr)
        );
        let replacement_id = daemon.client().process_id();
        assert_ne!(replacement_id, daemon.process.id());
        processes = vec![
            TestProcess::open(replacement_id),
            TestProcess::open(host.process_id),
            TestProcess::open(pane.child_pid),
        ];
        panic!("exercise test-fixture cleanup after a failed upgrade assertion");
    }));
    assert!(result.is_err());
    assert_eq!(
        processes.len(),
        3,
        "the test never reached the upgraded daemon"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while processes.iter().any(TestProcess::is_alive) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !processes.iter().any(TestProcess::is_alive),
        "unwinding left the replacement daemon or its pseudoconsole running"
    );
}
