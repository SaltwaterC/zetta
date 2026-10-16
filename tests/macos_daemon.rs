//! The packaged daemon's privacy identity across a real exec handover.
//!
//! This boundary needs the actual application and helper executables, not a
//! unit-test stand-in. It creates and signs a temporary bundle, uses only an
//! isolated catalog, and performs no network operations or permission changes.
//!
//! cargo build --bin zetta --bin zmux
//! cargo test --test macos_daemon -- --ignored

#![cfg(all(target_os = "macos", feature = "zmux"))]

use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};

#[test]
#[ignore = "drives separately built signed application and daemon binaries"]
fn bundled_daemon_owns_its_children_before_and_after_upgrade() {
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    let base = directory.path();
    let macos = base.join("Zetta.app/Contents/MacOS");
    fs::create_dir_all(&macos).unwrap();
    for (source, name) in [
        (env!("CARGO_BIN_EXE_zetta"), "zetta"),
        (env!("CARGO_BIN_EXE_zmux"), "zmux"),
    ] {
        fs::copy(source, macos.join(name)).unwrap();
    }
    let contents = macos.parent().unwrap();
    fs::write(
        contents.join("Info.plist"),
        r#"<?xml version="1.0"?><plist version="1.0"><dict>
        <key>CFBundleIdentifier</key><string>com.zetta.DaemonRegression</string>
        <key>CFBundleExecutable</key><string>zetta</string>
        <key>CFBundlePackageType</key><string>APPL</string>
        </dict></plist>"#,
    )
    .unwrap();
    assert!(
        Command::new("/usr/bin/codesign")
            .args(["--force", "--deep", "--sign", "-"])
            .arg(contents.parent().unwrap())
            .status()
            .unwrap()
            .success()
    );
    let config = base.join("config/zetta");
    fs::create_dir_all(&config).unwrap();
    fs::write(
        config.join("config.json"),
        serde_json::to_vec(&serde_json::json!({
            "profiles": [{
                "name": "Responsibility probe",
                "program": std::env::current_exe().unwrap(),
                "args": ["--ignored", "--exact", "report_responsibility"]
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    let daemon = OwnedDaemon(
        Command::new(macos.join("zmux"))
            .arg("--daemon")
            .env("XDG_CONFIG_HOME", base.join("config"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let endpoint = config.join("sessions/zmux.json");
    wait_for(|| endpoint.is_file().then_some(()));
    for (name, upgrade) in [("before", false), ("after", true)] {
        if upgrade {
            let output = command(&macos, base).arg("--upgrade").output().unwrap();
            successful(&output);
        }
        let report = base.join(name);
        let mut child = command(&macos, base)
            .args([
                "create",
                "--profile",
                "Responsibility probe",
                "--secret-stdin",
                "--env",
            ])
            .arg(format!("ZETTA_RESPONSIBILITY_REPORT={}", report.display()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"\n").unwrap();
        successful(&child.wait_with_output().unwrap());
        let (_, responsible): (u32, i32) =
            wait_for(|| serde_json::from_slice(&fs::read(&report).ok()?).ok());
        assert_eq!(responsible, daemon.0.id() as i32, "{name} upgrade");
    }
}

fn command(macos: &Path, base: &Path) -> Command {
    let mut command = Command::new(macos.join("zmux"));
    command.env("XDG_CONFIG_HOME", base.join("config"));
    command
}

fn successful(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn wait_for<T>(mut ready: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(value) = ready() {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the diagnostic daemon"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct OwnedDaemon(Child);

impl Drop for OwnedDaemon {
    fn drop(&mut self) {
        // Only the isolated daemon started by this test, never the user's.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "child entry point for bundled_daemon_owns_its_children_before_and_after_upgrade"]
fn report_responsibility() {
    let Some(report) = std::env::var_os("ZETTA_RESPONSIBILITY_REPORT").map(PathBuf::from) else {
        return;
    };
    // SAFETY: the terminated symbol names libSystem's PID query, and the
    // checked pointer is converted to its exported signature.
    let symbol = unsafe {
        libc::dlsym(
            libc::RTLD_DEFAULT,
            c"responsibility_get_pid_responsible_for_pid".as_ptr(),
        )
    };
    assert!(!symbol.is_null());
    let responsible: unsafe extern "C" fn(libc::pid_t) -> libc::pid_t =
        unsafe { std::mem::transmute(symbol) };
    let pid = std::process::id();
    // SAFETY: querying this process's own PID is permitted.
    let owner = unsafe { responsible(pid as libc::pid_t) };
    fs::write(report, serde_json::to_vec(&(pid, owner)).unwrap()).unwrap();
}
