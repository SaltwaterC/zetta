//! Getting the relay running inside the distribution, over the stdio of the
//! `wsl.exe` that will carry its traffic.
//!
//! `SCRIPT` runs first, under `/bin/sh`. It picks the relay for the
//! distribution's architecture and asks for it only when the user's cache does
//! not already hold that exact build. A cached relay is named by a hash of its
//! contents, so it is never stale, and a transfer is written under another
//! name and renamed once complete, so an interrupted one is never mistaken for
//! a relay. The script then execs the relay, which announces its socket.
//!
//! Copying the relay over stdio rather than through `/mnt/c` is deliberate:
//! the Windows drives may be mounted elsewhere, `noexec`, or not at all.
//!
//! The lines meant for `wslx.exe` start with `LINE_PREFIX`, so anything else
//! printed before the script runs is skipped rather than misread. After
//! `ready` the stream carries `protocol` messages, which is why `handshake`
//! reads through a caller's `BufRead`: whatever it buffered past that line
//! belongs to the bridge.

use std::io::{self, BufRead, Read, Write};

/// The script, run as `sh -c SCRIPT` followed by `RelayImages::script_arguments`.
pub const SCRIPT: &str = include_str!("bootstrap/bootstrap.sh");

pub const LINE_PREFIX: &str = "@wslx ";

/// The longest line read while waiting for the relay. A line from the script
/// or the relay is far shorter; this only bounds what noise can cost.
const MAX_LINE: u64 = 4096;

/// How many lines that are not for `wslx.exe` are tolerated.
const MAX_NOISE_LINES: usize = 64;

/// One build of the relay, for one architecture.
pub struct RelayImage<'a> {
    pub bytes: &'a [u8],
    pub hash: String,
}

impl<'a> RelayImage<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            hash: content_hash(bytes),
        }
    }
}

/// The relays `wslx.exe` carries, one per architecture WSL runs on.
pub struct RelayImages<'a> {
    pub x86_64: RelayImage<'a>,
    pub aarch64: RelayImage<'a>,
}

impl RelayImages<'_> {
    /// `$0`, then the hashes in the order the script reads them.
    pub fn script_arguments(&self) -> [&str; 3] {
        ["wslx", &self.x86_64.hash, &self.aarch64.hash]
    }

    fn image(&self, arch: &str) -> Option<&RelayImage<'_>> {
        match arch {
            "x86_64" => Some(&self.x86_64),
            "aarch64" => Some(&self.aarch64),
            _ => None,
        }
    }
}

/// What the script or the relay says on a line meant for `wslx.exe`.
#[derive(Debug, PartialEq, Eq)]
pub enum Announcement<'a> {
    /// Send the relay for this architecture: its size on a line, then it.
    Send(&'a str),
    /// The relay is listening on this socket.
    Ready(&'a str),
    /// The script or the relay gave up, saying why.
    Error(&'a str),
}

pub fn parse_line(line: &str) -> Option<Announcement<'_>> {
    let line = line.strip_suffix('\n').unwrap_or(line);
    let line = line.strip_suffix('\r').unwrap_or(line);
    let (verb, argument) = line.strip_prefix(LINE_PREFIX)?.split_once(' ')?;
    match verb {
        "send" => Some(Announcement::Send(argument)),
        "ready" => Some(Announcement::Ready(argument)),
        "error" => Some(Announcement::Error(argument)),
        _ => None,
    }
}

/// Answers the script until the relay is ready, returning its socket.
pub fn handshake(
    input: &mut impl BufRead,
    output: &mut impl Write,
    images: &RelayImages<'_>,
) -> io::Result<String> {
    let mut noise = 0;
    loop {
        let mut line = String::new();
        input.by_ref().take(MAX_LINE).read_line(&mut line)?;
        if line.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the relay exited before it was ready",
            ));
        }
        match parse_line(&line) {
            Some(Announcement::Send(arch)) => {
                let image = images.image(arch).ok_or_else(|| {
                    io::Error::other(format!(
                        "the relay asked for an unknown architecture {arch}"
                    ))
                })?;
                writeln!(output, "{}", image.bytes.len())?;
                output.write_all(image.bytes)?;
                output.flush()?;
            }
            Some(Announcement::Ready(socket)) => return Ok(socket.to_owned()),
            Some(Announcement::Error(reason)) => return Err(io::Error::other(reason.to_owned())),
            None => {
                noise += 1;
                if noise > MAX_NOISE_LINES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "the relay never announced itself",
                    ));
                }
            }
        }
    }
}

/// FNV-1a, 64-bit, as hex: enough to tell builds apart in a cache that only
/// `wslx.exe` writes to. It is not a defence against a hostile cache — anyone
/// who can write to the user's home inside the distribution already has the
/// user.
pub fn content_hash(bytes: &[u8]) -> String {
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    format!("{hash:016x}")
}

#[cfg(test)]
#[path = "tests/bootstrap.rs"]
mod tests;
