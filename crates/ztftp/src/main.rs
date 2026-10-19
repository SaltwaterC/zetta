//! Standalone TFTP client and server entry point.
use anyhow::Result;

fn run() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let server = args.first().is_some_and(|argument| argument == "server");
    if args.is_empty()
        || args
            .iter()
            .any(|argument| argument == "--help" || argument == "-h")
    {
        println!(
            "{}",
            if server {
                ztftp::cli::tftp_server_help()
            } else {
                ztftp::tftp_help()
            }
        );
        return Ok(());
    }
    if server {
        ztftp::cli::parse_server_args(args.into_iter().skip(1))?.run()
    } else {
        ztftp::parse_tftp_args(args)?.run()
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("ztftp: {error:#}");
        std::process::exit(1);
    }
}
