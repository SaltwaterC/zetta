//! Builds the Linux relay that `wslx.exe` carries into a WSL distribution.
//!
//! Only a Windows build embeds the relay, so every other target returns at
//! once. For Windows, the `wslx-relay` binary of this same crate is built for
//! each architecture WSL runs on — `x86_64` and `aarch64`, because an x64
//! `wslx.exe` runs emulated on Arm64 Windows, whose distributions are Arm64 —
//! and the paths are handed to `launcher.rs` for `include_bytes!`.
//!
//! The relay is a static musl binary linked by the `rust-lld` that ships with
//! rustc, so the only thing a Windows build needs beyond the pinned toolchain
//! is the two musl standard libraries, which `rust-toolchain.toml` lists. The
//! nested build gets its own target directory, because the outer build holds
//! the lock on its own, and an environment with Cargo's per-build variables
//! removed, because those describe the outer Windows build rather than this
//! one.

use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

/// The environment-variable stem `launcher.rs` reads, and the Rust target.
const RELAY_TARGETS: [(&str, &str); 2] = [
    ("X86_64", "x86_64-unknown-linux-musl"),
    ("AARCH64", "aarch64-unknown-linux-musl"),
];

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=Cargo.toml");
    println!("cargo::rerun-if-changed=Cargo.lock");
    println!("cargo::rerun-if-changed=src");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let target_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("relay");
    let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let rustc = env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));
    for (stem, triple) in RELAY_TARGETS {
        require_target(&rustc, triple);
        build_relay(&cargo, &manifest_dir, &target_dir, triple);
        let relay = target_dir.join(triple).join("release").join("wslx-relay");
        println!("cargo::rustc-env=WSLX_RELAY_{stem}={}", relay.display());
    }
}

/// Fails with the command that fixes it when a musl standard library is
/// missing, rather than with a page of "can't find crate for `std`".
fn require_target(rustc: &OsString, triple: &str) {
    let output = Command::new(rustc)
        .args(["--print", "target-libdir", "--target", triple])
        .output()
        .expect("running rustc --print target-libdir");
    let libdir = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let installed = output.status.success()
        && Path::new(&libdir)
            .read_dir()
            .is_ok_and(|mut entries| entries.next().is_some());
    if !installed {
        panic!(
            "wslx.exe embeds a Linux relay built for {triple}, whose standard library is not \
             installed. Run:\n\n    rustup target add x86_64-unknown-linux-musl \
             aarch64-unknown-linux-musl\n"
        );
    }
}

fn build_relay(cargo: &OsString, manifest_dir: &Path, target_dir: &Path, triple: &str) {
    let mut command = Command::new(cargo);
    command
        .current_dir(manifest_dir)
        .args([
            "build",
            "--release",
            "--locked",
            "--offline",
            "--bin",
            "wslx-relay",
        ])
        .arg("--manifest-path")
        .arg(manifest_dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(target_dir)
        .args(["--target", triple])
        .args(["--config", &format!("target.{triple}.linker=\"rust-lld\"")])
        // Small and self-contained matters more than speed for a binary that
        // is copied into every distribution and only ever relays a handful of
        // agent messages.
        .args(["--config", "profile.release.opt-level=\"z\""])
        .args(["--config", "profile.release.lto=true"])
        .args(["--config", "profile.release.codegen-units=1"])
        .args(["--config", "profile.release.panic=\"abort\""])
        .args(["--config", "profile.release.strip=true"])
        .args(["--config", "profile.release.debug=false"])
        .args(["--config", "profile.release.incremental=false"]);
    for (key, _) in env::vars_os() {
        if describes_outer_build(&key.to_string_lossy()) {
            command.env_remove(&key);
        }
    }
    let status = command.status().expect("running the nested relay build");
    assert!(status.success(), "building wslx-relay for {triple} failed");
}

/// The variables Cargo sets for this build script, or that configure the
/// outer build: passed on, they would point the nested build at the Windows
/// target, the outer target directory, or Clippy.
fn describes_outer_build(key: &str) -> bool {
    let keep = matches!(key, "CARGO_HOME" | "CARGO_NET_OFFLINE")
        || key.starts_with("CARGO_REGISTRIES_")
        || key.starts_with("CARGO_HTTP_");
    !keep
        && (key.starts_with("CARGO_")
            || key.starts_with("__CARGO")
            || matches!(
                key,
                "RUSTFLAGS"
                    | "RUSTDOCFLAGS"
                    | "RUSTC_WORKSPACE_WRAPPER"
                    | "RUSTC_LINKER"
                    | "TARGET"
                    | "HOST"
                    | "OUT_DIR"
                    | "OPT_LEVEL"
                    | "PROFILE"
                    | "DEBUG"
                    | "NUM_JOBS"
                    | "MAKEFLAGS"
                    | "MFLAGS"
            ))
}
