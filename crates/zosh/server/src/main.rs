mod args;
mod lifecycle;
mod protocol;
mod server;
mod terminal_state;
mod user_stream;

use anyhow::Result;

fn main() -> Result<()> {
    let raw: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    match args::parse(raw.clone())? {
        args::ParseOutcome::Help => {
            print!("{}", args::usage());
            Ok(())
        }
        args::ParseOutcome::Version => {
            println!(
                "mosh-server-rs {} (Mosh protocol 2)",
                env!("CARGO_PKG_VERSION")
            );
            Ok(())
        }
        args::ParseOutcome::Run(cfg) => {
            #[cfg(windows)]
            if !cfg.foreground && !cfg.internal_child {
                if lifecycle::windows_parent_bootstrap(&raw)? {
                    return Ok(());
                }
            }

            server::run(cfg)
        }
    }
}
