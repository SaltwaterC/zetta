//! Image paste for a foreground OpenSSH or Mosh process.
//!
//! A local terminal normally sends the native image-paste chord. When the
//! foreground process is OpenSSH, the remote application cannot read the
//! desktop clipboard, so this module sends a PNG through a second, batch-mode
//! SSH connection and pastes the resulting remote path instead.
//!
//! A Mosh session is the same problem reached a different way: `zosh` is the
//! launcher *and* the client in one process, so its argument vector still names
//! the SSH command and target it bootstrapped through. Recovering those turns a
//! Mosh pane into the invocation the OpenSSH path above already knows how to
//! upload through — Mosh's own UDP `-p`/`--port` deliberately plays no part in
//! it, because the auxiliary connection is taken from `--ssh` alone.
//!
//! Windows WSL profiles also lack access to the desktop image clipboard. When
//! no SSH or Mosh target is reported, `wsl` stages the image in that profile's
//! distribution instead of asking its Linux application to read the clipboard.
//!
//! The command line this module reads is untrusted, and it starts a process
//! from it. On a WSL, MSYS2 or Cygwin pane it is the shell integration's
//! `zetta-cmd` title marker, which the terminal accepts only with the nonce it
//! gave that shell — otherwise any output could name the command. Even then,
//! nothing is passed through as written: the `ssh` executable is chosen here
//! (`with_trusted_program`), and the options are rebuilt from an allowlist
//! (`parse_ssh_argv`), so neither a `ProxyCommand` nor an `ssh` planted in a
//! working directory runs on a paste.
//!
//! Recognising a Mosh foreground process is also what project detection needs
//! (see [`foreground_is_mosh_session`]), so that predicate lives here too.

#[cfg(any(windows, test))]
mod wsl;

use std::{
    collections::HashMap,
    io::{Read, Write},
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use gpui::Image;
use task::Shell;
use terminal::{ImagePasteHandler, ImagePasteResult};

use crate::image_paste::normalize_image;

#[cfg(windows)]
use crate::{cygwin_profile, msys2_profile};

#[cfg(any(windows, test))]
use crate::is_wsl_shell;

#[cfg(windows)]
use std::path::Path;

const SSH_CONNECT_TIMEOUT_SECONDS: u64 = 15;
const SSH_TRANSFER_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_REMOTE_PATH_BYTES: usize = 4096;
const IMAGE_FILE_NAME: &str = "image.png";

static NEXT_SENTINEL_ID: AtomicU64 = AtomicU64::new(1);

/// Resolves clipboard images for a local terminal whose foreground process may
/// be an OpenSSH client. A Windows WSL profile stages otherwise-local images
/// inside its distribution; other local profiles retain the native shortcut.
pub(crate) struct SshImagePasteHandler {
    execution: SshExecution,
    cleanup: Arc<CleanupRegistry>,
}

impl SshImagePasteHandler {
    pub(crate) fn new<I>(shell: Shell, environment: I, working_directory: Option<PathBuf>) -> Self
    where
        I: IntoIterator<Item = (String, String)>,
    {
        Self {
            execution: SshExecution {
                shell,
                environment: environment.into_iter().collect(),
                working_directory,
            },
            cleanup: Arc::new(CleanupRegistry::default()),
        }
    }

    fn upload(&self, invocation: OpenSshInvocation, image: Vec<u8>) -> Result<String> {
        let platform = self.probe(&invocation)?;
        let sentinel = next_sentinel();
        let command = match &platform {
            RemotePlatform::Posix => posix_upload_command(&sentinel),
            RemotePlatform::PowerShell(executable) => {
                powershell_remote_command(executable, &powershell_upload_script(&sentinel))
            }
        };
        let output = self
            .execution
            .run(&invocation, command, image)
            .context("uploading clipboard image over SSH")?;
        let path = extract_remote_path(&output, &sentinel, &platform)?;
        let directory = remote_directory(&path).context("remote image path has no directory")?;
        let command = match &platform {
            RemotePlatform::Posix => posix_cleanup_command(&directory),
            RemotePlatform::PowerShell(executable) => {
                powershell_remote_command(executable, &powershell_cleanup_script(&directory))
            }
        };
        self.cleanup.push(CleanupEntry {
            launch: self.execution.launch_spec(&invocation, command),
            timeout: SSH_TRANSFER_TIMEOUT,
        });
        Ok(path)
    }

    fn probe(&self, invocation: &OpenSshInvocation) -> Result<RemotePlatform> {
        let posix_sentinel = next_sentinel();
        let posix_command = posix_probe_command(&posix_sentinel);
        let posix_error = match self.execution.run(invocation, posix_command, Vec::new()) {
            Ok(output) if posix_probe_succeeded(&output, &posix_sentinel) => {
                return Ok(RemotePlatform::Posix);
            }
            Ok(_) => "the POSIX probe returned no usable result".to_owned(),
            Err(error) => format!("POSIX probe failed: {error:#}"),
        };

        let mut powershell_error = None;
        for executable in ["powershell.exe", "pwsh.exe"] {
            let sentinel = next_sentinel();
            let command =
                powershell_remote_command(executable, &powershell_probe_script(&sentinel));
            match self.execution.run(invocation, command, Vec::new()) {
                Ok(output) if output_contains(&output, &sentinel) => {
                    return Ok(RemotePlatform::PowerShell(executable.to_owned()));
                }
                Ok(_) => {
                    powershell_error =
                        Some(format!("{executable} probe returned no usable result"));
                }
                Err(error) => {
                    powershell_error = Some(format!("{executable} probe failed: {error:#}"));
                }
            }
        }

        bail!(
            "could not determine the remote SSH platform; {posix_error}; {}",
            powershell_error.unwrap_or_else(|| "PowerShell is unavailable".to_owned())
        )
    }
}

impl ImagePasteHandler for SshImagePasteHandler {
    fn paste_image(
        &self,
        image: &Image,
        foreground_process: Option<&[String]>,
    ) -> Result<ImagePasteResult> {
        let invocation = foreground_process
            .and_then(foreground_invocation)
            .and_then(|invocation| self.execution.with_trusted_program(invocation));
        let Some(argv) = invocation else {
            #[cfg(any(windows, test))]
            if is_wsl_shell(&self.execution.shell) {
                return self
                    .stage_wsl_image(image)
                    .map(ImagePasteResult::ResolvedPath);
            }
            // The one failure this module has no way to report: keeping the
            // native chord is right for a genuinely local pane and useless for
            // anything else, and the two are indistinguishable from here. Say
            // which command line was declined so a session that pastes the
            // chord into a program on another host can be diagnosed in one run.
            log::debug!(
                "image paste kept the native shortcut for foreground process {foreground_process:?}"
            );
            return Ok(ImagePasteResult::UseNativeShortcut);
        };
        let image = normalize_image(image)?;
        self.upload(argv, image).map(ImagePasteResult::ResolvedPath)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SshExecution {
    shell: Shell,
    environment: HashMap<String, String>,
    working_directory: Option<PathBuf>,
}

impl SshExecution {
    /// `invocation` with the program that will actually run the upload,
    /// chosen here rather than taken from the command line it was parsed
    /// from. `None` when the reported program is not an `ssh` this module will
    /// start; the pane then gets whatever a non-SSH foreground process gets.
    fn with_trusted_program(&self, mut invocation: OpenSshInvocation) -> Option<OpenSshInvocation> {
        let Some(program) = self.trusted_ssh_program(&invocation.executable) else {
            log::debug!(
                "image paste declined the SSH executable {:?}",
                invocation.executable
            );
            return None;
        };
        invocation.executable = program;
        Some(invocation)
    }

    fn trusted_ssh_program(&self, reported: &str) -> Option<String> {
        #[cfg(any(windows, test))]
        if is_wsl_shell(&self.shell) {
            return posix_environment_ssh_program(reported);
        }
        #[cfg(windows)]
        if msys2_profile(&self.shell).is_some() || cygwin_profile(&self.shell).is_some() {
            // The launch spec starts the installation's own `ssh.exe` whatever
            // this returns; the reported name is only vetted.
            return posix_environment_ssh_program(reported);
        }
        native_ssh_program(reported, &self.environment)
    }

    fn run(
        &self,
        invocation: &OpenSshInvocation,
        remote_command: String,
        input: Vec<u8>,
    ) -> Result<Vec<u8>> {
        let launch = self.launch_spec(invocation, remote_command);
        run_image_paste_process(launch, input, SSH_TRANSFER_TIMEOUT)
    }

    fn launch_spec(&self, invocation: &OpenSshInvocation, remote_command: String) -> LaunchSpec {
        #[cfg(any(windows, test))]
        if is_wsl_shell(&self.shell) {
            return wsl::ssh_launch_spec(
                &self.environment,
                &self.shell,
                invocation,
                remote_command,
            );
        }
        #[cfg(windows)]
        {
            if let Some((root, _)) = msys2_profile(&self.shell) {
                return msys2_launch_spec(&self.environment, &root, invocation, remote_command);
            }
            if let Some((root, _)) = cygwin_profile(&self.shell) {
                return cygwin_launch_spec(&self.environment, &root, invocation, remote_command);
            }
        }

        LaunchSpec {
            program: invocation.executable.clone(),
            args: invocation.batch_args(remote_command),
            environment: self.environment.clone(),
            working_directory: self.working_directory.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LaunchSpec {
    program: String,
    args: Vec<String>,
    environment: HashMap<String, String>,
    working_directory: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OpenSshInvocation {
    executable: String,
    options: Vec<String>,
    end_options: bool,
    target: String,
}

impl OpenSshInvocation {
    fn batch_args(&self, remote_command: String) -> Vec<String> {
        let mut args = self.options.clone();
        args.extend([
            "-T".to_owned(),
            "-o".to_owned(),
            "BatchMode=yes".to_owned(),
            "-o".to_owned(),
            format!("ConnectTimeout={SSH_CONNECT_TIMEOUT_SECONDS}"),
            "-o".to_owned(),
            "RemoteCommand=none".to_owned(),
            "-o".to_owned(),
            "SessionType=default".to_owned(),
            "-o".to_owned(),
            "StdinNull=no".to_owned(),
        ]);
        if self.end_options {
            args.push("--".to_owned());
        }
        args.push(self.target.clone());
        args.push(remote_command);
        args
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum RemotePlatform {
    Posix,
    PowerShell(String),
}

#[derive(Default)]
struct CleanupRegistry {
    entries: Mutex<Vec<CleanupEntry>>,
}

impl CleanupRegistry {
    fn push(&self, entry: CleanupEntry) {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(entry);
    }
}

impl Drop for CleanupRegistry {
    fn drop(&mut self) {
        let entries = std::mem::take(
            &mut *self
                .entries
                .get_mut()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        if entries.is_empty() {
            return;
        }
        let result = thread::Builder::new()
            .name("image-paste-cleanup".to_owned())
            .spawn(move || {
                for entry in entries {
                    if let Err(error) =
                        run_image_paste_process(entry.launch, Vec::new(), entry.timeout)
                    {
                        log::debug!("could not remove staged image directory: {error:#}");
                    }
                }
            });
        if let Err(error) = result {
            log::debug!("could not start image cleanup: {error}");
        }
    }
}

struct CleanupEntry {
    launch: LaunchSpec,
    timeout: Duration,
}

/// The auxiliary SSH connection a foreground process's clipboard image has to
/// travel through, or `None` when this pane's process is local and the native
/// chord is the right answer.
fn foreground_invocation(argv: &[String]) -> Option<OpenSshInvocation> {
    if let Some(invocation) = foreground_ssh_argv(argv) {
        return Some(invocation);
    }
    #[cfg(feature = "zosh-client")]
    if let Some(invocation) = foreground_mosh_argv(argv) {
        return Some(invocation);
    }
    None
}

fn foreground_ssh_argv(argv: &[String]) -> Option<OpenSshInvocation> {
    parse_ssh_argv(&foreground_argv(argv, is_open_ssh)?)
}

/// Recovers the SSH bootstrap of a Mosh session from the launcher's own
/// argument vector.
///
/// Only the `--ssh` command and the target are taken. Everything else a Mosh
/// command line carries describes the UDP session — `-p`/`--port` names the
/// *server's* port range and would be a different option entirely to `ssh` —
/// and building the invocation from `--ssh` alone is what keeps them apart.
#[cfg(feature = "zosh-client")]
fn foreground_mosh_argv(argv: &[String]) -> Option<OpenSshInvocation> {
    let arguments = mosh_launcher_arguments(argv)?;
    let command = crate::mosh::parse_mosh_args(arguments.values()).ok()?;
    // `--local` never opens an SSH connection, and `--fake-proxy` is the
    // launcher re-entering itself as an SSH `ProxyCommand` rather than a
    // session anyone is typing into.
    if command.local || command.proxy.is_some() {
        return None;
    }
    let target = command.target?;
    // A reconstruction cannot tell an option's value from the next argument
    // where the original was quoted, and the first thing that misparse reaches
    // is the target. An SSH destination is never an assignment, so this is what
    // stops a lossy split from uploading somewhere nobody asked for.
    if arguments.is_reconstructed() && target.contains('=') {
        return None;
    }
    let mut ssh_argv = command.ssh;
    ssh_argv.push(target);
    parse_ssh_argv(&ssh_argv)
}

/// A Mosh launcher's arguments, and how much they can be trusted.
#[cfg(feature = "zosh-client")]
enum MoshLauncherArguments {
    /// Taken from a launcher that is still the pane's process, exactly as it
    /// was invoked.
    Exact(Vec<std::ffi::OsString>),
    /// Rebuilt from `mosh-client`'s display string, where quoting is lost.
    Reconstructed(Vec<std::ffi::OsString>),
}

#[cfg(feature = "zosh-client")]
impl MoshLauncherArguments {
    fn values(&self) -> &[std::ffi::OsString] {
        match self {
            Self::Exact(arguments) | Self::Reconstructed(arguments) => arguments,
        }
    }

    fn is_reconstructed(&self) -> bool {
        matches!(self, Self::Reconstructed(_))
    }
}

/// The Mosh command line behind a pane's foreground process.
///
/// Two shapes, because the launcher only survives in one of them. The bundled
/// `zosh` is launcher and client in one process and still has its own argument
/// vector. Upstream Mosh replaces its launcher with `mosh-client`, which keeps
/// the original command line only as the `ps` display string Mosh builds for
/// it: `-# <arguments> |`. Mosh joins those arguments with a space, so quoting
/// is gone for good and the result is a reconstruction rather than the command
/// line — see the caller for what that costs.
#[cfg(feature = "zosh-client")]
fn mosh_launcher_arguments(argv: &[String]) -> Option<MoshLauncherArguments> {
    if let Some(launcher) = foreground_argv(argv, is_mosh_launcher) {
        return Some(MoshLauncherArguments::Exact(
            launcher[1..]
                .iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>(),
        ));
    }
    let argv = foreground_argv(argv, is_mosh_client)?;
    let display = argv
        .get(1)?
        .strip_prefix("-#")?
        .strip_suffix('|')?
        .split_whitespace()
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>();
    (!display.is_empty()).then_some(MoshLauncherArguments::Reconstructed(display))
}

/// A foreground process's argument vector, accepting the single-string form a
/// shell reports a command line as.
fn foreground_argv(argv: &[String], is_launcher: fn(&str) -> bool) -> Option<Vec<String>> {
    if argv.first().is_some_and(|program| is_launcher(program)) {
        return Some(argv.to_vec());
    }
    if argv.len() == 1 {
        let words = parse_shell_command(&argv[0]).ok()?;
        return is_launcher(words.first()?).then_some(words);
    }
    None
}

fn is_open_ssh(program: &str) -> bool {
    program_is_named(program, &["ssh", "ssh.exe"])
}

/// The `ssh` a WSL, MSYS2 or Cygwin session reported, if it is that
/// environment's own: the bare name, which the environment's `PATH` resolves
/// without the current directory, or the conventional `/usr/bin/ssh` or
/// `/bin/ssh`. Any other path is refused, because the report is title text and
/// the path in it could name anything.
#[cfg(any(windows, test))]
fn posix_environment_ssh_program(reported: &str) -> Option<String> {
    let program = reported
        .len()
        .checked_sub(".exe".len())
        .filter(|&end| {
            reported.is_char_boundary(end) && reported[end..].eq_ignore_ascii_case(".exe")
        })
        .map_or(reported, |end| &reported[..end]);
    matches!(program, "ssh" | "/usr/bin/ssh" | "/bin/ssh").then(|| program.to_owned())
}

/// The native `ssh` to start: the one a search of the system directories and
/// the absolute `PATH` entries finds, never the pane's working directory.
/// A reported bare name selects it; a reported path is accepted only when it
/// is that same executable, so an `ssh` elsewhere — in a project checkout, in
/// a directory something else wrote — is never run on a paste.
fn native_ssh_program(reported: &str, environment: &HashMap<String, String>) -> Option<String> {
    let resolved = resolve_native_ssh(environment)?;
    if reported.contains(['/', '\\', ':']) {
        let reported = std::path::Path::new(reported);
        if !reported.is_absolute() {
            return None;
        }
        let same = std::fs::canonicalize(reported)
            .ok()
            .zip(std::fs::canonicalize(&resolved).ok())
            .is_some_and(|(reported, resolved)| reported == resolved);
        if !same {
            return None;
        }
    }
    Some(resolved.to_string_lossy().into_owned())
}

#[cfg(windows)]
fn resolve_native_ssh(environment: &HashMap<String, String>) -> Option<PathBuf> {
    let path = environment
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("PATH"))
        .map(|(_, value)| std::ffi::OsString::from(value))
        .or_else(|| std::env::var_os("PATH"));
    terminal::resolve_application("ssh.exe", path)
        .ok()
        .flatten()
}

#[cfg(unix)]
fn resolve_native_ssh(environment: &HashMap<String, String>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt as _;

    let path = environment
        .get("PATH")
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("PATH"))?;
    std::env::split_paths(&path)
        // An empty or relative entry is the current directory.
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join("ssh"))
        .find(|candidate| {
            std::fs::metadata(candidate).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
}

/// Whether a pane's foreground process is a Mosh session: the bundled `zosh`,
/// an upstream `mosh` launcher, or the `mosh-client` either one runs.
///
/// Project detection asks this, not image paste. An OpenSSH session needs no
/// such check, because the remote shell's own `zetta-cwd:` reports pass
/// through it and move the pane out of any local project. A Mosh client
/// redraws the remote screen and sends the remote title with a `[mosh] `
/// prefix, so those reports never arrive and the pane would keep the local
/// directory it had before the session started. This is not gated on
/// `zosh-client`: an upstream mosh run in a pane has the same problem.
pub(crate) fn foreground_is_mosh_session(argv: &[String]) -> bool {
    foreground_argv(argv, |program| {
        is_mosh_launcher(program) || is_mosh_client(program)
    })
    .is_some()
}

/// The bundled `zosh` launcher, and a `mosh` that has not yet replaced itself
/// with its client.
fn is_mosh_launcher(program: &str) -> bool {
    program_is_named(program, &["zosh", "zosh.exe", "mosh", "mosh.exe"])
}

/// What upstream Mosh's launcher becomes, and what `zosh --client` runs.
fn is_mosh_client(program: &str) -> bool {
    program_is_named(program, &["mosh-client", "mosh-client.exe"])
}

fn program_is_named(program: &str, names: &[&str]) -> bool {
    program
        .rsplit(['/', '\\'])
        .next()
        .is_some_and(|program| names.iter().any(|name| program.eq_ignore_ascii_case(name)))
}

/// Rebuilds the auxiliary connection from a reported `ssh` command line.
///
/// The command line is untrusted input even when it is authentic — on a
/// WSL/MSYS2/Cygwin pane it arrives as printed title text — so nothing in it
/// is passed through as written. Every option is normalized, and only the
/// ones on [`short_option`]'s and [`config_option`]'s allowlists survive.
/// Anything that could run a command, load code, read another configuration
/// or write a file rejects the whole invocation, which leaves the native
/// paste chord in place.
fn parse_ssh_argv(argv: &[String]) -> Option<OpenSshInvocation> {
    let executable = argv.first()?.clone();
    if !is_open_ssh(&executable) {
        return None;
    }

    let mut options = Vec::new();
    let mut end_options = false;
    let mut index = 1;
    while let Some(argument) = argv.get(index) {
        if end_options {
            if argument.is_empty() {
                return None;
            }
            return Some(OpenSshInvocation {
                executable,
                options,
                end_options,
                target: argument.clone(),
            });
        }
        if argument == "--" {
            end_options = true;
            index += 1;
            continue;
        }
        if !argument.starts_with('-') || argument == "-" {
            if argument.is_empty() {
                return None;
            }
            return Some(OpenSshInvocation {
                executable,
                options,
                end_options,
                target: argument.clone(),
            });
        }

        index += parse_ssh_option_group(argv, index, &mut options)?;
    }
    None
}

/// Reads the short-option group at `argv[index]` — `-v`, `-vvi key`,
/// `-p2222`, `-oKey=value` — appending what survives to `options` one option
/// at a time, and returns how many arguments it consumed.
///
/// `None` rejects the invocation: an option off the allowlist, a long option,
/// or a value that is missing.
fn parse_ssh_option_group(
    argv: &[String],
    index: usize,
    options: &mut Vec<String>,
) -> Option<usize> {
    let group = argv.get(index)?.strip_prefix('-')?;
    if group.is_empty() || group.starts_with('-') {
        return None;
    }
    for (offset, letter) in group.char_indices() {
        let kind = short_option(letter);
        let takes_value = matches!(
            kind,
            ShortOption::KeepValue | ShortOption::DropValue | ShortOption::Config
        );
        if !takes_value {
            match kind {
                ShortOption::Keep => options.push(format!("-{letter}")),
                ShortOption::Drop => {}
                _ => return None,
            }
            continue;
        }
        // The rest of the group is the value, as `getopt` reads it; only when
        // nothing is left is it the next argument. Either way it is opaque:
        // `-Llocalhost:22:...` is not a run of option letters.
        let attached = &group[offset + letter.len_utf8()..];
        let (value, consumed) = if attached.is_empty() {
            (argv.get(index + 1)?.as_str(), 2)
        } else {
            (attached, 1)
        };
        if value.chars().any(char::is_control) {
            return None;
        }
        match kind {
            ShortOption::KeepValue => options.extend([format!("-{letter}"), value.to_owned()]),
            ShortOption::DropValue => {}
            ShortOption::Config => {
                if let Some(option) = config_option(value)? {
                    options.extend(["-o".to_owned(), option]);
                }
            }
            _ => unreachable!("only value-taking options reach here"),
        }
        return Some(consumed);
    }
    Some(1)
}

/// What a short `ssh` option means for the auxiliary connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShortOption {
    /// A flag that may change how the connection is made; kept.
    Keep,
    /// A flag only the interactive session needs; dropped.
    Drop,
    /// An option whose value is kept: target, port, user, identity, jump
    /// host and the like.
    KeepValue,
    /// An option whose value only the interactive session needs, such as a
    /// port forward the session already holds; dropped with its value.
    DropValue,
    /// `-o`, whose value goes through [`config_option`].
    Config,
    /// Everything else. Covers the options that would run a command or load
    /// code (`-F`, `-I`, `-w`), write a file (`-E`), or turn the connection
    /// into something that cannot carry an upload on stdin (`-N`, `-W`, `-s`).
    Reject,
}

fn short_option(letter: char) -> ShortOption {
    match letter {
        '4' | '6' | 'C' | 'K' | 'k' | 'q' | 'v' => ShortOption::Keep,
        'A' | 'a' | 'g' | 'M' | 'T' | 't' | 'X' | 'x' | 'Y' => ShortOption::Drop,
        'B' | 'b' | 'c' | 'i' | 'J' | 'l' | 'm' | 'p' | 'S' => ShortOption::KeepValue,
        'D' | 'e' | 'L' | 'R' => ShortOption::DropValue,
        'o' => ShortOption::Config,
        _ => ShortOption::Reject,
    }
}

/// `ssh_config` keywords the auxiliary connection keeps from the session's
/// command line, lowercased. They choose where and how to connect and
/// authenticate; none of them runs a program or reads another configuration.
const KEPT_CONFIG_OPTIONS: &[&str] = &[
    "addressfamily",
    "bindaddress",
    "bindinterface",
    "canonicaldomains",
    "canonicalizefallbacklocal",
    "canonicalizehostname",
    "canonicalizemaxdots",
    "canonicalizepermittedcnames",
    "certificatefile",
    "challengeresponseauthentication",
    "checkhostip",
    "ciphers",
    "compression",
    "connectionattempts",
    "controlpath",
    "fingerprinthash",
    "globalknownhostsfile",
    "gssapiauthentication",
    "gssapidelegatecredentials",
    "hashknownhosts",
    "hostbasedacceptedalgorithms",
    "hostbasedauthentication",
    "hostkeyalgorithms",
    "hostkeyalias",
    "hostname",
    "identitiesonly",
    "identityagent",
    "identityfile",
    "ipqos",
    "kbdinteractiveauthentication",
    "kbdinteractivedevices",
    "kexalgorithms",
    "loglevel",
    "macs",
    "nohostauthenticationforlocalhost",
    "numberofpasswordprompts",
    "passwordauthentication",
    "port",
    "preferredauthentications",
    "proxyjump",
    "pubkeyacceptedalgorithms",
    "pubkeyacceptedkeytypes",
    "pubkeyauthentication",
    "rekeylimit",
    "requiredrsasize",
    "serveralivecountmax",
    "serveraliveinterval",
    "stricthostkeychecking",
    "tcpkeepalive",
    "updatehostkeys",
    "user",
    "userknownhostsfile",
    "verifyhostkeydns",
];

/// `ssh_config` keywords that shape only the interactive session, or that the
/// auxiliary connection sets itself in [`OpenSshInvocation::batch_args`];
/// dropped. `ControlMaster`/`ControlPersist` are here so an upload can reuse a
/// session's master through `ControlPath` but never becomes one.
const DROPPED_CONFIG_OPTIONS: &[&str] = &[
    "addkeystoagent",
    "batchmode",
    "clearallforwardings",
    "connecttimeout",
    "controlmaster",
    "controlpersist",
    "dynamicforward",
    "escapechar",
    "exitonforwardfailure",
    "forwardagent",
    "forwardx11",
    "forwardx11timeout",
    "forwardx11trusted",
    "gatewayports",
    "localforward",
    "remotecommand",
    "remoteforward",
    "requesttty",
    "sendenv",
    "setenv",
    "streamlocalbindmask",
    "streamlocalbindunlink",
    "visualhostkey",
];

/// Normalizes one `-o` value to `Key=value` and applies the allowlist.
///
/// Returns `Some(Some(_))` to keep the option, `Some(None)` to drop it, and
/// `None` to reject the invocation. The keyword is split off the way `ssh` reads a
/// configuration line — leading blanks, then up to a blank or `=`, then one
/// `=` with blanks either side — so `-o ProxyCommand=x`, `-oProxyCommand x`
/// and `-o " proxycommand = x"` all reach the same decision. A keyword that is
/// not plain ASCII letters and digits (a quoted one, say) is rejected, as is
/// every keyword on neither list: `ProxyCommand`, `LocalCommand`,
/// `PermitLocalCommand`, `KnownHostsCommand`, `Include`, `PKCS11Provider`,
/// `SecurityKeyProvider` and whatever a future OpenSSH adds.
fn config_option(value: &str) -> Option<Option<String>> {
    const BLANK: [char; 4] = [' ', '\t', '\r', '\n'];
    let line = value.trim_start_matches(BLANK);
    let key_end = line.find(|character: char| BLANK.contains(&character) || character == '=');
    let (key, rest) = line.split_at(key_end.unwrap_or(line.len()));
    if key.is_empty() || !key.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return None;
    }
    let rest = rest.trim_start_matches(BLANK);
    let argument = rest.strip_prefix('=').unwrap_or(rest).trim_matches(BLANK);
    let name = key.to_ascii_lowercase();

    match name.as_str() {
        "stdinnull" => {
            return (!argument.eq_ignore_ascii_case("yes")).then_some(None);
        }
        "sessiontype" => {
            let session_type = argument.to_ascii_lowercase();
            return (!matches!(session_type.as_str(), "none" | "subsystem")).then_some(None);
        }
        _ => {}
    }
    if DROPPED_CONFIG_OPTIONS.contains(&name.as_str()) {
        return Some(None);
    }
    if !KEPT_CONFIG_OPTIONS.contains(&name.as_str()) || argument.is_empty() {
        return None;
    }
    Some(Some(format!("{key}={argument}")))
}

fn parse_shell_command(command: &str) -> Result<Vec<String>> {
    anyhow::ensure!(command.len() <= 32 * 1024, "foreground command is too long");
    let mut words = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quote = None;
    let mut escaped = false;

    for character in command.chars() {
        if character.is_control() {
            bail!("foreground command contains control characters");
        }
        if escaped {
            word.push(character);
            started = true;
            escaped = false;
            continue;
        }
        match quote {
            Some('\'') => {
                if character == '\'' {
                    quote = None;
                } else {
                    word.push(character);
                }
            }
            Some('"') => match character {
                '"' => quote = None,
                '\\' => escaped = true,
                '$' | '`' => bail!("foreground command contains an expansion"),
                _ => word.push(character),
            },
            Some(_) => unreachable!("only single and double quotes are tracked"),
            None => match character {
                '\'' | '"' => {
                    quote = Some(character);
                    started = true;
                }
                '\\' => {
                    escaped = true;
                    started = true;
                }
                character if character.is_whitespace() => {
                    if started {
                        words.push(std::mem::take(&mut word));
                        started = false;
                    }
                }
                '$' | '`' | ';' | '|' | '&' | '<' | '>' | '(' | ')' | '{' | '}' | '~' | '*'
                | '?' | '[' | ']' => bail!("foreground command contains shell syntax"),
                _ => {
                    word.push(character);
                    started = true;
                }
            },
        }
    }
    anyhow::ensure!(
        quote.is_none() && !escaped,
        "foreground command has an unfinished quote"
    );
    if started {
        words.push(word);
    }
    anyhow::ensure!(!words.is_empty(), "foreground command is empty");
    Ok(words)
}

fn next_sentinel() -> String {
    let id = NEXT_SENTINEL_ID.fetch_add(1, Ordering::Relaxed);
    format!("__ZETTA_IMAGE_{:016x}_{}__", id, std::process::id())
}

fn posix_probe_command(sentinel: &str) -> String {
    format!(
        "command -v uname >/dev/null 2>&1 && printf '%s%s%s\\n' '{sentinel}' \"$(uname -s)\" '{sentinel}'"
    )
}

fn posix_probe_succeeded(output: &[u8], sentinel: &str) -> bool {
    let Some(value) = delimited_value(output, sentinel) else {
        return false;
    };
    !value.trim().is_empty()
}

fn posix_upload_command(sentinel: &str) -> String {
    format!(
        "set -eu; umask 077; base=\"${{TMPDIR:-/tmp}}\"; case \"$base\" in /*) ;; *) base=/tmp ;; esac; directory=\"$(mktemp -d \"$base/zetta-image.XXXXXXXX\")\"; chmod 700 \"$directory\"; image_path=\"$directory/{IMAGE_FILE_NAME}\"; cat >\"$image_path\"; printf '%s%s%s\\n' '{sentinel}' \"$image_path\" '{sentinel}'"
    )
}

fn posix_cleanup_command(directory: &str) -> String {
    format!("rm -rf -- {}", posix_quote(directory))
}

fn posix_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn powershell_probe_script(sentinel: &str) -> String {
    format!("$ErrorActionPreference='Stop';[Console]::Out.WriteLine('{sentinel}')")
}

fn powershell_upload_script(sentinel: &str) -> String {
    let sentinel = powershell_quote(sentinel);
    format!(
        "$ErrorActionPreference='Stop';$root=[IO.Path]::GetTempPath();$directory=Join-Path $root ('zetta-image-'+[Guid]::NewGuid().ToString('N'));New-Item -ItemType Directory -Path $directory | Out-Null;$acl=Get-Acl -LiteralPath $directory;$acl.SetAccessRuleProtection($true,$false);foreach($entry in @($acl.Access)){{[void]$acl.RemoveAccessRule($entry)}};$identity=[Security.Principal.WindowsIdentity]::GetCurrent().Name;$rule=New-Object Security.AccessControl.FileSystemAccessRule($identity,'FullControl','ContainerInherit,ObjectInherit','None','Allow');[void]$acl.AddAccessRule($rule);Set-Acl -LiteralPath $directory -AclObject $acl;$path=Join-Path $directory '{IMAGE_FILE_NAME}';$input=[Console]::OpenStandardInput();$output=[IO.File]::Open($path,[IO.FileMode]::Create,[IO.FileAccess]::Write,[IO.FileShare]::None);try{{$input.CopyTo($output)}}finally{{$output.Dispose();$input.Dispose()}};[Console]::Out.WriteLine({sentinel}+$path+{sentinel})"
    )
}

fn powershell_cleanup_script(directory: &str) -> String {
    format!(
        "$ErrorActionPreference='SilentlyContinue';Remove-Item -LiteralPath {} -Recurse -Force -ErrorAction SilentlyContinue",
        powershell_quote(directory)
    )
}

fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn powershell_remote_command(executable: &str, script: &str) -> String {
    let encoded = BASE64.encode(
        script
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    format!("{executable} -NoLogo -NoProfile -NonInteractive -EncodedCommand {encoded}")
}

fn extract_remote_path(output: &[u8], sentinel: &str, platform: &RemotePlatform) -> Result<String> {
    let path = delimited_value(output, sentinel).context("SSH upload returned no image path")?;
    validate_remote_path(path.trim(), platform)
}

fn delimited_value<'a>(output: &'a [u8], sentinel: &str) -> Option<&'a str> {
    let output = std::str::from_utf8(output).ok()?;
    let start = output.find(sentinel)? + sentinel.len();
    let end = output[start..].find(sentinel)? + start;
    Some(&output[start..end])
}

fn output_contains(output: &[u8], value: &str) -> bool {
    std::str::from_utf8(output).is_ok_and(|output| output.contains(value))
}

fn validate_remote_path(path: &str, platform: &RemotePlatform) -> Result<String> {
    anyhow::ensure!(
        !path.is_empty() && path.len() <= MAX_REMOTE_PATH_BYTES,
        "remote image path is empty or too long"
    );
    anyhow::ensure!(
        !path.chars().any(char::is_control) && !path.contains(['*', '?']),
        "remote image path contains invalid characters"
    );
    anyhow::ensure!(
        path.rsplit(['/', '\\']).next() == Some(IMAGE_FILE_NAME),
        "remote image path does not name a PNG file"
    );
    match platform {
        RemotePlatform::Posix => {
            anyhow::ensure!(path.starts_with('/'), "POSIX image path is not absolute");
        }
        RemotePlatform::PowerShell(_) => {
            let windows_absolute = path.starts_with(r"\\")
                || path.len() >= 3
                    && path.as_bytes()[0].is_ascii_alphabetic()
                    && path.as_bytes()[1] == b':'
                    && matches!(path.as_bytes()[2], b'/' | b'\\');
            anyhow::ensure!(windows_absolute, "Windows image path is not absolute");
        }
    }
    for component in path.split(['/', '\\']) {
        anyhow::ensure!(
            component != "." && component != "..",
            "remote image path escapes its directory"
        );
    }
    Ok(path.to_owned())
}

fn remote_directory(path: &str) -> Option<String> {
    let separator = path.rfind(['/', '\\'])?;
    (separator > 0).then(|| path[..separator].to_owned())
}

fn run_image_paste_process(spec: LaunchSpec, input: Vec<u8>, timeout: Duration) -> Result<Vec<u8>> {
    let deadline = Instant::now() + timeout;
    let mut command = util::command::new_std_command(&spec.program);
    command
        .args(&spec.args)
        .envs(&spec.environment)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(directory) = &spec.working_directory {
        command.current_dir(directory);
    }
    let mut child = loop {
        match command.spawn() {
            Ok(process) => break process,
            Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                if Instant::now() >= deadline {
                    bail!("starting image-paste process timed out after {timeout:?}");
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("starting image-paste process {}", spec.program));
            }
        }
    };
    let mut stdout = child
        .stdout
        .take()
        .context("image-paste stdout is unavailable")?;
    let reader = thread::Builder::new()
        .name("image-paste-reader".to_owned())
        .spawn(move || {
            let mut output = Vec::new();
            let result = stdout.read_to_end(&mut output);
            (result, output)
        })
        .context("starting image-paste reader")?;
    let mut stdin = child
        .stdin
        .take()
        .context("image-paste stdin is unavailable")?;
    let writer = thread::Builder::new()
        .name("image-paste-writer".to_owned())
        .spawn(move || stdin.write_all(&input))
        .context("starting image-paste writer")?;

    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .context("waiting for image-paste process")?
        {
            break status;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            child.kill().ok();
            break child
                .wait()
                .context("stopping timed-out image-paste process")?;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let write_result = writer
        .join()
        .map_err(|_| anyhow::anyhow!("image-paste writer panicked"))?;
    let (read_result, output) = reader
        .join()
        .map_err(|_| anyhow::anyhow!("image-paste reader panicked"))?;
    read_result.context("reading image-paste output")?;
    if timed_out {
        bail!("image-paste transfer timed out after {timeout:?}");
    }
    write_result.context("sending clipboard image to image-paste process")?;
    anyhow::ensure!(
        status.success(),
        "image-paste process exited with status {status}"
    );
    Ok(output)
}

#[cfg(windows)]
fn msys2_launch_spec(
    environment: &HashMap<String, String>,
    root: &Path,
    invocation: &OpenSshInvocation,
    remote_command: String,
) -> LaunchSpec {
    let mut environment = environment.clone();
    prepend_windows_path(
        &mut environment,
        &[root.join("usr").join("bin"), root.join("bin")],
    );
    let program = root.join("usr").join("bin").join("ssh.exe");
    LaunchSpec {
        program: program.to_string_lossy().into_owned(),
        args: invocation.batch_args(remote_command),
        environment,
        working_directory: None,
    }
}

#[cfg(windows)]
fn cygwin_launch_spec(
    environment: &HashMap<String, String>,
    root: &Path,
    invocation: &OpenSshInvocation,
    remote_command: String,
) -> LaunchSpec {
    let mut environment = environment.clone();
    prepend_windows_path(&mut environment, &[root.join("bin")]);
    environment.insert("CHERE_INVOKING".to_owned(), "1".to_owned());
    environment.insert(
        "ZETTA_CYGWIN_ROOT".to_owned(),
        root.to_string_lossy().into_owned(),
    );
    let program = root.join("bin").join("ssh.exe");
    LaunchSpec {
        program: program.to_string_lossy().into_owned(),
        args: invocation.batch_args(remote_command),
        environment,
        working_directory: None,
    }
}

#[cfg(windows)]
fn prepend_windows_path(environment: &mut HashMap<String, String>, prefixes: &[PathBuf]) {
    let mut paths = prefixes.to_vec();
    if let Some(existing) = environment
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("PATH"))
        .map(|(_, value)| value.clone())
    {
        paths.extend(std::env::split_paths(&existing));
    }
    if let Ok(path) = std::env::join_paths(paths) {
        environment.insert("PATH".to_owned(), path.to_string_lossy().into_owned());
    }
}

#[cfg(test)]
#[path = "tests/ssh_image_paste/mod.rs"]
mod tests;
