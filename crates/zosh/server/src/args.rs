use anyhow::{Context, Result, anyhow, bail};
use std::ffi::OsString;
use std::net::IpAddr;

pub const DEFAULT_PORT_LOW: u16 = 60000;
pub const DEFAULT_PORT_HIGH: u16 = 61000;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind_ip: Option<IpAddr>,
    pub port_low: u16,
    pub port_high: u16,
    pub colors: u32,
    pub locale_env: Vec<(String, String)>,
    pub command: Vec<OsString>,
    pub verbose: u8,
    pub foreground: bool,
    #[cfg_attr(
        not(windows),
        expect(
            dead_code,
            reason = "The lifecycle flag is consumed only by the Windows detached bootstrap."
        )
    )]
    pub internal_child: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind_ip: None,
            port_low: DEFAULT_PORT_LOW,
            port_high: DEFAULT_PORT_HIGH,
            colors: 256,
            locale_env: Vec::new(),
            command: Vec::new(),
            verbose: 0,
            foreground: false,
            internal_child: false,
        }
    }
}

pub enum ParseOutcome {
    Run(Config),
    Help,
    Version,
}

pub fn parse(raw: Vec<OsString>) -> Result<ParseOutcome> {
    // Hidden lifecycle flags are recognized only before the command separator.
    // Strip them before applying the bundled zosh-server's "new" argument semantics.
    let mut filtered = Vec::with_capacity(raw.len());
    let mut foreground = false;
    let mut internal_child = false;
    let mut after_separator = false;

    for arg in raw {
        if !after_separator {
            if arg == "--" {
                after_separator = true;
                filtered.push(arg);
                continue;
            }
            if arg == "--foreground" || arg == "--no-detach" {
                foreground = true;
                continue;
            }
            if arg == "--internal-child" {
                internal_child = true;
                continue;
            }
        }
        filtered.push(arg);
    }

    let mut cfg = Config {
        foreground,
        internal_child,
        ..Config::default()
    };

    let mut i = 0usize;
    let option_mode = filtered.first().is_some_and(|s| s == "new");
    if option_mode {
        i += 1;
    }

    // The bundled server historically accepts a no-option legacy invocation.
    // This implementation intentionally treats an invocation without "new" as
    // the default server unless explicit --help/--version is requested.
    if !option_mode {
        if filtered.is_empty() {
            return Ok(ParseOutcome::Run(cfg));
        }
        if filtered.len() == 1 && filtered[0] == "--help" {
            return Ok(ParseOutcome::Help);
        }
        if filtered.len() == 1 && filtered[0] == "--version" {
            return Ok(ParseOutcome::Version);
        }
        bail!(
            "options require 'new' as the first non-internal argument; try 'zosh-server new --help'"
        );
    }

    while i < filtered.len() {
        let arg = filtered[i].to_string_lossy();
        match arg.as_ref() {
            "--" => {
                cfg.command = filtered[i + 1..].to_vec();
                break;
            }
            "-h" | "--help" => return Ok(ParseOutcome::Help),
            "--version" => return Ok(ParseOutcome::Version),
            "-s" => {
                let value = std::env::var("SSH_CONNECTION")
                    .context("-s requires SSH_CONNECTION in the environment")?;
                cfg.bind_ip = Some(parse_ssh_server_ip(&value)?);
                i += 1;
            }
            "-i" => {
                let value = require_value(&filtered, i, "-i")?;
                cfg.bind_ip =
                    Some(value.to_string_lossy().parse::<IpAddr>().with_context(|| {
                        format!("invalid bind IP: {}", value.to_string_lossy())
                    })?);
                i += 2;
            }
            "-p" => {
                let value = require_value(&filtered, i, "-p")?;
                let (lo, hi) = parse_port_range(&value.to_string_lossy())?;
                cfg.port_low = lo;
                cfg.port_high = hi;
                i += 2;
            }
            "-c" => {
                let value = require_value(&filtered, i, "-c")?;
                cfg.colors = value
                    .to_string_lossy()
                    .parse::<u32>()
                    .context("invalid color count")?;
                i += 2;
            }
            "-l" => {
                let value = require_value(&filtered, i, "-l")?;
                let value = value.to_string_lossy();
                let (name, val) = value
                    .split_once('=')
                    .ok_or_else(|| anyhow!("-l expects NAME=VALUE"))?;
                if name.is_empty() {
                    bail!("-l variable name must not be empty");
                }
                cfg.locale_env.push((name.to_owned(), val.to_owned()));
                i += 2;
            }
            "-v" => {
                cfg.verbose = cfg.verbose.saturating_add(1);
                i += 1;
            }
            // Upstream accepts -@ as an implementation-specific argument and
            // ignores it on platforms where it is not relevant. Keep that
            // behavior so wrappers do not fail unexpectedly.
            "-@" => {
                let _ = require_value(&filtered, i, "-@")?;
                i += 2;
            }
            other if other.starts_with("-v") && other.chars().skip(1).all(|c| c == 'v') => {
                cfg.verbose = cfg
                    .verbose
                    .saturating_add((other.len().saturating_sub(1)) as u8);
                i += 1;
            }
            other => bail!("unknown option {other:?}; use -- before the command"),
        }
    }

    Ok(ParseOutcome::Run(cfg))
}

fn require_value<'a>(args: &'a [OsString], index: usize, flag: &str) -> Result<&'a OsString> {
    args.get(index + 1)
        .ok_or_else(|| anyhow!("{flag} requires an argument"))
}

pub fn parse_port_range(value: &str) -> Result<(u16, u16)> {
    if value == "0" {
        return Ok((0, 0));
    }
    if let Some((left, right)) = value.split_once(':') {
        let lo = left.parse::<u16>().context("invalid first UDP port")?;
        let hi = right.parse::<u16>().context("invalid last UDP port")?;
        if lo == 0 || hi == 0 {
            bail!("port 0 can only be used by itself");
        }
        if lo > hi {
            bail!("UDP port range must be ascending");
        }
        Ok((lo, hi))
    } else {
        let port = value.parse::<u16>().context("invalid UDP port")?;
        Ok((port, port))
    }
}

pub fn parse_ssh_server_ip(value: &str) -> Result<IpAddr> {
    // SSH_CONNECTION = client_ip client_port server_ip server_port
    let fields: Vec<&str> = value.split_whitespace().collect();
    if fields.len() != 4 {
        bail!("SSH_CONNECTION must contain four fields");
    }
    fields[2]
        .parse::<IpAddr>()
        .with_context(|| format!("invalid server IP in SSH_CONNECTION: {}", fields[2]))
}

pub fn usage() -> &'static str {
    "Usage: zosh-server [--foreground]\n\
     zosh-server new [-s] [-v] [-i LOCALADDR] [-p PORT[:PORT2]] [-c COLORS] \
[-l NAME=VALUE] [-- COMMAND...]\n\n\
Options:\n\
  -s              Bind to the server address from SSH_CONNECTION\n\
  -i IP           Bind to a specific local IP address\n\
  -p P[:P2]       UDP port or inclusive UDP port range (default 60000:61000)\n\
  -c COLORS        Advertise terminal color capability (default 256)\n\
  -l NAME=VALUE   Add an environment variable to the child session\n\
  -v              Increase diagnostics (repeatable)\n\
  --foreground    Do not detach; useful for debugging and service managers\n\
  -- COMMAND...   Run a command instead of the default shell\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_range() {
        assert_eq!(parse_port_range("60000").unwrap(), (60000, 60000));
        assert_eq!(parse_port_range("60000:61000").unwrap(), (60000, 61000));
        assert_eq!(parse_port_range("0").unwrap(), (0, 0));
        assert!(parse_port_range("61000:60000").is_err());
    }

    #[test]
    fn ssh_connection_server_ip() {
        assert_eq!(
            parse_ssh_server_ip("192.0.2.10 50123 198.51.100.20 22").unwrap(),
            "198.51.100.20".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            parse_ssh_server_ip("2001:db8::1 50123 2001:db8::2 22").unwrap(),
            "2001:db8::2".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn command_after_separator() {
        let args = vec![
            OsString::from("new"),
            OsString::from("-p"),
            OsString::from("60001"),
            OsString::from("--"),
            OsString::from("sh"),
            OsString::from("-lc"),
            OsString::from("echo hi"),
        ];
        let ParseOutcome::Run(cfg) = parse(args).unwrap() else {
            panic!("expected config")
        };
        assert_eq!(cfg.port_low, 60001);
        assert_eq!(cfg.command.len(), 3);
    }
}
