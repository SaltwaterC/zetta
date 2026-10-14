//! Standalone Windows console-server benchmark and compatibility probe.
//! Zetta embeds the library; this executable remains the independent probe.
#[cfg(windows)]
use console_server::control;
#[cfg(windows)]
use console_server::native;
#[cfg(windows)]
mod options;
#[cfg(windows)]
mod pipeline;
#[cfg(windows)]
mod run;
#[cfg(windows)]
use console_server::server;
#[cfg(windows)]
mod sink;

fn main() {
    if let Err(error) = entry() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
fn entry() -> std::io::Result<()> {
    #[cfg(windows)]
    {
        let options = options::Options::parse(std::env::args_os().skip(1))?;
        if options.help {
            println!("{}", options::help());
            return Ok(());
        }
        if options.pipeline {
            pipeline::run(options)
        } else {
            run::run(options)
        }
    }
    #[cfg(not(windows))]
    {
        Err(std::io::Error::other("console-server requires Windows"))
    }
}

#[cfg(windows)]
use console_server::output;
