//! Starting the multiplexer daemon this client talks to.

use super::*;

/// Starts a detached daemon that outlives this process.
///
/// Zetta passes `None` for `startup_retention`: it applies the configuration
/// through `Configure` after the fresh daemon publishes its endpoint. The
/// standalone disk-resume path supplies a bootstrap mode because it has no
/// Zetta configuration to apply.
#[cfg_attr(
    not(feature = "session-persistence"),
    expect(
        unused_variables,
        reason = "daemon options only carry persistence recipients"
    )
)]
pub(super) fn start_daemon(
    directory: &std::path::Path,
    startup_retention: Option<Retention>,
    #[cfg(feature = "session-persistence")] persistence_recipients: Option<Vec<String>>,
) -> Result<()> {
    let (executable, mut arguments) = multiplexer_command()?;
    append_startup_retention_arguments(&mut arguments, startup_retention);
    #[cfg(feature = "session-persistence")]
    if let Some(recipients) = persistence_recipients
        && !recipients.is_empty()
    {
        crate::catalog::create_private_dir(directory)?;
        let options_path = directory.join(format!("daemon-options-{}.json", std::process::id()));
        let options = serde_json::to_vec(&DaemonOptionsFile { recipients })?;
        crate::catalog::write_private_file(&options_path, &options)?;
        arguments.push(format!("--daemon-options={}", options_path.display()));
    }
    Command::new(&executable)
        .args(&arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("starting the multiplexer {}", executable.display()))?;
    Ok(())
}

/// `zmux --daemon --detach`: starts the daemon with `forwarded` — every other
/// argument that command was given — so that it outlives this process's
/// session, and returns without waiting for it.
pub(crate) fn start_detached_daemon(forwarded: &[std::ffi::OsString]) -> Result<()> {
    let (executable, arguments) = multiplexer_command()?;
    let arguments = arguments
        .into_iter()
        .map(std::ffi::OsString::from)
        .chain(forwarded.iter().cloned())
        .collect::<Vec<_>>();
    crate::daemon_spawn::spawn_detached(&executable, &arguments)
        .with_context(|| format!("starting the multiplexer {}", executable.display()))
}

pub(super) fn append_startup_retention_arguments(
    arguments: &mut Vec<String>,
    retention: Option<Retention>,
) {
    let Some(retention) = retention else {
        return;
    };
    arguments.extend(["--retention".to_owned(), retention.name().to_owned()]);
    if let Retention::Memory { bytes } = retention {
        arguments.extend(["--retention-bytes".to_owned(), bytes.to_string()]);
    }
}

/// How to start the multiplexer: the `zmux` binary beside this executable, or
/// this executable's own `mux` subcommand when there is none.
///
/// Resolved from this process's own location rather than `PATH`, so an
/// unrelated `zmux` earlier in the path cannot be handed a session's terminals.
pub(super) fn multiplexer_command() -> Result<(PathBuf, Vec<String>)> {
    let current = std::env::current_exe().context("locating this executable")?;
    #[cfg(target_os = "macos")]
    if let Some(application) = crate::macos_application::executable(&current) {
        return Ok((application, vec!["mux".to_owned(), "--daemon".to_owned()]));
    }
    let current_name = current
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if current_name == "zmux" || (cfg!(windows) && current_name.eq_ignore_ascii_case("zmux.exe")) {
        return Ok((current, vec!["--daemon".to_owned()]));
    }
    let beside = current.with_file_name(if cfg!(windows) { "zmux.exe" } else { "zmux" });
    if beside.is_file() {
        return Ok((beside, vec!["--daemon".to_owned()]));
    }
    Ok((current, vec!["mux".to_owned(), "--daemon".to_owned()]))
}
