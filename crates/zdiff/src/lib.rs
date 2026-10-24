//! Terminal pager for unified diff text supplied on standard input.

mod args;
mod diff_document;
mod help;
mod view;

use anyhow::{Context as _, Result, bail};
use std::ffi::OsString;
use std::io::{self, IsTerminal as _, Read as _, Write as _};

/// Runs `zdiff` with the supplied process arguments.
pub fn run<I>(arguments: I) -> Result<()>
where
    I: IntoIterator<Item = OsString>,
{
    match args::parse(arguments.into_iter().collect())? {
        args::ParsedArgs::Help => {
            println!("{}", help::command_help());
            Ok(())
        }
        args::ParsedArgs::Options(options) => {
            let stdin = io::stdin();
            if stdin.is_terminal() {
                bail!(
                    "expected diff text on standard input; try `git diff | zdiff` or `GIT_PAGER=zdiff git --paginate diff`"
                );
            }

            let mut input = Vec::new();
            stdin
                .lock()
                .read_to_end(&mut input)
                .context("failed to read diff from standard input")?;
            if input.is_empty() {
                return Ok(());
            }

            let stdout = io::stdout();
            if !stdout.is_terminal() {
                stdout
                    .lock()
                    .write_all(&input)
                    .context("failed to pass diff text to standard output")?;
                Ok(())
            } else {
                let document = diff_document::DiffDocument::parse(&input);
                if document.rows.is_empty() {
                    return Ok(());
                }
                view::run(document, options.color, options.columns, options.width)
            }
        }
    }
}

/// Entry point shared by the standalone tool build and root package binary.
pub fn cli_main() {
    if let Err(error) = run(std::env::args_os().skip(1)) {
        eprintln!("zdiff: {error:#}");
        std::process::exit(1);
    }
}
