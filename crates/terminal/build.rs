//! Stages OpenConsole beside this crate's test binaries on Windows.
//!
//! `alacritty_terminal` loads `conpty.dll` only from the application directory
//! or System32, so without this every test here would run against the inbox
//! ConPTY rather than the OpenConsole the application ships with, and
//! `conpty_startup_reply_does_not_wait_for_the_ui` would test a startup
//! handshake that never happens. The runtime comes from the cache the root
//! build shares under `<repository>/target`, downloaded and verified there if
//! no build has filled it yet.

use std::{env, path::Path};

#[path = "../../build_support/conpty_runtime.rs"]
mod conpty_runtime;
#[path = "../../build_support/conpty_tools.rs"]
mod conpty_tools;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest_dir =
        env::var("CARGO_MANIFEST_DIR").expect("Cargo did not provide the manifest directory");
    let repository = Path::new(&manifest_dir).join("../..");
    // Unit and integration test binaries are linked into `deps`.
    conpty_runtime::stage(&repository, &[conpty_runtime::profile_dir().join("deps")]);
}
