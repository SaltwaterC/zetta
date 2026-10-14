//! CLI for the console-server harness, independent of Zetta's application CLI.
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
#[derive(Debug)]
pub struct Options {
    pub command: Option<String>,
    pub drain: bool,
    pub pipeline: bool,
    pub capture: Option<PathBuf>,
    pub report: Option<PathBuf>,
    pub timeout: u64,
    pub history: usize,
    pub ctrl_c: bool,
    pub control_child: bool,
    pub help: bool,
}
impl Options {
    pub fn parse(args: impl IntoIterator<Item = OsString>) -> io::Result<Self> {
        let mut args = args.into_iter();
        let mut options = Self {
            command: None,
            drain: false,
            pipeline: false,
            capture: None,
            report: None,
            timeout: 30,
            history: 500,
            ctrl_c: false,
            control_child: false,
            help: false,
        };
        while let Some(argument) = args.next() {
            let argument = argument
                .to_str()
                .ok_or_else(|| io::Error::other("invalid option encoding"))?;
            match argument {
                "--help" | "-h" => options.help = true,
                "--drain" | "-d" => options.drain = true,
                "--pipeline" | "-p" => options.pipeline = true,
                "--ctrl-c" | "-i" => options.ctrl_c = true,
                "--control-child" | "-C" => options.control_child = true,
                "--command" | "-c" => {
                    options.command = Some(
                        value(&mut args, argument)?
                            .into_string()
                            .map_err(|_| io::Error::other("invalid command encoding"))?,
                    );
                }
                "--capture" | "-o" => options.capture = Some(value(&mut args, argument)?.into()),
                "--report" | "-r" => options.report = Some(value(&mut args, argument)?.into()),
                "--timeout" | "-t" => options.timeout = number(&mut args, argument)?,
                "--history" | "-s" => options.history = number(&mut args, argument)? as usize,
                _ => return Err(io::Error::other(format!("unknown option {argument}"))),
            }
        }
        if options.timeout == 0 {
            return Err(io::Error::other("timeout must be positive"));
        }
        if !options.help && !options.control_child && !options.ctrl_c && options.command.is_none() {
            return Err(io::Error::other("supply --command or --ctrl-c; see --help"));
        }
        Ok(options)
    }
}
fn value(args: &mut impl Iterator<Item = OsString>, name: &str) -> io::Result<OsString> {
    args.next()
        .ok_or_else(|| io::Error::other(format!("{name} requires a value")))
}
fn number(args: &mut impl Iterator<Item = OsString>, name: &str) -> io::Result<u64> {
    value(args, name)?
        .to_str()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| io::Error::other(format!("{name} requires an unsigned integer")))
}
pub fn help() -> String {
    format!(
        "Console server\n\nUsage: console-server [OPTIONS]\n\n\
Launch a private child console and consume output with Zetta's 80x24 parser.\n\
Without --command, --ctrl-c launches this executable's handler probe.\n\nOptions:\n{}",
        format_help_table([
            (
                "--command, -c COMMAND",
                "Windows command line to launch (quote executable paths with spaces)"
            ),
            (
                "--drain, -d",
                "Discard output instead of parsing; measure transport ceiling"
            ),
            (
                "--capture, -o FILE",
                "Capture unchanged UTF-8 output; disable for performance runs"
            ),
            (
                "--pipeline, -p",
                "Embed the console and use the pooled parser thread; requires --command"
            ),
            ("--report, -r FILE", "Write a versioned JSON result"),
            (
                "--timeout, -t SECONDS",
                "Terminate only the launched child tree on timeout (default 30)"
            ),
            (
                "--history, -s LINES",
                "Parser scrollback limit (default 500)"
            ),
            (
                "--ctrl-c, -i",
                "Send Ctrl+C when the child writes ZETTA_CTRL_READY; require handler marker"
            ),
            ("--help, -h", "Show usage"),
        ])
    )
}

#[cfg(test)]
#[path = "tests/options.rs"]
mod tests;
pub(crate) fn format_help_table<'a>(rows: impl AsRef<[(&'a str, &'a str)]>) -> String {
    let rows = rows
        .as_ref()
        .iter()
        .map(|&(label, description)| (label.trim_end(), description))
        .collect::<Vec<_>>();
    let label_width = rows
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(0);
    rows.into_iter()
        .map(|(label, description)| {
            let mut lines = description.split('\n');
            let first_line = lines.next().unwrap_or("").trim_end();
            let mut formatted = String::new();
            formatted.push_str("  ");
            formatted.push_str(label);
            formatted.push_str(&" ".repeat(label_width - label.chars().count()));
            if !first_line.is_empty() {
                formatted.push_str("  ");
                formatted.push_str(first_line);
            }
            for line in lines {
                formatted.push('\n');
                let line = line.trim_end();
                if !line.is_empty() {
                    formatted.push_str("  ");
                    formatted.push_str(&" ".repeat(label_width));
                    formatted.push_str("  ");
                    formatted.push_str(line);
                }
            }
            formatted
        })
        .collect::<Vec<_>>()
        .join("\n")
}
