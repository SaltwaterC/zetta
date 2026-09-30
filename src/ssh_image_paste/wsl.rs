//! Stages a Windows clipboard image inside a WSL profile's distribution.
//!
//! The auxiliary launcher preserves the profile's distribution, user and other
//! launcher options, but never replays its shell command or CWD tracker. The
//! ordered terminal input worker runs this code, so conversion and WSL I/O do
//! not block the window or let later keystrokes overtake the pasted image.

use super::*;

const WSL_TRANSFER_TIMEOUT: Duration = Duration::from_secs(30);
const STAGE_IMAGE: &str = include_str!("wsl/stage-image.sh");

impl SshImagePasteHandler {
    pub(super) fn stage_wsl_image(&self, image: &Image) -> Result<String> {
        let bytes = normalize_image(image)?;
        let sentinel = next_sentinel();
        let launch = launch_spec(
            &self.execution.environment,
            &self.execution.shell,
            vec![
                "/bin/sh".to_owned(),
                "-c".to_owned(),
                STAGE_IMAGE.to_owned(),
                "zetta-image-paste".to_owned(),
                bytes.len().to_string(),
                sentinel.clone(),
            ],
        );
        let output = run_image_paste_process(launch, bytes, WSL_TRANSFER_TIMEOUT)
            .context("staging clipboard image in WSL")?;
        let path = delimited_value(&output, &sentinel)
            .context("WSL image staging returned no image path")?;
        let path = validate_remote_path(path, &RemotePlatform::Posix)?;
        let directory = remote_directory(&path).context("WSL image path has no directory")?;
        self.cleanup.push(CleanupEntry {
            launch: launch_spec(
                &self.execution.environment,
                &self.execution.shell,
                vec![
                    "/bin/sh".to_owned(),
                    "-c".to_owned(),
                    posix_cleanup_command(&directory),
                ],
            ),
            timeout: WSL_TRANSFER_TIMEOUT,
        });
        Ok(path)
    }
}

pub(super) fn launch_spec(
    environment: &HashMap<String, String>,
    shell: &Shell,
    command: Vec<String>,
) -> LaunchSpec {
    let (program, shell_args) = shell.program_and_args();
    let exec_index = shell_args.iter().position(|argument| {
        argument.eq_ignore_ascii_case("--exec") || argument.eq_ignore_ascii_case("-e")
    });
    let mut args = shell_args[..exec_index.unwrap_or(shell_args.len())].to_vec();
    args.push("--exec".to_owned());
    args.extend(command);
    LaunchSpec {
        program,
        args,
        environment: environment.clone(),
        working_directory: None,
    }
}

pub(super) fn ssh_launch_spec(
    environment: &HashMap<String, String>,
    shell: &Shell,
    invocation: &OpenSshInvocation,
    remote_command: String,
) -> LaunchSpec {
    let command = std::iter::once(invocation.executable.clone())
        .chain(invocation.batch_args(remote_command))
        .collect();
    launch_spec(environment, shell, command)
}

#[cfg(test)]
#[path = "../tests/ssh_image_paste/wsl.rs"]
mod tests;
