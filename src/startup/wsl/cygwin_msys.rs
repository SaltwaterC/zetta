//! The MSYS2 and Cygwin layers: translating a Windows path into each one's
//! POSIX form, and the environment and per-shell trackers that report a
//! pane's working directory back from inside them.

use super::*;

pub(super) fn windows_path_to_msys(path: &Path) -> Option<String> {
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

pub(super) const POSIX_ZSH_TRACKER: &str = include_str!("posix_tracker.zsh");

pub(super) const CYGWIN_FISH_TRACKER: &str = include_str!("cygwin_tracker.fish");

pub(super) fn cygwin_nushell_tracker(config_path: &str) -> String {
    include_str!("cygwin_tracker.nu")
        .replace("__ZETTA_CONFIG_PATH__", &config_path.replace('\'', "''"))
}

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

pub(crate) fn msys2_cwd_tracking_environment(
    shell: &Shell,
    pane_id: u64,
    temporary_directory: &Path,
) -> Result<Vec<(String, String)>> {
    let Some((_, shell)) = msys2_profile(shell) else {
        return Ok(Vec::new());
    };
    match shell {
        Msys2Shell::Bash => Ok(vec![(
            "PROMPT_COMMAND".to_owned(),
            bash_prompt_command(
                PosixLayer::Msys2,
                env::var("PROMPT_COMMAND").ok().as_deref(),
            ),
        )]),
        Msys2Shell::Zsh => {
            let directory = temporary_directory
                .join(format!("zetta-msys2-zsh-{}-{pane_id}", std::process::id()));
            fs::create_dir_all(&directory)
                .with_context(|| format!("creating {}", directory.display()))?;
            fs::write(
                directory.join(".zshenv"),
                posix_tracker(POSIX_ZSH_TRACKER, PosixLayer::Msys2),
            )
            .with_context(|| {
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

pub(super) fn cygwin_path_environment_value(root: &Path, inherited_path: Option<&str>) -> String {
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
                bash_prompt_command(
                    PosixLayer::Cygwin,
                    env::var("PROMPT_COMMAND").ok().as_deref(),
                ),
            ));
        }
        CygwinShell::Zsh => {
            let directory = temporary_directory
                .join(format!("zetta-cygwin-zsh-{}-{pane_id}", std::process::id()));
            fs::create_dir_all(&directory)
                .with_context(|| format!("creating {}", directory.display()))?;
            fs::write(
                directory.join(".zshenv"),
                posix_tracker(POSIX_ZSH_TRACKER, PosixLayer::Cygwin),
            )
            .with_context(|| {
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
            bash_prompt_command(PosixLayer::Cygwin, existing),
        );
    }
}
