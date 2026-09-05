//! Image paste for a foreground OpenSSH process.
//!
//! A local terminal normally sends the native image-paste chord. When the
//! foreground process is OpenSSH, the remote application cannot read the
//! desktop clipboard, so this module sends a PNG through a second, batch-mode
//! SSH connection and pastes the resulting remote path instead.

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
use crate::{cygwin_profile, is_wsl_shell, msys2_profile};

#[cfg(windows)]
use std::path::Path;

const SSH_CONNECT_TIMEOUT_SECONDS: u64 = 15;
const SSH_TRANSFER_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_REMOTE_PATH_BYTES: usize = 4096;
const IMAGE_FILE_NAME: &str = "image.png";

static NEXT_SENTINEL_ID: AtomicU64 = AtomicU64::new(1);

/// Resolves clipboard images for a local terminal whose foreground process may
/// be an OpenSSH client. Unsupported foreground processes return the native
/// shortcut so they retain the ordinary local-paste behavior.
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
        self.cleanup.push(CleanupEntry {
            execution: self.execution.clone(),
            invocation,
            platform,
            directory,
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
        let Some(argv) = foreground_process.and_then(foreground_ssh_argv) else {
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
    fn run(
        &self,
        invocation: &OpenSshInvocation,
        remote_command: String,
        input: Vec<u8>,
    ) -> Result<Vec<u8>> {
        let launch = self.launch_spec(invocation, remote_command);
        run_ssh_process(launch, input, SSH_TRANSFER_TIMEOUT)
    }

    fn launch_spec(&self, invocation: &OpenSshInvocation, remote_command: String) -> LaunchSpec {
        #[cfg(windows)]
        {
            if is_wsl_shell(&self.shell) {
                return wsl_launch_spec(&self.environment, &self.shell, invocation, remote_command);
            }
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
            .name("ssh-image-paste-cleanup".to_owned())
            .spawn(move || {
                for entry in entries {
                    let command = match &entry.platform {
                        RemotePlatform::Posix => posix_cleanup_command(&entry.directory),
                        RemotePlatform::PowerShell(executable) => powershell_remote_command(
                            executable,
                            &powershell_cleanup_script(&entry.directory),
                        ),
                    };
                    if let Err(error) = entry.execution.run(&entry.invocation, command, Vec::new())
                    {
                        log::debug!("could not remove remote image directory: {error:#}");
                    }
                }
            });
        if let Err(error) = result {
            log::debug!("could not start remote image cleanup: {error}");
        }
    }
}

struct CleanupEntry {
    execution: SshExecution,
    invocation: OpenSshInvocation,
    platform: RemotePlatform,
    directory: String,
}

fn foreground_ssh_argv(argv: &[String]) -> Option<OpenSshInvocation> {
    let argv = if argv.first().is_some_and(|program| is_open_ssh(program)) {
        argv.to_vec()
    } else if argv.len() == 1 {
        parse_shell_command(&argv[0]).ok()?
    } else {
        return None;
    };
    parse_ssh_argv(&argv)
}

fn is_open_ssh(program: &str) -> bool {
    program.rsplit(['/', '\\']).next().is_some_and(|name| {
        name.eq_ignore_ascii_case("ssh") || name.eq_ignore_ascii_case("ssh.exe")
    })
}

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

        let (action, consumed) = parse_ssh_option(argv, index)?;
        match action {
            SshOptionAction::Keep(arguments) => options.extend(arguments),
            SshOptionAction::Drop => {}
            SshOptionAction::Reject => return None,
        }
        index += consumed;
    }
    None
}

enum SshOptionAction {
    Keep(Vec<String>),
    Drop,
    Reject,
}

fn parse_ssh_option(argv: &[String], index: usize) -> Option<(SshOptionAction, usize)> {
    let argument = argv.get(index)?;
    let option = argument.strip_prefix('-')?;
    if option.is_empty() {
        return None;
    }

    if option.starts_with('-') {
        return Some((SshOptionAction::Reject, 1));
    }
    if option.starts_with('o') {
        return parse_o_option(argv, index, option.strip_prefix('o').unwrap_or_default());
    }

    let mut saw_tty_flag = false;
    let mut saw_other_flag = false;
    for (offset, option_name) in option.char_indices() {
        if matches!(option_name, 't' | 'T') {
            saw_tty_flag = true;
            continue;
        }
        if matches!(
            option_name,
            'n' | 'N' | 'f' | 'W' | 'O' | 'Q' | 'G' | 'V' | 's'
        ) {
            return Some((SshOptionAction::Reject, 1));
        }

        saw_other_flag = true;
        if short_option_takes_value(option_name) {
            if saw_tty_flag {
                return Some((SshOptionAction::Reject, 1));
            }
            let value_start = offset + option_name.len_utf8();
            if value_start < option.len() {
                // The rest of a short-option group is the value. Do not
                // inspect it for option letters: `-Llocalhost:22:...` and
                // `-i~/.ssh/...` are valid and their values are opaque.
                return Some((SshOptionAction::Keep(vec![argument.clone()]), 1));
            }
            let value = argv.get(index + 1)?.clone();
            return Some((SshOptionAction::Keep(vec![argument.clone(), value]), 2));
        }
    }
    if saw_tty_flag {
        if saw_other_flag {
            return Some((SshOptionAction::Reject, 1));
        }
        return Some((SshOptionAction::Drop, 1));
    }
    Some((SshOptionAction::Keep(vec![argument.clone()]), 1))
}

fn parse_o_option(
    argv: &[String],
    index: usize,
    attached: &str,
) -> Option<(SshOptionAction, usize)> {
    if attached.is_empty() {
        let value = argv.get(index + 1)?.clone();
        return parse_o_value(&value).map(|action| (action, 2));
    }
    parse_o_value(attached).map(|action| (action, 1))
}

fn parse_o_value(value: &str) -> Option<SshOptionAction> {
    let name = value.split_once('=').map_or(value, |(name, _)| name);
    if name.eq_ignore_ascii_case("requesttty")
        || name.eq_ignore_ascii_case("remotecommand")
        || name.eq_ignore_ascii_case("batchmode")
        || name.eq_ignore_ascii_case("connecttimeout")
    {
        return Some(SshOptionAction::Drop);
    }
    if name.eq_ignore_ascii_case("stdinnull") {
        let enabled = value
            .split_once('=')
            .is_some_and(|(_, value)| value.eq_ignore_ascii_case("yes"));
        return Some(if enabled {
            SshOptionAction::Reject
        } else {
            SshOptionAction::Drop
        });
    }
    if name.eq_ignore_ascii_case("sessiontype") {
        let session_type = value
            .split_once('=')
            .map_or("default", |(_, value)| value)
            .to_ascii_lowercase();
        if matches!(session_type.as_str(), "none" | "subsystem") {
            return Some(SshOptionAction::Reject);
        }
        return Some(SshOptionAction::Drop);
    }
    if name.is_empty() {
        return Some(SshOptionAction::Reject);
    }
    Some(SshOptionAction::Keep(vec![
        "-o".to_owned(),
        value.to_owned(),
    ]))
}

fn short_option_takes_value(option: char) -> bool {
    matches!(
        option,
        'B' | 'b'
            | 'c'
            | 'D'
            | 'E'
            | 'e'
            | 'F'
            | 'I'
            | 'i'
            | 'J'
            | 'L'
            | 'l'
            | 'm'
            | 'p'
            | 'R'
            | 'S'
            | 'w'
    )
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

fn run_ssh_process(spec: LaunchSpec, input: Vec<u8>, timeout: Duration) -> Result<Vec<u8>> {
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
    let mut child = command
        .spawn()
        .with_context(|| format!("starting auxiliary SSH client {}", spec.program))?;
    let mut stdout = child
        .stdout
        .take()
        .context("auxiliary SSH stdout is unavailable")?;
    let reader = thread::Builder::new()
        .name("ssh-image-paste-reader".to_owned())
        .spawn(move || {
            let mut output = Vec::new();
            let result = stdout.read_to_end(&mut output);
            (result, output)
        })
        .context("starting auxiliary SSH reader")?;
    let mut stdin = child
        .stdin
        .take()
        .context("auxiliary SSH stdin is unavailable")?;
    let writer = thread::Builder::new()
        .name("ssh-image-paste-writer".to_owned())
        .spawn(move || stdin.write_all(&input))
        .context("starting auxiliary SSH writer")?;

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .context("waiting for auxiliary SSH client")?
        {
            break status;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            child.kill().ok();
            break child
                .wait()
                .context("stopping timed-out auxiliary SSH client")?;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let write_result = writer
        .join()
        .map_err(|_| anyhow::anyhow!("auxiliary SSH writer panicked"))?;
    let (read_result, output) = reader
        .join()
        .map_err(|_| anyhow::anyhow!("auxiliary SSH reader panicked"))?;
    read_result.context("reading auxiliary SSH output")?;
    if timed_out {
        bail!("auxiliary SSH transfer timed out after {timeout:?}");
    }
    write_result.context("sending clipboard image to auxiliary SSH")?;
    anyhow::ensure!(
        status.success(),
        "auxiliary SSH client exited with status {status}"
    );
    Ok(output)
}

#[cfg(windows)]
fn wsl_launch_spec(
    environment: &HashMap<String, String>,
    shell: &Shell,
    invocation: &OpenSshInvocation,
    remote_command: String,
) -> LaunchSpec {
    let (program, shell_args) = shell.program_and_args();
    let exec_index = shell_args.iter().position(|argument| {
        argument.eq_ignore_ascii_case("--exec") || argument.eq_ignore_ascii_case("-e")
    });
    let mut args = shell_args[..exec_index.unwrap_or(shell_args.len())].to_vec();
    args.push("--exec".to_owned());
    args.push(invocation.executable.clone());
    args.extend(invocation.batch_args(remote_command));
    LaunchSpec {
        program,
        args,
        environment: environment.clone(),
        working_directory: None,
    }
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
#[path = "tests/ssh_image_paste.rs"]
mod tests;
