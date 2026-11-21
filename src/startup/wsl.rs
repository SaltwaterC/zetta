//! Shells that run under another layer on Windows: the environment WSL is given
//! through `WSLENV`, and the MSYS2 and Cygwin profiles, paths and directory
//! tracking.

use super::*;
use std::{ffi::OsStr, sync::LazyLock};

#[cfg(windows)]
mod cygwin_msys;
#[cfg(windows)]
pub(crate) use cygwin_msys::*;

pub(crate) fn is_wsl_shell(shell: &Shell) -> bool {
    let program = match shell {
        Shell::System => return false,
        Shell::Program(program) | Shell::WithArguments { program, .. } => program,
    };
    program.rsplit(['/', '\\']).next().is_some_and(|name| {
        name.eq_ignore_ascii_case("wsl.exe") || (cfg!(windows) && name.eq_ignore_ascii_case("wsl"))
    })
}

fn add_wslenv_entry(wslenv: &mut String, variable: &str) {
    let name = variable.split('/').next().unwrap();
    if wslenv
        .split(':')
        .any(|entry| entry.split('/').next() == Some(name))
    {
        return;
    }
    if !wslenv.is_empty() {
        wslenv.push(':');
    }
    wslenv.push_str(variable);
}

fn set_wslenv_entry(wslenv: &mut String, variable: &str) {
    let name = variable.split('/').next().unwrap();
    let inherited = std::mem::take(wslenv);
    let mut variable_seen = false;

    for entry in inherited.split(':') {
        if entry.split('/').next() == Some(name) {
            if !variable_seen {
                add_wslenv_entry(wslenv, variable);
                variable_seen = true;
            }
        } else if !entry.is_empty() {
            if !wslenv.is_empty() {
                wslenv.push(':');
            }
            wslenv.push_str(entry);
        }
    }

    if !variable_seen {
        add_wslenv_entry(wslenv, variable);
    }
}

pub(crate) fn add_wsl_environment_variables<S>(environment: &mut HashMap<String, String, S>)
where
    S: std::hash::BuildHasher,
{
    let mut wslenv = environment
        .remove("WSLENV")
        .or_else(|| env::var("WSLENV").ok())
        .unwrap_or_default();

    // Host CLI requests must carry the current pane's IDs back to Windows.
    // /u alone leaves Windows children with the WSL launcher's inherited IDs,
    // which can belong to a different tab or an older Zetta process.
    for variable in [
        "ZETTA_PROCESS_ID",
        "ZETTA_ATTENTION_ID",
        "ZETTA_PANE_ID",
        "ZETTA_PANE_ROUTING_ID",
    ] {
        set_wslenv_entry(&mut wslenv, variable);
    }
    for variable in ["ZETTA_THEME/u", "ZETTA_NO_MUX/u"] {
        add_wslenv_entry(&mut wslenv, variable);
    }

    environment.insert("WSLENV".to_owned(), wslenv);
}

pub(crate) fn add_wsl_environment_variable_names<'a, S>(
    environment: &mut HashMap<String, String, S>,
    names: impl IntoIterator<Item = &'a str>,
) where
    S: std::hash::BuildHasher,
{
    let mut wslenv = environment
        .remove("WSLENV")
        .or_else(|| env::var("WSLENV").ok())
        .unwrap_or_default();

    for name in names {
        add_wslenv_entry(&mut wslenv, &format!("{name}/u"));
    }

    environment.insert("WSLENV".to_owned(), wslenv);
}

#[cfg(windows)]
fn wsl_terminal_environment_values_for(
    executable: &Path,
    cwd_tracking_file: Option<&Path>,
    inherited_wslenv: Option<&str>,
) -> (String, Option<String>, String) {
    let mut wslenv = inherited_wslenv.unwrap_or_default().to_owned();
    set_wslenv_entry(&mut wslenv, "ZETTA_HOST_EXECUTABLE/up");
    if cwd_tracking_file.is_some() {
        set_wslenv_entry(&mut wslenv, "ZETTA_CWD_TRACKING_FILE/up");
    }
    (
        executable.to_string_lossy().into_owned(),
        cwd_tracking_file.map(|path| path.to_string_lossy().into_owned()),
        wslenv,
    )
}

#[cfg(windows)]
#[cfg(test)]
fn wsl_terminal_environment_for(
    executable: &Path,
    cwd_tracking_file: Option<&Path>,
    inherited_wslenv: Option<&str>,
) -> HashMap<String, String> {
    let (executable, cwd_tracking_file, wslenv) =
        wsl_terminal_environment_values_for(executable, cwd_tracking_file, inherited_wslenv);

    let mut environment = HashMap::from([
        ("WSLENV".to_owned(), wslenv),
        ("ZETTA_HOST_EXECUTABLE".to_owned(), executable),
    ]);
    if let Some(cwd_tracking_file) = cwd_tracking_file {
        environment.insert("ZETTA_CWD_TRACKING_FILE".to_owned(), cwd_tracking_file);
    }
    environment
}

#[cfg(windows)]
fn windows_wsl_terminal_environment_values(
    cwd_tracking_file: Option<&Path>,
) -> Option<(String, Option<String>, String)> {
    let executable = env::current_exe().ok()?;
    let inherited_wslenv = env::var("WSLENV").ok();
    Some(wsl_terminal_environment_values_for(
        &executable,
        cwd_tracking_file,
        inherited_wslenv.as_deref(),
    ))
}

pub(crate) fn wsl_terminal_environment<S>(
    environment: &mut HashMap<String, String, S>,
    cwd_tracking_file: Option<&Path>,
) where
    S: std::hash::BuildHasher,
{
    #[cfg(windows)]
    {
        if let Some((executable, cwd_tracking_file, wslenv)) =
            windows_wsl_terminal_environment_values(cwd_tracking_file)
        {
            environment.insert("WSLENV".to_owned(), wslenv);
            environment.insert("ZETTA_HOST_EXECUTABLE".to_owned(), executable);
            if let Some(cwd_tracking_file) = cwd_tracking_file {
                environment.insert("ZETTA_CWD_TRACKING_FILE".to_owned(), cwd_tracking_file);
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (environment, cwd_tracking_file);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Msys2Shell {
    Bash,
    Zsh,
}

pub(crate) fn msys2_profile(shell: &Shell) -> Option<(PathBuf, Msys2Shell)> {
    let Shell::WithArguments { program, args, .. } = shell else {
        return None;
    };
    if !program
        .rsplit(['/', '\\'])
        .next()
        .is_some_and(|name| name.eq_ignore_ascii_case("cmd.exe"))
    {
        return None;
    }
    let command = args.last()?.strip_prefix("\"\"")?;
    let launcher_end = command.find("\" -defterm")?;
    let launcher = PathBuf::from(&command[..launcher_end]);
    if !launcher
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("msys2_shell.cmd"))
    {
        return None;
    }
    let shell = command[launcher_end..]
        .split_once(" -shell ")?
        .1
        .strip_suffix('"')?;
    let shell = match shell {
        "bash" => Msys2Shell::Bash,
        "zsh" => Msys2Shell::Zsh,
        _ => return None,
    };
    Some((launcher.parent()?.to_path_buf(), shell))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CygwinShell {
    Bash,
    Zsh,
    Fish,
    Nushell,
}

fn cygwin_shell_name(name: &str) -> Option<CygwinShell> {
    let name = name.to_ascii_lowercase();
    match name.strip_suffix(".exe").unwrap_or(&name) {
        "bash" => Some(CygwinShell::Bash),
        "zsh" => Some(CygwinShell::Zsh),
        "fish" => Some(CygwinShell::Fish),
        "nu" | "nushell" => Some(CygwinShell::Nushell),
        _ => None,
    }
}

fn cygwin_shell_name_from_title(title: Option<&str>) -> Option<CygwinShell> {
    match title?.to_ascii_lowercase().as_str() {
        "cygwin" => Some(CygwinShell::Bash),
        "cygwin: zsh" => Some(CygwinShell::Zsh),
        "cygwin: fish" => Some(CygwinShell::Fish),
        "cygwin: nushell" => Some(CygwinShell::Nushell),
        _ => None,
    }
}

fn cygwin_root_from_program(program: &str) -> Option<PathBuf> {
    let program = Path::new(program);
    let bin = program.parent()?;
    if !bin
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case(OsStr::new("bin")))
    {
        return None;
    }
    bin.parent().map(Path::to_path_buf)
}

pub(crate) fn cygwin_profile(shell: &Shell) -> Option<(PathBuf, CygwinShell)> {
    let (program, title) = match shell {
        Shell::Program(program) => (program, None),
        Shell::WithArguments {
            program,
            title_override,
            ..
        } => (program, title_override.as_deref()),
        Shell::System => return None,
    };
    let root = cygwin_root_from_program(program)?;
    let program_shell = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(cygwin_shell_name)?;
    let title_shell = cygwin_shell_name_from_title(title);
    let shell = title_shell.unwrap_or(program_shell);
    if title_shell.is_none() && !root.join("bin").join("cygwin1.dll").is_file() {
        return None;
    }
    Some((root, shell))
}

pub(crate) fn msys2_path_to_windows(root: &Path, directory: &str) -> Option<PathBuf> {
    if !directory.starts_with('/') || directory.chars().any(char::is_control) {
        return None;
    }
    let parts = directory
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.iter().any(|part| matches!(*part, "." | "..")) {
        return None;
    }
    if directory.starts_with("//") {
        return (parts.len() >= 2)
            .then(|| PathBuf::from(format!(r"\\{}\{}", parts[0], parts[1..].join(r"\"))));
    }
    if parts
        .first()
        .is_some_and(|drive| drive.len() == 1 && drive.as_bytes()[0].is_ascii_alphabetic())
    {
        let drive = parts[0].to_ascii_uppercase();
        let mut path = PathBuf::from(format!("{drive}:\\"));
        path.extend(&parts[1..]);
        return Some(path);
    }
    let mut path = root.to_path_buf();
    path.extend(parts);
    Some(path)
}

pub(crate) fn cygwin_path_to_windows(root: &Path, directory: &str) -> Option<PathBuf> {
    if !directory.starts_with('/') || directory.chars().any(char::is_control) {
        return None;
    }
    let parts = directory
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts
        .iter()
        .any(|part| matches!(*part, "." | "..") || part.contains(['\\', ':']))
    {
        return None;
    }
    if directory.starts_with("//") {
        return (parts.len() >= 2)
            .then(|| PathBuf::from(format!(r"\\{}\{}", parts[0], parts[1..].join(r"\"))));
    }
    if parts
        .first()
        .is_some_and(|part| part.eq_ignore_ascii_case("cygdrive"))
    {
        let drive = parts.get(1)?;
        if drive.len() != 1 || !drive.as_bytes()[0].is_ascii_alphabetic() {
            return None;
        }
        let drive = drive.to_ascii_uppercase();
        let mut path = PathBuf::from(format!(r"{drive}:\"));
        path.extend(&parts[2..]);
        return Some(path);
    }
    let mut path = root.to_path_buf();
    path.extend(parts);
    Some(path)
}

fn path_for_external_editor(path: &str) -> String {
    #[cfg(windows)]
    {
        if let Some(root) = env::var_os("ZETTA_CYGWIN_ROOT") {
            return windows_path_to_cygwin(Path::new(&root), Path::new(path))
                .unwrap_or_else(|| path.to_owned());
        }
        if env::var_os("MSYSTEM").is_some() {
            return windows_path_to_msys(Path::new(path)).unwrap_or_else(|| path.to_owned());
        }
    }
    path.to_owned()
}

pub(crate) fn paths_for_external_editor(arguments: &[String]) -> Vec<String> {
    arguments
        .iter()
        .map(|path| path_for_external_editor(path))
        .collect()
}

#[cfg(not(windows))]
pub(crate) fn msys2_cwd_tracking_environment(
    _shell: &Shell,
    _pane_id: u64,
    _temporary_directory: &Path,
) -> Result<Vec<(String, String)>> {
    Ok(Vec::new())
}

pub(crate) fn launch_working_directory(
    profile: &Profile,
    inherited: Option<PathBuf>,
    inherited_wsl: Option<String>,
    fallback: Option<PathBuf>,
    fallback_is_configured: bool,
) -> (Option<PathBuf>, Option<String>) {
    // Windows process inspection sees the cwd of wsl.exe, not of its Linux shell.
    // Passing that value to a new WSL session leaks Zetta's own launch directory.
    let is_wsl = is_wsl_shell(&profile.command);
    let has_inherited_wsl = inherited_wsl.is_some();
    let working_directory = if is_wsl && has_inherited_wsl {
        None
    } else if is_wsl {
        fallback_is_configured.then_some(fallback).flatten()
    } else {
        inherited.or(fallback)
    };
    let wsl_directory = if is_wsl && has_inherited_wsl {
        inherited_wsl
    } else {
        (is_wsl && !fallback_is_configured).then(|| "~".to_owned())
    };
    (working_directory, wsl_directory)
}

pub(crate) fn wsl_cwd_tracking_file(profile: &Profile, pane_id: u64) -> Option<PathBuf> {
    (cfg!(windows) && is_wsl_shell(&profile.command)).then(|| {
        let path = env::temp_dir().join(format!("zetta-wsl-cwd-{}-{pane_id}", std::process::id()));
        let _ = fs::remove_file(&path);
        path
    })
}

/// The POSIX layer a Windows Bash or Zsh runs under. One tracker script serves
/// both; only Cygwin's also reports the directory as an OSC 7 file URI.
#[cfg(any(windows, test))]
#[derive(Clone, Copy)]
enum PosixLayer {
    Msys2,
    Cygwin,
}

/// The Bash tracker of every POSIX layer Zetta starts a shell in: MSYS2 and
/// Cygwin through `bash_prompt_command`, and WSL through `wsl_cwd_tracker`.
const POSIX_BASH_TRACKER: &str = include_str!("wsl/posix_tracker.bash");

/// What fills a tracker's `# zetta: file-uri` line where `$PWD` is also a path
/// the host can open as a `file://` URI.
const FILE_URI_REPORT: &str = r#"printf '\033]7;file://localhost%s\033\\' "$PWD""#;

/// `script` with its `# zetta: file-uri` line filled in for `layer`. MSYS2
/// leaves the comment where it is.
#[cfg(any(windows, test))]
fn posix_tracker(script: &str, layer: PosixLayer) -> String {
    match layer {
        PosixLayer::Msys2 => script.to_owned(),
        PosixLayer::Cygwin => script.replace("# zetta: file-uri", FILE_URI_REPORT),
    }
}

/// The `PROMPT_COMMAND` that loads the Bash tracker. The tracker runs once and
/// replaces itself; `existing`, the user's own `PROMPT_COMMAND`, is handed to
/// it as a quoted word and runs at every prompt after Zetta's own hook.
///
/// The value ends on the tracker's last command, not a newline, because rc
/// files extend it with `PROMPT_COMMAND="$PROMPT_COMMAND; …"`, and a line that
/// starts with `;` is a syntax error Bash skips without a word.
#[cfg(any(windows, test))]
fn bash_prompt_command(layer: PosixLayer, existing: Option<&str>) -> String {
    let existing = existing.unwrap_or_default();
    posix_tracker(POSIX_BASH_TRACKER, layer)
        .replace(
            "# zetta: user-prompt-command",
            &format!(
                "__zetta_user_prompt_command='{}'",
                existing.replace('\'', r"'\''")
            ),
        )
        .trim_end()
        .to_owned()
}

/// The script a WSL pane's `/bin/sh` runs to start the user's shell with
/// tracking. Its Bash branch carries the same tracker MSYS2 and Cygwin use,
/// with the WSL loader in place of a hook known in advance: the distribution's
/// own `PROMPT_COMMAND` is only known inside it.
pub(crate) fn wsl_cwd_tracker() -> &'static str {
    static TRACKER: LazyLock<String> = LazyLock::new(|| {
        let bash = POSIX_BASH_TRACKER
            .replace("# zetta: file-uri", FILE_URI_REPORT)
            .replace(
                "# zetta: user-prompt-command\n",
                include_str!("wsl/wsl_bash_loader.bash"),
            );
        include_str!("wsl/cwd_tracker.sh").replace("# zetta: bash-tracker", bash.trim_end())
    });
    &TRACKER
}

pub(crate) fn wsl_shell_with_tracking(
    shell: Shell,
    directory: Option<&str>,
    cwd_file: Option<&Path>,
) -> Shell {
    match shell {
        Shell::Program(program) => {
            wsl_command_with_tracking(program, Vec::new(), None, directory, cwd_file)
        }
        Shell::WithArguments {
            program,
            args,
            title_override,
        } => wsl_command_with_tracking(program, args, title_override, directory, cwd_file),
        Shell::System => Shell::System,
    }
}

pub(crate) fn wsl_command_with_tracking(
    program: String,
    mut args: Vec<String>,
    title_override: Option<String>,
    directory: Option<&str>,
    cwd_file: Option<&Path>,
) -> Shell {
    let exec_index = args.iter().position(|arg| arg == "--exec" || arg == "-e");
    if let Some(directory) = directory
        && !args
            .iter()
            .take(exec_index.unwrap_or(args.len()))
            .any(|arg| arg == "--cd" || arg.starts_with("--cd="))
    {
        args.splice(
            exec_index.unwrap_or(args.len())..exec_index.unwrap_or(args.len()),
            ["--cd".to_owned(), directory.to_owned()],
        );
    }
    if exec_index.is_none() && cwd_file.is_some() {
        args.extend([
            "--exec".to_owned(),
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            wsl_cwd_tracker().to_owned(),
            "zetta-wsl-cwd".to_owned(),
        ]);
    }
    Shell::WithArguments {
        program,
        args,
        title_override,
    }
}

#[cfg(test)]
#[path = "../tests/startup/wsl.rs"]
mod tests;
