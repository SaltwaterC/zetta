//! Which host a sealed session key belongs to, named by that host's SSH host
//! keys.
//!
//! A sealed key is opened on one machine and its plaintext sent to the
//! multiplexer that holds the session, which for a remote session is another
//! machine. That multiplexer is also what said which envelope to open, so a
//! host holding some *other* session's envelope could offer it as its own and
//! be sent that session's key. The envelope therefore names the host it was
//! sealed on, and the opener compares that name with one the remote host cannot
//! choose.
//!
//! The SSH host key is that name. The sealing side reads its own public host
//! keys; the opening side asks its own OpenSSH which keys it trusts for the
//! destination it is about to connect to — the `known_hosts` entries that very
//! connection is verified against. A host can only appear under a destination
//! the user trusts by holding the private half of one of those keys, so a
//! different host cannot borrow the name, and it survives what a host name
//! would not: aliases, IP addresses, jump hosts and forwarded ports.
//!
//! What it does not cover, and refuses rather than guesses at: a host whose
//! public host keys are not readable at the default paths when the key is
//! sealed, a destination trusted only through `@cert-authority` or a
//! `KnownHostsCommand`, and two hosts sharing one host key (a cloned image),
//! which are one host as far as SSH is concerned too.

use std::{
    io::Read as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use sha2::{Digest as _, Sha256};

use crate::remote::RemoteTarget;

/// The host key types read on the sealing side, in the file names OpenSSH
/// generates them under.
const HOST_KEY_TYPES: [&str; 3] = ["ed25519", "ecdsa", "rsa"];

/// How long `ssh -G` or `ssh-keygen -F` may take. Neither touches the
/// network, but a `Match exec` in the user's configuration runs a command.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);

/// The `SHA256:` fingerprints of this host's SSH host keys, as OpenSSH prints
/// them. Empty when none can be read, which leaves a key sealed here openable
/// only here.
pub(crate) fn local_fingerprints() -> Vec<String> {
    let mut fingerprints: Vec<String> = host_key_directories()
        .iter()
        .flat_map(|directory| {
            HOST_KEY_TYPES
                .iter()
                .map(move |key_type| directory.join(format!("ssh_host_{key_type}_key.pub")))
        })
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .filter_map(|contents| contents.lines().find_map(public_key_fingerprint))
        .collect();
    fingerprints.sort();
    fingerprints.dedup();
    fingerprints
}

fn host_key_directories() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("ProgramData")
            .map(|data| vec![PathBuf::from(data).join("ssh")])
            .unwrap_or_default()
    }
    #[cfg(not(windows))]
    {
        vec![
            PathBuf::from("/etc/ssh"),
            PathBuf::from("/usr/local/etc/ssh"),
        ]
    }
}

/// The fingerprints of the host keys this machine's OpenSSH trusts for
/// `target` — the keys a connection to it is verified against.
pub(crate) fn known_fingerprints(target: &RemoteTarget) -> Result<Vec<String>> {
    known_fingerprints_with(Path::new("ssh"), Path::new("ssh-keygen"), target)
}

pub(super) fn known_fingerprints_with(
    ssh: &Path,
    ssh_keygen: &Path,
    target: &RemoteTarget,
) -> Result<Vec<String>> {
    target.validate()?;
    let mut arguments = vec!["-G".to_owned()];
    if let Some(port) = target.port() {
        arguments.push("-p".to_owned());
        arguments.push(port.to_string());
    }
    arguments.push(target.destination().to_owned());
    let (success, configuration) = run(ssh, &arguments)
        .with_context(|| format!("asking OpenSSH how it reaches {}", target.destination()))?;
    anyhow::ensure!(
        success,
        "OpenSSH could not resolve its configuration for {}",
        target.destination()
    );
    let lookup = KnownHostsLookup::parse(&configuration).with_context(|| {
        format!(
            "reading OpenSSH's configuration for {}",
            target.destination()
        )
    })?;

    let mut trusted = Vec::new();
    let mut revoked = Vec::new();
    for file in lookup.files.iter().filter(|file| file.is_file()) {
        let arguments = [
            "-F".to_owned(),
            lookup.name.clone(),
            "-f".to_owned(),
            file.display().to_string(),
        ];
        // `ssh-keygen -F` exits non-zero when nothing matches; what it printed
        // is the answer either way, and it prints nothing then.
        let (_, entries) = run(ssh_keygen, &arguments)
            .with_context(|| format!("looking up {} in {}", lookup.name, file.display()))?;
        let entries = parse_known_hosts_entries(&entries);
        trusted.extend(entries.trusted);
        revoked.extend(entries.revoked);
    }
    trusted.retain(|fingerprint| !revoked.contains(fingerprint));
    trusted.sort();
    trusted.dedup();
    Ok(trusted)
}

/// What `ssh -G` says about where a destination's host keys are looked up.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct KnownHostsLookup {
    /// The name OpenSSH looks the host up under: the `HostKeyAlias`, or the
    /// resolved host name, bracketed with its port when that is not 22.
    pub(super) name: String,
    pub(super) files: Vec<PathBuf>,
}

impl KnownHostsLookup {
    pub(super) fn parse(configuration: &str) -> Result<Self> {
        let mut hostname = None;
        let mut port = None;
        let mut alias = None;
        let mut files = Vec::new();
        for line in configuration.lines() {
            let Some((option, value)) = line.trim().split_once(' ') else {
                continue;
            };
            let value = value.trim();
            match option.to_ascii_lowercase().as_str() {
                "hostname" => hostname = Some(value.to_owned()),
                "port" => port = value.parse::<u16>().ok(),
                "hostkeyalias" if !value.eq_ignore_ascii_case("none") => {
                    alias = Some(value.to_owned());
                }
                "userknownhostsfile" | "globalknownhostsfile" => {
                    files.extend(value.split_whitespace().filter_map(expand_known_hosts_path));
                }
                _ => {}
            }
        }
        let name = match (alias, hostname) {
            (Some(alias), _) => alias,
            (None, Some(hostname)) => match port {
                None | Some(22) => hostname,
                Some(port) => format!("[{hostname}]:{port}"),
            },
            (None, None) => anyhow::bail!("OpenSSH reported no host name"),
        };
        Ok(Self { name, files })
    }
}

fn expand_known_hosts_path(path: &str) -> Option<PathBuf> {
    if path.eq_ignore_ascii_case("none") || path == "/dev/null" {
        return None;
    }
    if let Some(rest) = path.strip_prefix('~') {
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
        return Some(PathBuf::from(home).join(rest.trim_start_matches(['/', '\\'])));
    }
    // Windows OpenSSH prints its system directory under this placeholder.
    if let Some(rest) = path.strip_prefix("__PROGRAMDATA__") {
        let data = std::env::var_os("ProgramData")?;
        return Some(PathBuf::from(data).join(rest.trim_start_matches(['/', '\\'])));
    }
    Some(PathBuf::from(path))
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct KnownHostsEntries {
    pub(super) trusted: Vec<String>,
    pub(super) revoked: Vec<String>,
}

/// The host keys in `known_hosts` lines. A `@cert-authority` line names a CA
/// rather than the host's own key, so it is no help in recognising the host
/// and is skipped.
pub(super) fn parse_known_hosts_entries(output: &str) -> KnownHostsEntries {
    let mut entries = KnownHostsEntries::default();
    for line in output.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace().peekable();
        let marker = fields
            .next_if(|field| field.starts_with('@'))
            .map(|marker| marker[1..].to_ascii_lowercase());
        let (Some(_hosts), Some(key_type), Some(key)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let Some(fingerprint) = fingerprint(key_type, key) else {
            continue;
        };
        match marker.as_deref() {
            None => entries.trusted.push(fingerprint),
            Some("revoked") => entries.revoked.push(fingerprint),
            Some(_) => {}
        }
    }
    entries
}

/// The fingerprint of a public key line: `type base64 [comment]`.
pub(super) fn public_key_fingerprint(line: &str) -> Option<String> {
    let mut fields = line.split_whitespace();
    fingerprint(fields.next()?, fields.next()?)
}

/// OpenSSH's `SHA256:` fingerprint of a key blob, after checking that the blob
/// names the type it is listed under.
fn fingerprint(key_type: &str, key: &str) -> Option<String> {
    let blob = STANDARD.decode(key).ok()?;
    let length = u32::from_be_bytes(blob.get(..4)?.try_into().ok()?) as usize;
    if blob.get(4..4 + length)? != key_type.as_bytes() {
        return None;
    }
    Some(format!(
        "SHA256:{}",
        STANDARD_NO_PAD.encode(Sha256::digest(&blob))
    ))
}

/// Runs `program`, returning whether it succeeded and what it printed.
fn run(program: &Path, arguments: &[String]) -> Result<(bool, String)> {
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("starting {}", program.display()))?;
    let mut stdout = child.stdout.take().context("stdout was not captured")?;
    let reader = thread::spawn(move || {
        let mut output = Vec::new();
        stdout.read_to_end(&mut output).map(|_| output)
    });
    let deadline = Instant::now() + LOOKUP_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("{} did not finish in {LOOKUP_TIMEOUT:?}", program.display());
        }
        thread::sleep(Duration::from_millis(10));
    };
    let output = reader
        .join()
        .map_err(|_| anyhow::anyhow!("the output reader panicked"))??;
    Ok((
        status.success(),
        String::from_utf8(output).context("the output was not UTF-8")?,
    ))
}

#[cfg(test)]
#[path = "../tests/auto_protect/host_keys.rs"]
mod tests;
