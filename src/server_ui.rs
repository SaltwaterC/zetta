use super::*;

pub(crate) enum ServerRoot {
    Local(PathBuf),
    Wsl { command: Shell, directory: String },
}

impl ServerRoot {
    pub(crate) fn resolve(self) -> Result<PathBuf> {
        match self {
            Self::Local(directory) => Ok(directory),
            Self::Wsl { command, directory } => resolve_wsl_server_root(&command, &directory),
        }
    }
}

impl Zetta {
    pub(crate) fn active_server_root(&self, cx: &App) -> Result<ServerRoot> {
        let pane = self
            .tabs
            .get(self.active_tab)
            .and_then(Tab::active_pane)
            .context("finding the active pane")?;

        if is_wsl_shell(&pane.profile.command) {
            let directory = pane
                .wsl_working_directory(cx)
                .context("reading the active WSL pane working directory")?;
            return Ok(ServerRoot::Wsl {
                command: pane.profile.command.clone(),
                directory,
            });
        }

        let directory = pane
            .terminal
            .as_ref()
            .and_then(|terminal| terminal.read(cx).working_directory())
            .or_else(|| env::current_dir().ok())
            .context("reading the active pane working directory")?;
        Ok(ServerRoot::Local(directory))
    }
}

#[cfg(windows)]
fn resolve_wsl_server_root(command: &Shell, directory: &str) -> Result<PathBuf> {
    use std::os::windows::process::CommandExt as _;

    const CREATE_NO_WINDOW: u32 = 0x08000000;

    // The built-in servers stay in Zetta's process so their existing lifecycle and log panes
    // continue to work. Ask the pane's distribution for its Windows-accessible (usually UNC)
    // spelling of the Linux directory rather than guessing a distribution mount path.
    let (program, arguments) =
        wsl_path_command(command, directory).context("building the WSL working-directory query")?;
    let output = std::process::Command::new(&program)
        .args(&arguments)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .with_context(|| format!("querying the active WSL directory with {program}"))?;
    anyhow::ensure!(
        output.status.success(),
        "WSL could not expose {directory:?} to Windows: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );

    let path = String::from_utf8(output.stdout).context("WSL returned a non-UTF-8 Windows path")?;
    let path = PathBuf::from(path.trim_end_matches(['\r', '\n']));
    anyhow::ensure!(
        path.is_absolute(),
        "WSL returned a non-absolute Windows path"
    );
    Ok(path)
}

#[cfg(not(windows))]
fn resolve_wsl_server_root(_: &Shell, _: &str) -> Result<PathBuf> {
    anyhow::bail!("WSL server roots are only available on Windows")
}

#[cfg(any(windows, test))]
fn wsl_path_command(command: &Shell, directory: &str) -> Option<(String, Vec<String>)> {
    let (program, arguments) = match command {
        Shell::Program(program) => (program.clone(), Vec::new()),
        Shell::WithArguments { program, args, .. } => (program.clone(), args.clone()),
        Shell::System => return None,
    };
    let exec_index = arguments
        .iter()
        .position(|argument| argument == "--exec" || argument == "-e")
        .unwrap_or(arguments.len());
    let mut global_arguments = Vec::with_capacity(exec_index + 4);
    let mut index = 0;
    while index < exec_index {
        let argument = &arguments[index];
        if argument == "--cd" {
            index += 2;
        } else if argument.starts_with("--cd=") {
            index += 1;
        } else {
            global_arguments.push(argument.clone());
            index += 1;
        }
    }
    global_arguments.extend([
        "--exec".to_owned(),
        "wslpath".to_owned(),
        "-w".to_owned(),
        directory.to_owned(),
    ]);
    Some((program, global_arguments))
}

#[cfg(test)]
#[path = "tests/server_ui.rs"]
mod tests;
