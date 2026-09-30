//! Host-side tools for staging the Windows ConPTY runtime. Build scripts run
//! on the host, so Linux/WSL cross-builds must use Linux tools and Linux paths.

use std::{path::Path, process::Command};

pub use host::{download, extract};

pub fn verify_sha256(path: &Path, expected: &str) {
    let output = host::checksum(path);
    let actual = parse_sha256(&output).expect("checksum tool did not return a SHA256 hash");
    assert!(
        actual.eq_ignore_ascii_case(expected),
        "ConPTY package checksum mismatch: expected {expected}, got {actual}"
    );
}

fn parse_sha256(output: &str) -> Option<&str> {
    output
        .split_whitespace()
        .find(|word| word.len() == 64 && word.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn run(command: &mut Command, action: &str) -> String {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("failed to start {command:?} to {action}: {error}"));
    assert!(
        output.status.success(),
        "{command:?} failed to {action}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[cfg(windows)]
mod host {
    use super::*;

    pub fn download(url: &str, archive: &Path) {
        powershell(&format!(
            "Invoke-WebRequest -Uri '{url}' -OutFile '{}'",
            powershell_path(archive)
        ));
    }

    pub fn extract(archive: &Path, extracted: &Path) {
        powershell(&format!(
            "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
            powershell_path(archive),
            powershell_path(extracted)
        ));
    }

    pub fn checksum(path: &Path) -> String {
        run(
            Command::new("certutil")
                .arg("-hashfile")
                .arg(path)
                .arg("SHA256"),
            "verify ConPTY",
        )
    }

    fn powershell(script: &str) {
        run(
            Command::new("powershell").args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!("$ErrorActionPreference = 'Stop'; $ProgressPreference = 'SilentlyContinue'; {script}"),
            ]),
            "stage ConPTY",
        );
    }

    fn powershell_path(path: &Path) -> String {
        path.display().to_string().replace('\'', "''")
    }
}

#[cfg(not(windows))]
mod host {
    use super::*;

    pub fn download(url: &str, archive: &Path) {
        run(
            Command::new("curl")
                .args([
                    "--fail",
                    "--location",
                    "--silent",
                    "--show-error",
                    "--output",
                ])
                .arg(archive)
                .arg(url),
            "download ConPTY (install curl on the build host)",
        );
    }

    pub fn extract(archive: &Path, extracted: &Path) {
        run(
            Command::new("unzip")
                .arg("-q")
                .arg("-o")
                .arg(archive)
                .arg("-d")
                .arg(extracted),
            "extract ConPTY (install unzip on the build host)",
        );
    }

    pub fn checksum(path: &Path) -> String {
        // shasum is supplied by macOS; Linux generally supplies sha256sum.
        let output = Command::new("sha256sum").arg(path).output();
        match output {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => run(
                Command::new("shasum").args(["-a", "256"]).arg(path),
                "verify ConPTY (install sha256sum or shasum on the build host)",
            ),
            result => {
                let output = result.expect("failed to start sha256sum while verifying ConPTY");
                assert!(
                    output.status.success(),
                    "sha256sum failed while verifying ConPTY: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                String::from_utf8_lossy(&output.stdout).into_owned()
            }
        }
    }
}

#[cfg(test)]
#[path = "../src/tests/conpty_tools.rs"]
mod tests;
