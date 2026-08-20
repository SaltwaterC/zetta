use super::*;

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

fn path_for_external_editor(path: &str) -> String {
    #[cfg(windows)]
    {
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
const MSYS2_BASH_TRACKER: &str = r#"__zetta_preexec() {
    [[ "$__zetta_at_prompt" == 1 ]] || return
    __zetta_at_prompt=0
    printf '\033]2;zetta-cmd:%s\033\\' "$BASH_COMMAND"
}
__zetta_precmd() {
    printf '\033]2;zetta-cwd:%s\033\\' "$PWD"
    printf '\033]2;zetta-cmd:bash\033\\'
    __zetta_at_prompt=1
}
trap '__zetta_preexec' DEBUG"#;

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
    [[ "$PWD" == /* ]] && printf '\033]2;zetta-cwd:%s\033\\' "$PWD"
    printf '\033]2;zetta-cmd:zsh\033\\'
}
function __zetta_report_preexec() {
    printf '\033]2;zetta-cmd:%s\033\\' "$1"
}
autoload -Uz add-zsh-hook
add-zsh-hook precmd __zetta_report_cwd
add-zsh-hook preexec __zetta_report_preexec
command rm -rf -- "$ZETTA_INTEGRATION_ZDOTDIR"
unset ZETTA_ORIGINAL_ZDOTDIR ZETTA_INTEGRATION_ZDOTDIR original_zdotdir
"#;

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
                    "{MSYS2_BASH_TRACKER}{};__zetta_precmd",
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
    case "$BASH_COMMAND" in
        __zetta_precmd) return ;;
    esac
    printf '\033]2;zetta-cmd:%s\033\\' "$BASH_COMMAND"
}
__zetta_precmd() {
    if [[ ${_zetta_integration_attempted:-0} != 1 && -n ${ZETTA_HOST_EXECUTABLE:-} ]] &&
        ! declare -F _zetta_complete >/dev/null; then
        _zetta_integration_attempted=1
        eval "$("$ZETTA_HOST_EXECUTABLE" init bash)"
    fi
    case "$PWD" in
        /*) printf '\033]7;file://localhost%s\033\\\033]2;zetta-cwd:%s\033\\' "$PWD" "$PWD" ;;
    esac
    printf '\033]2;zetta-cmd:%s\033\\' "$ZETTA_SHELL_NAME"
}
trap '__zetta_preexec' DEBUG
PROMPT_COMMAND="__zetta_precmd${ZETTA_ORIGINAL_PROMPT_COMMAND:+;${ZETTA_ORIGINAL_PROMPT_COMMAND}}"
__zetta_precmd
ZETTA_BASH_PROMPT
)"
        export ZETTA_ORIGINAL_PROMPT_COMMAND="$PROMPT_COMMAND"
        PROMPT_COMMAND="$zetta_full_prompt_command"
        export PROMPT_COMMAND
        exec "$shell" -l
        ;;
    fish)
        exec "$shell" -l -C 'function __zetta_report_cwd --on-event fish_prompt; if string match -qr "^/" -- "$PWD"; printf "\033]7;file://localhost%s\033\\" "$PWD"; printf "\033]2;zetta-cwd:%s\033\\" "$PWD"; end; printf "\033]2;zetta-cmd:%s\033\\" "$ZETTA_SHELL_NAME"; end; function __zetta_report_preexec --on-event fish_preexec; printf "\033]2;zetta-cmd:%s\033\\" "$argv[1]"; end; if test -n "$ZETTA_HOST_EXECUTABLE"; and not functions -q __zetta_at_subcommand; $ZETTA_HOST_EXECUTABLE init fish | source; end'
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
    [[ "$PWD" == /* ]] && printf '\033]7;file://localhost%s\033\\\033]2;zetta-cwd:%s\033\\' "$PWD" "$PWD"
    printf '\033]2;zetta-cmd:%s\033\\' "$ZETTA_SHELL_NAME"
}
function __zetta_report_preexec() {
    printf '\033]2;zetta-cmd:%s\033\\' "$1"
}
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
