use super::*;

use crate::mosh::parse_mosh_args;

pub(crate) fn parse_mosh_subcommand(arguments: &[OsString]) -> Result<StartupArgs> {
    let mut command = parse_mosh_args(arguments)?;
    command.raw_arguments = arguments.to_vec();
    Ok(StartupArgs::for_mode(StartupMode::Mosh(command)))
}
