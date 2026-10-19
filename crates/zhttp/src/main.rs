//! Standalone HTTP server entry point.
use anyhow::Result;

fn run() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.is_empty()
        || args
            .iter()
            .any(|argument| argument == "--help" || argument == "-h")
    {
        println!("{}", zhttp::cli::http_server_help());
        return Ok(());
    }
    zhttp::cli::parse_http_args(args)?.run()
}

fn main() {
    if let Err(error) = run() {
        eprintln!("zhttp: {error:#}");
        std::process::exit(1);
    }
}
