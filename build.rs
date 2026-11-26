use std::{env, path::Path};

#[path = "build_support/conpty_runtime.rs"]
mod conpty_runtime;
#[path = "build_support/conpty_tools.rs"]
mod conpty_tools;

fn main() {
    emit_cfg_aliases();

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    // Increase stack size for Windows to avoid stack overflow in debug builds
    // Default is 1MB, increase to 8MB
    if env::var("PROFILE").as_deref() == Ok("debug") {
        let flag = if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
            "-Wl,--stack,8388608"
        } else {
            "/STACK:8388608"
        };
        println!("cargo:rustc-link-arg={flag}");
    }

    let icon = "assets/icons/zetta-terminal-icon.ico";
    let resource = "resources/windows/zetta.rc";
    let profile_icons = [
        "assets/icons/profile/zetta.svg",
        "assets/icons/profile/zetta.ico",
        "assets/icons/profile/tux.png",
        "assets/icons/profile/tux.ico",
        "assets/icons/profile/bash.png",
        "assets/icons/profile/bash.ico",
        "assets/icons/profile/zsh.svg",
        "assets/icons/profile/zsh.ico",
        "assets/icons/profile/fish.svg",
        "assets/icons/profile/fish.ico",
    ];

    println!("cargo:rerun-if-changed={icon}");
    println!("cargo:rerun-if-changed={resource}");
    for profile_icon in profile_icons {
        println!("cargo:rerun-if-changed={profile_icon}");
    }

    embed_resource::compile(resource, embed_resource::NONE)
        .manifest_required()
        .unwrap();

    stage_conpty_runtime();
}

/// Collapses platform and feature predicates that are otherwise respelled at
/// every use site into single-word `cfg` aliases. Each alias must also be
/// declared to `rustc-check-cfg` or `cargo check` reports it as unexpected.
fn emit_cfg_aliases() {
    println!("cargo::rustc-check-cfg=cfg(linux_like)");
    println!("cargo::rustc-check-cfg=cfg(drawn_window_controls)");
    println!("cargo::rustc-check-cfg=cfg(servers_enabled)");
    println!("cargo::rustc-check-cfg=cfg(tftp_enabled)");
    println!("cargo::rustc-check-cfg=cfg(byte_stream_panes)");
    println!("cargo::rustc-check-cfg=cfg(cli_services)");
    println!("cargo::rustc-check-cfg=cfg(notify_cleanup_enabled)");
    println!("cargo::rustc-check-cfg=cfg(tftp_setcap)");
    println!("cargo::rustc-check-cfg=cfg(ssh_clients)");

    let linux_like = matches!(
        env::var("CARGO_CFG_TARGET_OS").as_deref(),
        Ok("linux") | Ok("freebsd")
    );
    if linux_like {
        println!("cargo::rustc-cfg=linux_like");
    }
    if linux_like || env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // The platforms whose window buttons Zetta draws itself, and so has to
        // know the window's capabilities for. macOS draws its own traffic lights.
        println!("cargo::rustc-cfg=drawn_window_controls");
    }
    if linux_like && feature_enabled("NOTIFICATIONS") {
        // The leaked-worker bug behind `notify cleanup` reaps only reproduces on the
        // D-Bus-backed Linux/BSD notification path; other platforms tear
        // their workers down as soon as the notification is submitted.
        println!("cargo::rustc-cfg=notify_cleanup_enabled");
    }
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") && feature_enabled("TFTP_SERVER") {
        // `zetta tftp setcap`: file capabilities are Linux's, and only the
        // TFTP server binds a privileged port. macOS and Windows let any user
        // bind port 69.
        println!("cargo::rustc-cfg=tftp_setcap");
    }
    if feature_enabled("HTTP_SERVER") || feature_enabled("TFTP_SERVER") {
        println!("cargo::rustc-cfg=servers_enabled");
    }
    if feature_enabled("TFTP_SERVER") || feature_enabled("TFTP_CLIENT") {
        println!("cargo::rustc-cfg=tftp_enabled");
    }
    if feature_enabled("ZMUX") || feature_enabled("ZOSH_CLIENT") {
        // The commands that reach a host over SSH: `mux` and `mosh`. They are
        // what parse attached option values and complete `~/.ssh/config` hosts.
        println!("cargo::rustc-cfg=ssh_clients");
    }
    if feature_enabled("SERIAL_CONSOLE")
        || feature_enabled("HTTP_SERVER")
        || feature_enabled("TFTP_SERVER")
    {
        println!("cargo::rustc-cfg=byte_stream_panes");
    }
    if [
        "SERIAL_CONSOLE",
        "HTTP_SERVER",
        "TFTP_SERVER",
        "NOTIFICATIONS",
        "CLIPBOARD",
    ]
    .iter()
    .any(|feature| feature_enabled(feature))
    {
        println!("cargo::rustc-cfg=cli_services");
    }
}

fn feature_enabled(feature: &str) -> bool {
    env::var_os(format!("CARGO_FEATURE_{feature}")).is_some()
}

fn stage_conpty_runtime() {
    let repository =
        env::var("CARGO_MANIFEST_DIR").expect("Cargo did not provide the manifest directory");
    conpty_runtime::stage(Path::new(&repository), &[conpty_runtime::profile_dir()]);
}
