//! `zetta tftp setcap`: grants `zetta` and the `ztftp` beside it the one
//! capability the TFTP server needs, `cap_net_bind_service`, so that a
//! user-local installation can serve on port 69 without running as root.
//! `zetta` binds in-process for the GUI's server panes; `zetta tftp server`
//! runs `ztftp`.
//!
//! A file capability lives in the `security.capability` extended attribute,
//! and installing a new binary replaces the file and drops it, so it is lost
//! on every install. That makes this a post-install step: it checks first,
//! asks for root only when something is missing, and does nothing when the
//! capability is already in place or the system does not require one.
//!
//! The grant itself runs `setcap` under `sudo`, rather than running this
//! binary as root. The privileged command is then exactly the one printed,
//! and it names only these two files.

use std::{
    ffi::{CString, OsStr, OsString},
    fs,
    io::{self, IsTerminal as _},
    os::unix::{ffi::OsStrExt as _, fs::PermissionsExt as _},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context as _, Result};

use super::CliServiceCommand;
use crate::startup::format_help_table;

/// TFTP's well-known port, which is what the capability is for.
const TFTP_PORT: u16 = 69;
const CAPABILITY: &str = "cap_net_bind_service";
const CAPABILITY_ATTRIBUTE: &str = "security.capability";
const UNPRIVILEGED_PORT_START: &str = "/proc/sys/net/ipv4/ip_unprivileged_port_start";

// `struct vfs_cap_data` from linux/capability.h: a little-endian magic word
// carrying the revision and the effective flag, then permitted/inheritable
// pairs (one pair in revision 1, two in 2 and 3), then in revision 3 the
// owning user namespace's root uid.
const CAP_NET_BIND_SERVICE: u32 = 10;
const VFS_CAP_REVISION_MASK: u32 = 0xff00_0000;
const VFS_CAP_REVISION_1: u32 = 0x0100_0000;
const VFS_CAP_REVISION_2: u32 = 0x0200_0000;
const VFS_CAP_REVISION_3: u32 = 0x0300_0000;
const VFS_CAP_FLAGS_EFFECTIVE: u32 = 0x0000_0001;
const VFS_CAP_MAX_BYTES: usize = 24;

/// Where `setcap` lives when a user's `PATH` lacks the `sbin` directories,
/// as it does by default on Debian.
const SETCAP_FALLBACK_DIRECTORIES: [&str; 3] = ["/usr/sbin", "/sbin", "/usr/bin"];

/// The line `zetta tftp --help` ends with, pointing here.
pub(crate) const TFTP_SETCAP_HINT: &str =
    "Run `zetta tftp setcap` to let the server bind port 69 without root.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TftpSetcapCommand {
    check_only: bool,
}

pub(crate) fn tftp_setcap_help() -> String {
    format!(
        "Let the TFTP server bind port {TFTP_PORT} without root\n\nUsage: zetta tftp setcap [OPTIONS]\n\nOptions:\n{}\n\nGrants {CAPABILITY} to this zetta executable and the ztftp beside it,\nrunning setcap through sudo when this is not already root. Installing new\nbinaries drops the capability, so run this again after each install; it\ndoes nothing when the capability is already granted or the system lets every\nuser bind port {TFTP_PORT}.",
        format_help_table([
            (
                "-c, --check",
                "Report whether the capability is granted; exit 1 when it is missing",
            ),
            ("-h, --help", "Print help"),
        ])
    )
}

pub(crate) fn parse_tftp_setcap_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<CliServiceCommand> {
    let mut check_only = false;
    for argument in args {
        match argument.to_string_lossy().as_ref() {
            "--check" | "-c" => {
                anyhow::ensure!(!check_only, "--check may only be specified once");
                check_only = true;
            }
            option if option.starts_with('-') => {
                anyhow::bail!("unknown tftp setcap option {option:?}")
            }
            value => anyhow::bail!("unexpected tftp setcap argument {value:?}"),
        }
    }
    Ok(CliServiceCommand::TftpSetcap(TftpSetcapCommand {
        check_only,
    }))
}

impl TftpSetcapCommand {
    pub(super) fn run(self) -> Result<()> {
        if let Some(start) = read_unprivileged_port_start()
            && start <= TFTP_PORT
        {
            println!(
                "Not needed: this system lets every user bind ports from {start} up (net.ipv4.ip_unprivileged_port_start)."
            );
            return Ok(());
        }
        let binaries = tftp_binaries()?;
        let missing = report_missing(&binaries)?;
        if missing.is_empty() {
            return Ok(());
        }
        for binary in &missing {
            anyhow::ensure!(
                !on_nosuid_mount(binary)?,
                "{} is on a nosuid filesystem, where Linux ignores file capabilities; install Zetta elsewhere to serve TFTP on port {TFTP_PORT}",
                binary.display()
            );
        }
        anyhow::ensure!(
            !self.check_only,
            "{CAPABILITY} is missing; run `zetta tftp setcap` to grant it"
        );
        grant(&missing)?;
        let still_missing = report_missing(&missing)?;
        anyhow::ensure!(
            still_missing.is_empty(),
            "setcap succeeded, but {CAPABILITY} is still missing"
        );
        Ok(())
    }
}

/// `zetta` itself and the `ztftp` that `zetta tftp server` runs, found the
/// same way `run_tftp_proxy` finds it. A build without `ztftp` beside it has
/// only the first.
fn tftp_binaries() -> Result<Vec<PathBuf>> {
    let zetta = std::env::current_exe().context("locating the zetta executable")?;
    let ztftp = zetta.with_file_name("ztftp");
    let mut binaries = vec![zetta];
    if ztftp.is_file() {
        binaries.push(ztftp);
    }
    Ok(binaries)
}

/// Prints each binary's state and returns the ones without the capability.
fn report_missing(binaries: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut missing = Vec::new();
    for binary in binaries {
        if read_capability(binary)?.is_some_and(|data| grants_net_bind_service(&data)) {
            println!("{CAPABILITY} is granted to {}", binary.display());
        } else {
            println!("{CAPABILITY} is missing from {}", binary.display());
            missing.push(binary.clone());
        }
    }
    Ok(missing)
}

fn grant(binaries: &[PathBuf]) -> Result<()> {
    let setcap = find_setcap(std::env::var_os("PATH").as_deref()).context(
        "setcap was not found; install libcap2-bin (Debian, Ubuntu), libcap (Fedora, Arch) or libcap-progs (openSUSE), then run `zetta tftp setcap` again",
    )?;
    let mut command_line = vec![setcap.into_os_string()];
    command_line.extend(setcap_arguments(binaries));
    // SAFETY: geteuid has no preconditions and cannot fail.
    if unsafe { libc::geteuid() } != 0 {
        anyhow::ensure!(
            io::stdin().is_terminal(),
            "granting {CAPABILITY} needs root, and there is no terminal to ask for a password on; run:\n  sudo {}",
            display_command(&command_line)
        );
        command_line.insert(0, OsString::from("sudo"));
    }
    println!("Running: {}", display_command(&command_line));
    let status = Command::new(&command_line[0])
        .args(&command_line[1..])
        .status()
        .with_context(|| {
            format!(
                "starting {}; run as root instead: {}",
                command_line[0].to_string_lossy(),
                display_command(&command_line)
            )
        })?;
    anyhow::ensure!(status.success(), "setcap failed with {status}");
    Ok(())
}

/// `CAP=+ep FILE` once per file, which `setcap` accepts in one invocation.
fn setcap_arguments(binaries: &[PathBuf]) -> Vec<OsString> {
    binaries
        .iter()
        .flat_map(|binary| {
            [
                OsString::from(format!("{CAPABILITY}=+ep")),
                binary.as_os_str().to_owned(),
            ]
        })
        .collect()
}

fn find_setcap(path: Option<&OsStr>) -> Option<PathBuf> {
    path.into_iter()
        .flat_map(std::env::split_paths)
        .chain(SETCAP_FALLBACK_DIRECTORIES.iter().map(PathBuf::from))
        .map(|directory| directory.join("setcap"))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// A command line as it would be typed, quoting only words that need it.
fn display_command(words: &[OsString]) -> String {
    words
        .iter()
        .map(|word| {
            let word = word.to_string_lossy();
            if !word.is_empty()
                && word.chars().all(|character| {
                    character.is_ascii_alphanumeric() || "-_./=+:@".contains(character)
                })
            {
                word.into_owned()
            } else {
                format!("'{}'", word.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn read_unprivileged_port_start() -> Option<u16> {
    parse_unprivileged_port_start(&fs::read_to_string(UNPRIVILEGED_PORT_START).ok()?)
}

fn parse_unprivileged_port_start(contents: &str) -> Option<u16> {
    contents.trim().parse().ok()
}

/// Whether the decoded `security.capability` value puts `cap_net_bind_service`
/// in the permitted set with the effective flag, which is what `+ep` writes.
/// A revision 3 value scoped to another user namespace does not apply here.
fn grants_net_bind_service(data: &[u8]) -> bool {
    let words = data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|word| u32::from_le_bytes(*word))
        .collect::<Vec<_>>();
    let Some(&magic) = words.first() else {
        return false;
    };
    let expected_words = match magic & VFS_CAP_REVISION_MASK {
        VFS_CAP_REVISION_1 => 3,
        VFS_CAP_REVISION_2 => 5,
        VFS_CAP_REVISION_3 => 6,
        _ => return false,
    };
    if data.len() != expected_words * 4 {
        return false;
    }
    if expected_words == 6 && words[5] != 0 {
        return false;
    }
    magic & VFS_CAP_FLAGS_EFFECTIVE != 0 && words[1] & (1 << CAP_NET_BIND_SERVICE) != 0
}

/// The raw `security.capability` value, or `None` when the file has none.
fn read_capability(path: &Path) -> Result<Option<Vec<u8>>> {
    let c_path = CString::new(path.as_os_str().as_bytes())
        .with_context(|| format!("{} contains a NUL byte", path.display()))?;
    let name = CString::new(CAPABILITY_ATTRIBUTE).expect("the attribute name has no NUL byte");
    let mut buffer = [0_u8; VFS_CAP_MAX_BYTES];
    // SAFETY: both strings are NUL-terminated and outlive the call, and the
    // buffer pointer and length describe `buffer`, which getxattr writes at
    // most `buffer.len()` bytes into.
    let length = unsafe {
        libc::getxattr(
            c_path.as_ptr(),
            name.as_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
        )
    };
    if let Ok(length) = usize::try_from(length) {
        return Ok(Some(buffer[..length].to_vec()));
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ENODATA) => Ok(None),
        // Larger than any revision this reads, so not one it can vouch for.
        Some(libc::ERANGE) => Ok(Some(Vec::new())),
        Some(libc::ENOTSUP) => anyhow::bail!(
            "{} is on a filesystem without extended attributes, which cannot hold file capabilities",
            path.display()
        ),
        _ => Err(error).with_context(|| format!("reading the capabilities of {}", path.display())),
    }
}

fn on_nosuid_mount(path: &Path) -> Result<bool> {
    let c_path = CString::new(path.as_os_str().as_bytes())
        .with_context(|| format!("{} contains a NUL byte", path.display()))?;
    // SAFETY: an all-zero statvfs is a valid value of this plain C struct,
    // which statvfs overwrites on success.
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: the path is NUL-terminated and outlives the call, and `stats`
    // is a valid, writable statvfs.
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stats) } != 0 {
        return Err(io::Error::last_os_error())
            .with_context(|| format!("inspecting the filesystem of {}", path.display()));
    }
    Ok(stats.f_flag & libc::ST_NOSUID != 0)
}

#[cfg(test)]
#[path = "../tests/cli_services/tftp_setcap.rs"]
mod tests;
