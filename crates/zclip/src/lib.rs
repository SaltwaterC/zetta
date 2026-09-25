//! Shared clipboard command parser. The optional backend belongs only to the standalone binaries.
use anyhow::{Context as _, Result};
use std::ffi::OsString;

pub mod host;
pub mod protocol;
pub mod remote;

fn format_help_table<'a>(rows: impl AsRef<[(&'a str, &'a str)]>) -> String {
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

pub(crate) const CLIPBOARD_DAEMON_FLAG: &str = "--internal-clipboard-daemon";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyCommand;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PasteCommand;

pub fn copy_help() -> String {
    format!(
        "Copy standard input to the clipboard\n\nUsage: zcopy [OPTIONS]\n\nReads standard input as UTF-8 text, mirroring macOS's pbcopy. In an interactive SSH or zosh pane, sends it to the clipboard of the displaying Zetta window. If no Zetta channel answers, uses the local clipboard backend when built with one. An explicit remote error exits nonzero. Also available as zetta copy and, outside macOS, pbcopy through shell integration.\n\nOptions:\n{}\n\nOn Linux and FreeBSD, a local copy starts a detached process that keeps serving the clipboard after this command exits, since the X11 and Wayland clipboards are only available while their owning process is running. A backend-free remote build has no local fallback.",
        format_help_table([
            (
                "-pboard NAME",
                "Accepted for pbcopy compatibility (general, ruler, find, or font); Zetta has only one clipboard, so this has no effect",
            ),
            ("-h, -help, --help", "Print help"),
        ])
    )
}

pub fn paste_help() -> String {
    format!(
        "Print the clipboard's contents\n\nUsage: zpaste [OPTIONS]\n\nWrites clipboard text to standard output, mirroring macOS's pbpaste. In an interactive SSH or zosh pane, reads the displaying Zetta window's clipboard when Allow Remote Clipboard Paste is enabled for that tab. If no Zetta channel answers, uses the local clipboard backend when built with one. An explicit denial or transfer error exits nonzero. Prints nothing if the clipboard is empty or holds no text. Also available as zetta paste and, outside macOS, pbpaste through shell integration.\n\nOptions:\n{}",
        format_help_table([
            (
                "-pboard NAME",
                "Accepted for pbpaste compatibility (general, ruler, find, or font); Zetta has only one clipboard, so this has no effect",
            ),
            (
                "-Prefer TYPE",
                "Accepted for pbpaste compatibility (txt, rtf, or ps); Zetta only stores plain text, so this has no effect",
            ),
            ("-h, -help, --help", "Print help"),
        ])
    )
}

fn parse_pboard_name(argument: &OsString) -> Result<()> {
    let value = argument.to_string_lossy();
    anyhow::ensure!(
        matches!(
            value.to_ascii_lowercase().as_str(),
            "general" | "ruler" | "find" | "font"
        ),
        "-pboard must be general, ruler, find, or font, got {value:?}"
    );
    Ok(())
}

fn parse_prefer_type(argument: &OsString) -> Result<()> {
    let value = argument.to_string_lossy();
    anyhow::ensure!(
        matches!(value.to_ascii_lowercase().as_str(), "txt" | "rtf" | "ps"),
        "-Prefer must be txt, rtf, or ps, got {value:?}"
    );
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CopyMode {
    Copy,
    Daemon,
}

pub fn parse_copy_args(args: impl IntoIterator<Item = OsString>) -> Result<CopyMode> {
    let mut pboard_seen = false;
    let mut daemon = false;
    let mut arguments = args.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            CLIPBOARD_DAEMON_FLAG => daemon = true,
            "-pboard" | "--pboard" => {
                anyhow::ensure!(!pboard_seen, "-pboard may only be specified once");
                pboard_seen = true;
                parse_pboard_name(
                    &arguments
                        .next()
                        .context("-pboard requires general, ruler, find, or font")?,
                )?;
            }
            "--help" | "-h" | "-help" => anyhow::bail!("{}", copy_help()),
            option if option.starts_with('-') => anyhow::bail!("unknown copy option {option:?}"),
            value => anyhow::bail!("unexpected copy argument {value:?}"),
        }
    }
    Ok(if daemon {
        CopyMode::Daemon
    } else {
        CopyMode::Copy
    })
}

pub fn parse_paste_args(args: impl IntoIterator<Item = OsString>) -> Result<PasteCommand> {
    let mut pboard_seen = false;
    let mut prefer_seen = false;
    let mut arguments = args.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            "-pboard" | "--pboard" => {
                anyhow::ensure!(!pboard_seen, "-pboard may only be specified once");
                pboard_seen = true;
                parse_pboard_name(
                    &arguments
                        .next()
                        .context("-pboard requires general, ruler, find, or font")?,
                )?;
            }
            "-Prefer" | "--Prefer" | "-prefer" | "--prefer" => {
                anyhow::ensure!(!prefer_seen, "-Prefer may only be specified once");
                prefer_seen = true;
                parse_prefer_type(
                    &arguments
                        .next()
                        .context("-Prefer requires txt, rtf, or ps")?,
                )?;
            }
            "--help" | "-h" | "-help" => anyhow::bail!("{}", paste_help()),
            option if option.starts_with('-') => anyhow::bail!("unknown paste option {option:?}"),
            value => anyhow::bail!("unexpected paste argument {value:?}"),
        }
    }
    Ok(PasteCommand)
}

#[cfg(feature = "backend")]
pub mod backend;

#[cfg(test)]
#[path = "tests/lib.rs"]
mod tests;
