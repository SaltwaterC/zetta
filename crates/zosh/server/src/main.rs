mod agent;
mod args;
mod child_exit;
mod lifecycle;
mod protocol;
mod server;
mod sleep_guard;
mod terminal_state;
mod timing;
mod user_stream;
mod wake;

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
                "zosh-server-rs {} (Mosh protocol 2)",
                env!("CARGO_PKG_VERSION")
            );
            Ok(())
        }
        args::ParseOutcome::Run(cfg) => {
            #[cfg(windows)]
            if !cfg.foreground && !cfg.internal_child && lifecycle::windows_parent_bootstrap(&raw)?
            {
                return Ok(());
            }

            server::run(cfg)
        }
    }
}
