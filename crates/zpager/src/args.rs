//! Parsing for `zpager`'s pager options.

use anyhow::{Context as _, Result, bail};
use std::ffi::OsString;

const DEFAULT_COLUMNS: usize = 80;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Options {
    pub(crate) color: bool,
    pub(crate) columns: usize,
    pub(crate) width: Option<usize>,
}

pub(crate) enum ParsedArgs {
    Help,
    Options(Options),
}

pub(crate) fn parse(arguments: Vec<OsString>) -> Result<ParsedArgs> {
    let no_color = std::env::var("NO_COLOR").is_ok_and(|value| !value.is_empty());
    let mut color = !no_color;
    let mut columns_override = None;
    let mut width_override = None;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--help" | "-h") => return Ok(ParsedArgs::Help),
            Some("--no-color" | "-n") => color = false,
            Some("--columns" | "-c") => {
                let value = arguments
                    .next()
                    .context("--columns requires a column count")?;
                columns_override = Some(parse_columns(&value, "--columns")?);
            }
            Some("--width" | "-w") => {
                let value = arguments
                    .next()
                    .context("--width requires a column count")?;
                width_override = Some(parse_columns(&value, "--width")?);
            }
            Some(value) if value.starts_with("--columns=") => {
                columns_override = Some(parse_column_text(
                    &value["--columns=".len()..],
                    "--columns",
                )?);
            }
            Some(value) if value.starts_with("--width=") => {
                width_override = Some(parse_column_text(&value["--width=".len()..], "--width")?);
            }
            Some(value) if value.starts_with("-c") && value.len() > 2 => {
                columns_override = Some(parse_column_text(&value[2..], "-c")?);
            }
            Some(value) if value.starts_with("-w") && value.len() > 2 => {
                width_override = Some(parse_column_text(&value[2..], "-w")?);
            }
            Some(option) => bail!(
                "unknown option {option:?}; zpager reads a diff from standard input, see --help"
            ),
            None => bail!("arguments must be valid UTF-8; see --help"),
        }
    }

    let columns = columns_override.map_or_else(configured_columns, Ok)?;
    let width = width_override.map_or_else(configured_width, |width| Ok(Some(width)))?;
    Ok(ParsedArgs::Options(Options {
        color,
        columns,
        width,
    }))
}

fn configured_width() -> Result<Option<usize>> {
    match std::env::var("ZDIFF_WIDTH") {
        Ok(value) => Ok(Some(parse_column_text(&value, "ZDIFF_WIDTH")?)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            bail!("ZDIFF_WIDTH must be valid UTF-8")
        }
    }
}

fn configured_columns() -> Result<usize> {
    match std::env::var("ZDIFF_COLUMNS") {
        Ok(value) => parse_column_text(&value, "ZDIFF_COLUMNS"),
        Err(std::env::VarError::NotPresent) => Ok(DEFAULT_COLUMNS),
        Err(std::env::VarError::NotUnicode(_)) => {
            bail!("ZDIFF_COLUMNS must be valid UTF-8")
        }
    }
}

fn parse_columns(value: &std::ffi::OsStr, option: &str) -> Result<usize> {
    let value = value
        .to_str()
        .with_context(|| format!("{option} value must be valid UTF-8"))?;
    parse_column_text(value, option)
}

fn parse_column_text(value: &str, option: &str) -> Result<usize> {
    let columns = value
        .parse::<usize>()
        .with_context(|| format!("{option} requires a positive integer, got {value:?}"))?;
    if columns == 0 {
        bail!("{option} must be greater than zero");
    }
    Ok(columns)
}
