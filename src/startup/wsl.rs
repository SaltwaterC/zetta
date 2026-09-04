use super::*;
use std::ffi::OsStr;

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

#[cfg(windows)]
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

    for variable in [
        "ZETTA_PROCESS_ID/u",
        "ZETTA_ATTENTION_ID/u",
        "ZETTA_PANE_ID/u",
        "ZETTA_PANE_ROUTING_ID/u",
        "ZETTA_THEME/u",
        "ZETTA_NO_MUX/u",
    ] {
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

#[cfg(windows)]
fn windows_path_to_msys(path: &Path) -> Option<String> {
    let path = path.to_string_lossy().replace('\\', "/");
    let bytes = path.as_bytes();
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1..3] == *b":/" {
        return Some(format!(
            "/{}/{}",
            (bytes[0] as char).to_ascii_lowercase(),
            &path[3..]
        ));
    }
    path.strip_prefix("//")
        .map(|path| format!("//{path}"))
        .or_else(|| path.starts_with('/').then_some(path))
}

#[cfg(windows)]
pub(crate) fn windows_path_to_cygwin(root: &Path, path: &Path) -> Option<String> {
    let mut path = path.to_string_lossy().replace('\\', "/");
    if path.chars().any(char::is_control) {
        return None;
    }
    if let Some(stripped) = path.strip_prefix("//?/") {
        path = stripped.to_owned();
    }
    if path
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("UNC/"))
    {
        path = format!("//{}", &path[4..]);
    }

    let mut root = root.to_string_lossy().replace('\\', "/");
    while root.ends_with('/') {
        root.pop();
    }
    if path.eq_ignore_ascii_case(&root) {
        return Some("/".to_owned());
    }
    if path.len() > root.len()
        && path[root.len()..].starts_with('/')
        && path[..root.len()].eq_ignore_ascii_case(&root)
    {
        return Some(format!("/{}", &path[root.len() + 1..]));
    }
    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        let suffix = path[2..].trim_start_matches('/');
        return if suffix.is_empty() {
            Some(format!(
                "/cygdrive/{}",
                (bytes[0] as char).to_ascii_lowercase()
            ))
        } else {
            Some(format!(
                "/cygdrive/{}/{}",
                (bytes[0] as char).to_ascii_lowercase(),
                suffix
            ))
        };
    }
    if path.starts_with("//") {
        return Some(path);
    }
    path.starts_with('/').then_some(path)
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

#[cfg(windows)]
const MSYS2_BASH_TRACKER: &str = r#"__zetta_at_prompt=0
__zetta_command_started=0
__zetta_preexec() {
    [[ "$__zetta_at_prompt" == 1 ]] || return
    __zetta_at_prompt=0
    case "$BASH_COMMAND" in
        __zetta_precmd|__zetta_mark_prompt) return ;;
    esac
    __zetta_command_started=1
    printf '\033]2;zetta-event:command-started:%s\033\\' "$BASH_COMMAND"
    printf '\033]2;zetta-cmd:%s\033\\' "$BASH_COMMAND"
}
__zetta_precmd() {
    local status=$?
    if [[ "$__zetta_command_started" == 1 ]]; then
        printf '\033]2;zetta-event:command-finished:%s\033\\' "$status"
        __zetta_command_started=0
    fi
    printf '\033]2;zetta-cwd:%s\033\\' "$PWD"
    printf '\033]2;zetta-cmd:bash\033\\'
    return "$status"
}
__zetta_mark_prompt() {
    __zetta_at_prompt=1
}
trap '__zetta_preexec' DEBUG
printf '\033]2;zetta-event:tracking-ready\033\\'"#;

#[cfg(windows)]
const MSYS2_ZSH_TRACKER: &str = r#"if [[ -n ${ZETTA_ORIGINAL_ZDOTDIR+x} ]]; then
    ZDOTDIR="$ZETTA_ORIGINAL_ZDOTDIR"
    export ZDOTDIR
else
    unset ZDOTDIR
fi
original_zdotdir="${ZDOTDIR:-$HOME}"
[[ -r "$original_zdotdir/.zshenv" ]] && source "$original_zdotdir/.zshenv"

function __zetta_report_cwd() {
    local zetta_status=$?
    if (( __ZETTA_COMMAND_STARTED )); then
        printf '\033]2;zetta-event:command-finished:%s\033\\' "$zetta_status"
        __ZETTA_COMMAND_STARTED=0
    fi
    [[ "$PWD" == /* ]] && printf '\033]2;zetta-cwd:%s\033\\' "$PWD"
    printf '\033]2;zetta-cmd:zsh\033\\'
    return $zetta_status
}
function __zetta_report_preexec() {
    __ZETTA_COMMAND_STARTED=1
    printf '\033]2;zetta-event:command-started:%s\033\\' "$1"
    printf '\033]2;zetta-cmd:%s\033\\' "$1"
}
typeset -g __ZETTA_COMMAND_STARTED=0
autoload -Uz add-zsh-hook
add-zsh-hook precmd __zetta_report_cwd
add-zsh-hook preexec __zetta_report_preexec
command rm -rf -- "$ZETTA_INTEGRATION_ZDOTDIR"
unset ZETTA_ORIGINAL_ZDOTDIR ZETTA_INTEGRATION_ZDOTDIR original_zdotdir
printf '\033]2;zetta-event:tracking-ready\033\\'
"#;

#[cfg(windows)]
const CYGWIN_BASH_TRACKER: &str = r#"__zetta_at_prompt=0
__zetta_command_started=0
__zetta_preexec() {
    [[ "$__zetta_at_prompt" == 1 ]] || return
    __zetta_at_prompt=0
    case "$BASH_COMMAND" in
        __zetta_precmd|__zetta_mark_prompt) return ;;
    esac
    __zetta_command_started=1
    printf '\033]2;zetta-event:command-started:%s\033\\' "$BASH_COMMAND"
    printf '\033]2;zetta-cmd:%s\033\\' "$BASH_COMMAND"
}
__zetta_precmd() {
    local status=$?
    if [[ "$__zetta_command_started" == 1 ]]; then
        printf '\033]2;zetta-event:command-finished:%s\033\\' "$status"
        __zetta_command_started=0
    fi
    case "$PWD" in
        /*) printf '\033]7;file://localhost%s\033\\\033]2;zetta-cwd:%s\033\\' "$PWD" "$PWD" ;;
    esac
    printf '\033]2;zetta-cmd:bash\033\\'
    return "$status"
}
__zetta_mark_prompt() {
    __zetta_at_prompt=1
}
trap '__zetta_preexec' DEBUG
printf '\033]2;zetta-event:tracking-ready\033\\'"#;

#[cfg(windows)]
const CYGWIN_ZSH_TRACKER: &str = r#"if [[ -n ${ZETTA_ORIGINAL_ZDOTDIR+x} ]]; then
    ZDOTDIR="$ZETTA_ORIGINAL_ZDOTDIR"
    export ZDOTDIR
else
    unset ZDOTDIR
fi
original_zdotdir="${ZDOTDIR:-$HOME}"
[[ -r "$original_zdotdir/.zshenv" ]] && source "$original_zdotdir/.zshenv"

function __zetta_report_cwd() {
    local zetta_status=$?
    if (( __ZETTA_COMMAND_STARTED )); then
        printf '\033]2;zetta-event:command-finished:%s\033\\' "$zetta_status"
        __ZETTA_COMMAND_STARTED=0
    fi
    [[ "$PWD" == /* ]] && printf '\033]7;file://localhost%s\033\\\033]2;zetta-cwd:%s\033\\' "$PWD" "$PWD"
    printf '\033]2;zetta-cmd:zsh\033\\'
    return $zetta_status
}
function __zetta_report_preexec() {
    __ZETTA_COMMAND_STARTED=1
    printf '\033]2;zetta-event:command-started:%s\033\\' "$1"
    printf '\033]2;zetta-cmd:%s\033\\' "$1"
}
typeset -g __ZETTA_COMMAND_STARTED=0
autoload -Uz add-zsh-hook
add-zsh-hook precmd __zetta_report_cwd
add-zsh-hook preexec __zetta_report_preexec
command rm -rf -- "$ZETTA_INTEGRATION_ZDOTDIR"
unset ZETTA_ORIGINAL_ZDOTDIR ZETTA_INTEGRATION_ZDOTDIR original_zdotdir
printf '\033]2;zetta-event:tracking-ready\033\\'
"#;

#[cfg(windows)]
const CYGWIN_FISH_TRACKER: &str = r#"set -g __ZETTA_COMMAND_STARTED 0; function __zetta_report_cwd --on-event fish_prompt; set -l command_status $status; if test "$__ZETTA_COMMAND_STARTED" = 1; printf '\033]2;zetta-event:command-finished:%s\033\\' "$command_status"; set -g __ZETTA_COMMAND_STARTED 0; end; if string match -qr '^/' -- "$PWD"; printf '\033]7;file://localhost%s\033\\' "$PWD"; printf '\033]2;zetta-cwd:%s\033\\' "$PWD"; end; printf '\033]2;zetta-cmd:fish\033\\'; end; function __zetta_report_preexec --on-event fish_preexec; set -g __ZETTA_COMMAND_STARTED 1; printf '\033]2;zetta-event:command-started:%s\033\\' "$argv[1]"; printf '\033]2;zetta-cmd:%s\033\\' "$argv[1]"; end; printf '\033]2;zetta-event:tracking-ready\033\\'"#;

#[cfg(windows)]
fn cygwin_nushell_tracker(config_path: &str) -> String {
    format!(
        r#"let zetta_user_config = ($nu.default-config-dir | path join 'config.nu')
if ($zetta_user_config | path exists) {{ source $zetta_user_config }}
$env.config.hooks.pre_prompt = ($env.config.hooks.pre_prompt | append {{||
    if ($env.ZETTA_COMMAND_STARTED? | default false) {{
        let status = ($env.LAST_EXIT_CODE? | default 0)
        print -n $"\e]2;zetta-event:command-finished:($status)\e\\"
        $env.ZETTA_COMMAND_STARTED = false
    }}
    print -n $"\e]7;file://localhost($env.PWD)\e\\"
    print -n $"\e]2;zetta-cwd:($env.PWD)\e\\"
    print -n "\e]2;zetta-cmd:nu\e\\"
}})
$env.config.hooks.pre_execution = ($env.config.hooks.pre_execution | append {{||
    let command = (commandline)
    $env.ZETTA_COMMAND_STARTED = true
    print -n $"\e]2;zetta-event:command-started:($command)\e\\"
    print -n $"\e]2;zetta-cmd:($command)\e\\"
}})
print -n "\e]2;zetta-event:tracking-ready\e\\"
^rm -f -- '{}'
"#,
        config_path.replace('\'', "''")
    )
}

#[cfg(windows)]
pub(crate) fn cygwin_shell_with_tracking(
    shell: Shell,
    pane_id: u64,
    temporary_directory: &Path,
) -> Result<Shell> {
    let Some((root, shell_kind)) = cygwin_profile(&shell) else {
        return Ok(shell);
    };

    let make_nushell_config = || -> Result<String> {
        let path = temporary_directory.join(format!(
            "zetta-cygwin-nu-{}-{pane_id}.nu",
            std::process::id()
        ));
        let cygwin_path = windows_path_to_cygwin(&root, &path)
            .context("temporary Nushell config cannot be represented as a Cygwin path")?;
        fs::write(&path, cygwin_nushell_tracker(&cygwin_path)).with_context(|| {
            format!(
                "writing Cygwin Nushell CWD integration in {}",
                path.display()
            )
        })?;
        Ok(cygwin_path)
    };

    let add_tracking_arguments = |args: &mut Vec<String>| -> Result<()> {
        match shell_kind {
            CygwinShell::Fish => {
                if !args.iter().any(|arg| arg == "-C" || arg == "--command") {
                    args.extend(["-C".to_owned(), CYGWIN_FISH_TRACKER.to_owned()]);
                }
            }
            CygwinShell::Nushell => {
                args.extend(["--config".to_owned(), make_nushell_config()?]);
            }
            CygwinShell::Bash | CygwinShell::Zsh => {}
        }
        Ok(())
    };

    match shell {
        Shell::Program(program) => {
            let mut args = Vec::new();
            add_tracking_arguments(&mut args)?;
            Ok(Shell::WithArguments {
                program,
                args,
                title_override: None,
            })
        }
        Shell::WithArguments {
            program,
            mut args,
            title_override,
        } => {
            add_tracking_arguments(&mut args)?;
            Ok(Shell::WithArguments {
                program,
                args,
                title_override,
            })
        }
        Shell::System => unreachable!(),
    }
}

#[cfg(windows)]
pub(crate) fn msys2_cwd_tracking_environment(
    shell: &Shell,
    pane_id: u64,
    temporary_directory: &Path,
) -> Result<Vec<(String, String)>> {
    let Some((_, shell)) = msys2_profile(shell) else {
        return Ok(Vec::new());
    };
    match shell {
        Msys2Shell::Bash => {
            let existing = env::var("PROMPT_COMMAND").ok();
            Ok(vec![(
                "PROMPT_COMMAND".to_owned(),
                format!(
                    "{MSYS2_BASH_TRACKER}__zetta_precmd{};__zetta_mark_prompt",
                    existing
                        .filter(|command| !command.is_empty())
                        .map(|command| format!(";{command}"))
                        .unwrap_or_default()
                ),
            )])
        }
        Msys2Shell::Zsh => {
            let directory = temporary_directory
                .join(format!("zetta-msys2-zsh-{}-{pane_id}", std::process::id()));
            fs::create_dir_all(&directory)
                .with_context(|| format!("creating {}", directory.display()))?;
            fs::write(directory.join(".zshenv"), MSYS2_ZSH_TRACKER).with_context(|| {
                format!(
                    "writing MSYS2 Zsh CWD integration in {}",
                    directory.display()
                )
            })?;
            let msys_directory = windows_path_to_msys(&directory)
                .context("temporary directory cannot be represented as an MSYS2 path")?;
            let mut environment = vec![
                ("ZDOTDIR".to_owned(), msys_directory.clone()),
                ("ZETTA_INTEGRATION_ZDOTDIR".to_owned(), msys_directory),
            ];
            if let Some(original) = env::var_os("ZDOTDIR") {
                let original = PathBuf::from(original);
                let original = if original.is_absolute() {
                    windows_path_to_msys(&original)
                        .context("ZDOTDIR cannot be represented as an MSYS2 path")?
                } else {
                    original.to_string_lossy().into_owned()
                };
                environment.push(("ZETTA_ORIGINAL_ZDOTDIR".to_owned(), original));
            }
            Ok(environment)
        }
    }
}

#[cfg(not(windows))]
pub(crate) fn msys2_cwd_tracking_environment(
    _shell: &Shell,
    _pane_id: u64,
    _temporary_directory: &Path,
) -> Result<Vec<(String, String)>> {
    Ok(Vec::new())
}

#[cfg(windows)]
fn cygwin_path_environment_value(root: &Path, inherited_path: Option<&str>) -> String {
    let inherited_path = inherited_path
        .map(OsString::from)
        .or_else(|| env::var_os("PATH"));
    let inherited_text = inherited_path
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned());
    let cygwin_bin = root.join("bin");
    let cygwin_bin_key = cygwin_bin
        .to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase();
    let mut paths = vec![cygwin_bin.clone()];
    if let Some(inherited_path) = inherited_path.as_ref() {
        paths.extend(env::split_paths(&inherited_path).filter(|path| {
            path.to_string_lossy()
                .replace('\\', "/")
                .trim_end_matches('/')
                .to_ascii_lowercase()
                != cygwin_bin_key
        }));
    }
    if let Ok(joined) = env::join_paths(paths) {
        return joined.to_string_lossy().into_owned();
    }
    // Joining only fails when a component contains the separator, which leaves
    // no vector to join; build the string directly instead.
    let separator = if cfg!(windows) { ";" } else { ":" };
    let root = cygwin_bin.to_string_lossy().into_owned();
    inherited_text
        .map(|path| format!("{root}{separator}{path}"))
        .unwrap_or(root)
}

#[cfg(windows)]
pub(crate) fn cygwin_cwd_tracking_environment_with_path(
    shell: &Shell,
    pane_id: u64,
    temporary_directory: &Path,
    inherited_path: Option<&str>,
) -> Result<Vec<(String, String)>> {
    let Some((root, shell_kind)) = cygwin_profile(shell) else {
        return Ok(Vec::new());
    };

    let mut environment = vec![
        (
            "PATH".to_owned(),
            cygwin_path_environment_value(&root, inherited_path),
        ),
        ("CHERE_INVOKING".to_owned(), "1".to_owned()),
        (
            "ZETTA_CYGWIN_ROOT".to_owned(),
            root.to_string_lossy().into_owned(),
        ),
    ];
    match shell_kind {
        CygwinShell::Bash => {
            environment.push((
                "PROMPT_COMMAND".to_owned(),
                cygwin_bash_prompt_command(env::var("PROMPT_COMMAND").ok().as_deref()),
            ));
        }
        CygwinShell::Zsh => {
            let directory = temporary_directory
                .join(format!("zetta-cygwin-zsh-{}-{pane_id}", std::process::id()));
            fs::create_dir_all(&directory)
                .with_context(|| format!("creating {}", directory.display()))?;
            fs::write(directory.join(".zshenv"), CYGWIN_ZSH_TRACKER).with_context(|| {
                format!(
                    "writing Cygwin Zsh CWD integration in {}",
                    directory.display()
                )
            })?;
            let cygwin_directory = windows_path_to_cygwin(&root, &directory)
                .context("temporary directory cannot be represented as a Cygwin path")?;
            let mut zdotdir = vec![
                ("ZDOTDIR".to_owned(), cygwin_directory.clone()),
                ("ZETTA_INTEGRATION_ZDOTDIR".to_owned(), cygwin_directory),
            ];
            if let Some(original) = env::var_os("ZDOTDIR") {
                let original = PathBuf::from(original);
                let original = if original.is_absolute() {
                    windows_path_to_cygwin(&root, &original)
                        .context("ZDOTDIR cannot be represented as a Cygwin path")?
                } else {
                    original.to_string_lossy().into_owned()
                };
                zdotdir.push(("ZETTA_ORIGINAL_ZDOTDIR".to_owned(), original));
            }
            environment.extend(zdotdir);
        }
        CygwinShell::Fish | CygwinShell::Nushell => {}
    }
    Ok(environment)
}

#[cfg(windows)]
pub(crate) fn ensure_cygwin_environment<S>(
    shell: &Shell,
    environment: &mut HashMap<String, String, S>,
) where
    S: std::hash::BuildHasher,
{
    let Some((root, shell_kind)) = cygwin_profile(shell) else {
        return;
    };
    let inherited_path = environment.get("PATH").map(String::as_str);
    environment.insert(
        "PATH".to_owned(),
        cygwin_path_environment_value(&root, inherited_path),
    );
    environment.insert("CHERE_INVOKING".to_owned(), "1".to_owned());
    environment.insert(
        "ZETTA_CYGWIN_ROOT".to_owned(),
        root.to_string_lossy().into_owned(),
    );
    if matches!(shell_kind, CygwinShell::Zsh)
        && let Some(integration_directory) = environment.get("ZETTA_INTEGRATION_ZDOTDIR").cloned()
    {
        environment.insert("ZDOTDIR".to_owned(), integration_directory);
    }
    if matches!(shell_kind, CygwinShell::Bash)
        && !environment
            .get("PROMPT_COMMAND")
            .is_some_and(|command| command.contains("__zetta_precmd"))
    {
        let existing = environment
            .get("PROMPT_COMMAND")
            .map(String::as_str)
            .filter(|command| !command.is_empty());
        environment.insert(
            "PROMPT_COMMAND".to_owned(),
            cygwin_bash_prompt_command(existing),
        );
    }
}

#[cfg(windows)]
fn cygwin_bash_prompt_command(existing: Option<&str>) -> String {
    format!(
        "{CYGWIN_BASH_TRACKER}__zetta_precmd{};__zetta_mark_prompt",
        existing
            .filter(|command| !command.is_empty())
            .map(|command| format!(";{command}"))
            .unwrap_or_default()
    )
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

pub(crate) const WSL_CWD_TRACKER: &str = r#"marker="${ZETTA_CWD_TRACKING_FILE:-}"
unset ZETTA_CWD_TRACKING_FILE
shell="${SHELL:-}"
if [ ! -x "$shell" ]; then
    shell="$(getent passwd "$(id -u)" 2>/dev/null | cut -d: -f7)"
fi
[ -x "$shell" ] || shell=/bin/sh
# Windows-side process inspection can't see into the WSL VM's own process
# namespace, so the tab title can't be derived from the host process tree the
# way it is for native Windows shells. Report it explicitly instead: a
# `zetta-cmd:<value>` title marker carrying the shell name at idle, or the
# command about to run, mirrored by `reported_foreground_command_from_title`
# in crates/terminal/src/terminal.rs.
export ZETTA_SHELL_NAME="${shell##*/}"

case "${shell##*/}" in
    bash)
        zetta_full_prompt_command="$(cat <<'ZETTA_BASH_PROMPT'
__zetta_preexec() {
    [[ "${__zetta_at_prompt:-0}" == 1 ]] || return
    __zetta_at_prompt=0
    case "$BASH_COMMAND" in
        __zetta_precmd|__zetta_mark_prompt) return ;;
    esac
    __zetta_command_started=1
    printf '\033]2;zetta-event:command-started:%s\033\\' "$BASH_COMMAND"
    printf '\033]2;zetta-cmd:%s\033\\' "$BASH_COMMAND"
}
__zetta_precmd() {
    local status=$?
    if [[ ${_zetta_integration_attempted:-0} != 1 && -n ${ZETTA_HOST_EXECUTABLE:-} ]] &&
        ! declare -F _zetta_complete >/dev/null; then
        _zetta_integration_attempted=1
        eval "$("$ZETTA_HOST_EXECUTABLE" init bash)"
    fi
    if [[ "$__zetta_command_started" == 1 ]]; then
        printf '\033]2;zetta-event:command-finished:%s\033\\' "$status"
        __zetta_command_started=0
    fi
    case "$PWD" in
        /*) printf '\033]7;file://localhost%s\033\\\033]2;zetta-cwd:%s\033\\' "$PWD" "$PWD" ;;
    esac
    printf '\033]2;zetta-cmd:%s\033\\' "$ZETTA_SHELL_NAME"
    return "$status"
}
__zetta_mark_prompt() {
    __zetta_at_prompt=1
}
trap '__zetta_preexec' DEBUG
PROMPT_COMMAND="__zetta_precmd${ZETTA_ORIGINAL_PROMPT_COMMAND:+;${ZETTA_ORIGINAL_PROMPT_COMMAND}};__zetta_mark_prompt"
printf '\033]2;zetta-event:tracking-ready\033\\'
__zetta_precmd
ZETTA_BASH_PROMPT
)"
        export ZETTA_ORIGINAL_PROMPT_COMMAND="$PROMPT_COMMAND"
        PROMPT_COMMAND="$zetta_full_prompt_command"
        export PROMPT_COMMAND
        exec "$shell" -l
        ;;
    fish)
        exec "$shell" -l -C 'set -g __ZETTA_COMMAND_STARTED 0; function __zetta_report_cwd --on-event fish_prompt; set -l command_status $status; if test "$__ZETTA_COMMAND_STARTED" = 1; printf "\033]2;zetta-event:command-finished:%s\033\\" "$command_status"; set -g __ZETTA_COMMAND_STARTED 0; end; if string match -qr "^/" -- "$PWD"; printf "\033]7;file://localhost%s\033\\" "$PWD"; printf "\033]2;zetta-cwd:%s\033\\" "$PWD"; end; printf "\033]2;zetta-cmd:%s\033\\" "$ZETTA_SHELL_NAME"; end; function __zetta_report_preexec --on-event fish_preexec; set -g __ZETTA_COMMAND_STARTED 1; printf "\033]2;zetta-event:command-started:%s\033\\" "$argv[1]"; printf "\033]2;zetta-cmd:%s\033\\" "$argv[1]"; end; printf "\033]2;zetta-event:tracking-ready\033\\"; if test -n "$ZETTA_HOST_EXECUTABLE"; and not functions -q __zetta_at_subcommand; $ZETTA_HOST_EXECUTABLE init fish | source; end'
        ;;
    zsh)
        integration_zdotdir="$(mktemp -d "${TMPDIR:-/tmp}/zetta-zsh-XXXXXX" 2>/dev/null || true)"
        if [ -n "$integration_zdotdir" ]; then
            export ZETTA_ORIGINAL_ZDOTDIR="${ZDOTDIR:-$HOME}"
            export ZETTA_INTEGRATION_ZDOTDIR="$integration_zdotdir"
            cat > "$integration_zdotdir/.zshenv" <<'ZETTA_ZSHENV'
ZDOTDIR="$ZETTA_ORIGINAL_ZDOTDIR"
[[ -r "$ZDOTDIR/.zshenv" ]] && source "$ZDOTDIR/.zshenv"

if [[ -n ${ZETTA_HOST_EXECUTABLE:-} ]]; then
    function zetta { command "$ZETTA_HOST_EXECUTABLE" "$@"; }
fi

function __zetta_report_cwd() {
    local zetta_status=$?
    if (( __ZETTA_COMMAND_STARTED )); then
        printf '\033]2;zetta-event:command-finished:%s\033\\' "$zetta_status"
        __ZETTA_COMMAND_STARTED=0
    fi
    [[ "$PWD" == /* ]] && printf '\033]7;file://localhost%s\033\\\033]2;zetta-cwd:%s\033\\' "$PWD" "$PWD"
    printf '\033]2;zetta-cmd:%s\033\\' "$ZETTA_SHELL_NAME"
    return $zetta_status
}
function __zetta_report_preexec() {
    __ZETTA_COMMAND_STARTED=1
    printf '\033]2;zetta-event:command-started:%s\033\\' "$1"
    printf '\033]2;zetta-cmd:%s\033\\' "$1"
}
typeset -g __ZETTA_COMMAND_STARTED=0
function __zetta_load_shell_integration() {
    add-zsh-hook -d precmd __zetta_load_shell_integration
    if [[ -n ${ZETTA_HOST_EXECUTABLE:-} ]] && (( ! $+functions[_zetta] )); then
        eval "$("$ZETTA_HOST_EXECUTABLE" init zsh)"
    fi
}
autoload -Uz add-zsh-hook
add-zsh-hook precmd __zetta_load_shell_integration
add-zsh-hook precmd __zetta_report_cwd
add-zsh-hook preexec __zetta_report_preexec
command rm -rf -- "$ZETTA_INTEGRATION_ZDOTDIR"
unset ZETTA_ORIGINAL_ZDOTDIR ZETTA_INTEGRATION_ZDOTDIR
printf '\033]2;zetta-event:tracking-ready\033\\'
ZETTA_ZSHENV
            ZDOTDIR="$integration_zdotdir"
            export ZDOTDIR
            exec "$shell" -l
        fi
        ;;
esac

# Shells without an injection mechanism retain the legacy tracker.
parent=$$
if [ -n "$marker" ]; then
    (
        previous=
        while kill -0 "$parent" 2>/dev/null; do
            cwd="$(readlink "/proc/$parent/cwd" 2>/dev/null)" || break
            if [ "$cwd" != "$previous" ]; then
                printf '%s\n' "$cwd" > "${marker}.tmp" && mv -f "${marker}.tmp" "$marker"
                previous="$cwd"
            fi
            sleep 0.1
        done
        rm -f "$marker" "${marker}.tmp"
    ) </dev/null >/dev/null 2>&1 &
fi
exec "$shell" -l"#;

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
            WSL_CWD_TRACKER.to_owned(),
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
