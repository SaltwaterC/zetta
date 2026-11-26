//! The pinned ConPTY runtime (`conpty.dll` and `OpenConsole.exe`) and where a
//! build script stages it. Shared by the root build script, which puts it
//! beside `zetta.exe`, and `crates/terminal`'s, which puts it beside that
//! crate's test binaries.
//!
//! `alacritty_terminal` loads `conpty.dll` only from the application directory
//! or System32, so a binary without the runtime beside it gets the inbox
//! ConPTY. Tests that rely on OpenConsole's behaviour need it staged next to
//! them.
//!
//! Both build scripts include this file with `#[path]` beside
//! `conpty_tools.rs`, which provides the host-side download, checksum and
//! extract steps.

use std::{
    env,
    path::{Path, PathBuf},
};

use crate::conpty_tools;

const PACKAGE_URL: &str = "https://github.com/microsoft/terminal/releases/download/v1.24.12741.0/Microsoft.Windows.Console.ConPTY.1.24.261001001.nupkg";
const PACKAGE_ID: &str = "1.24.261001001";
const PACKAGE_SHA256: &str = "ECEAAFE3BDCC85E95D18666EB647E0B8E7E00AF45DBCAD7034CDC61C4A29FE74";

/// The build profile's output directory (`target/<triple>/<profile>` or
/// `target/<profile>`), the directory a binary of this build lands in.
pub fn profile_dir() -> PathBuf {
    let out_dir = env::var("OUT_DIR").expect("Cargo did not provide OUT_DIR");
    Path::new(&out_dir)
        .ancestors()
        .nth(3)
        .expect("could not locate the Cargo profile directory")
        .to_path_buf()
}

/// Copies `conpty.dll` and `OpenConsole.exe` into each of `destinations`,
/// downloading and verifying the pinned package first if the cache under
/// `<repository>/target` does not hold it yet. Every build script uses that one
/// cache whatever its own target directory, so the package is fetched once per
/// checkout.
pub fn stage(repository: &Path, destinations: &[PathBuf]) {
    let architecture =
        env::var("CARGO_CFG_TARGET_ARCH").expect("Cargo did not provide target arch");
    let (runtime_arch, console_arch) = match architecture.as_str() {
        "x86_64" => ("win-x64", "x64"),
        "aarch64" => ("win-arm64", "arm64"),
        other => panic!("unsupported Windows architecture for ConPTY: {other}"),
    };

    let cache_dir = repository
        .join("target")
        .join("zetta-conpty")
        .join(PACKAGE_ID)
        .join(&architecture);
    let conpty_source = cache_dir.join("conpty.dll");
    let open_console_source = cache_dir.join("OpenConsole.exe");

    if !conpty_source.is_file() || !open_console_source.is_file() {
        let out_dir = PathBuf::from(env::var("OUT_DIR").expect("Cargo did not provide OUT_DIR"));
        std::fs::create_dir_all(&cache_dir).expect("failed to create the ConPTY cache");
        let archive = out_dir.join("conpty.nupkg.zip");
        let extracted = out_dir.join("conpty");
        conpty_tools::download(PACKAGE_URL, &archive);
        conpty_tools::verify_sha256(&archive, PACKAGE_SHA256);
        conpty_tools::extract(&archive, &extracted);

        fill_cache(
            &extracted
                .join("runtimes")
                .join(runtime_arch)
                .join("native")
                .join("conpty.dll"),
            &conpty_source,
        );
        fill_cache(
            &extracted
                .join("build")
                .join("native")
                .join("runtimes")
                .join(console_arch)
                .join("OpenConsole.exe"),
            &open_console_source,
        );
    }

    for destination in destinations {
        std::fs::create_dir_all(destination)
            .unwrap_or_else(|error| panic!("failed to create {}: {error}", destination.display()));
        copy_runtime(&conpty_source, &destination.join("conpty.dll"));
        copy_runtime(&open_console_source, &destination.join("OpenConsole.exe"));
    }
}

/// Puts `source` into the cache under its final name in one step, so a build
/// script staging from the cache while another fills it never copies a
/// half-written file. Two build scripts can fill it at once: the root's and,
/// through a path dependency, `crates/terminal`'s.
fn fill_cache(source: &Path, target: &Path) {
    let partial = target.with_extension(format!("{}.partial", std::process::id()));
    copy_runtime(source, &partial);
    if let Err(error) = std::fs::rename(&partial, target) {
        let _ = std::fs::remove_file(&partial);
        // Another build script finished the same file first.
        assert!(
            target.is_file(),
            "failed to move {} into the ConPTY cache: {error}",
            target.display()
        );
    }
}

fn copy_runtime(source: &Path, target: &Path) {
    std::fs::copy(source, target).unwrap_or_else(|error| {
        panic!(
            "failed to stage {} as {}: {error}",
            source.display(),
            target.display()
        )
    });
}
