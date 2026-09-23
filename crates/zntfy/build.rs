use std::env;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(linux_like)");
    println!("cargo::rustc-check-cfg=cfg(notify_cleanup_enabled)");
    if matches!(
        env::var("CARGO_CFG_TARGET_OS").as_deref(),
        Ok("linux") | Ok("freebsd")
    ) {
        println!("cargo::rustc-cfg=linux_like");
        println!("cargo::rustc-cfg=notify_cleanup_enabled");
    }
}
