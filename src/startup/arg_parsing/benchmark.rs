//! `zetta benchmark` and `zetta benchmark output`.
//!
//! The workload flags select one producer pattern between them, so a second,
//! different request is rejected rather than letting the last flag on the
//! command line silently win.

use super::*;

/// `zetta benchmark [output] …`
pub(super) fn parse_benchmark_subcommand(arguments: &[OsString]) -> Result<StartupArgs> {
    if arguments
        .first()
        .is_none_or(|argument| argument != "output")
    {
        return parse_benchmark_args(arguments);
    }
    let mut size_mib = None;
    let mut output_type = OutputBenchmarkType::RepeatedLines;
    let mut benchmark_arguments = arguments[1..].iter();
    while let Some(argument) = benchmark_arguments.next() {
        match argument.to_string_lossy().as_ref() {
            "--help" | "-h" => {
                println!("{}", benchmark_output_help());
                std::process::exit(0);
            }
            "--size" | "-s" => {
                anyhow::ensure!(size_mib.is_none(), "--size may only be specified once");
                let value = benchmark_arguments
                    .next()
                    .context("--size requires a number of MiB")?
                    .to_string_lossy()
                    .parse::<usize>()
                    .context("--size must be a whole number of MiB")?;
                anyhow::ensure!(value > 0, "--size must be greater than zero");
                anyhow::ensure!(
                    value.checked_mul(MIB_BYTES).is_some(),
                    "--size is too large"
                );
                size_mib = Some(value);
            }
            "--output-type" | "-t" => {
                let value = benchmark_arguments
                    .next()
                    .context("--output-type requires repeated or unique")?
                    .to_string_lossy();
                output_type = match value.as_ref() {
                    "repeated" => OutputBenchmarkType::RepeatedLines,
                    "unique" => OutputBenchmarkType::UniqueLines,
                    _ => {
                        anyhow::bail!(
                            "--output-type must be either repeated or unique, got {value:?}"
                        )
                    }
                };
            }
            unknown => anyhow::bail!("unknown benchmark output argument {unknown:?}"),
        }
    }
    Ok(StartupArgs::for_mode(StartupMode::OutputBenchmark {
        size_mib: size_mib.unwrap_or(DEFAULT_OUTPUT_BENCHMARK_MIB),
        output_type,
    }))
}

/// Records which producer pattern the benchmark should run. The workload flags
/// select one pattern between them, so this remembers which flag asked for
/// which and rejects a second, different request rather than letting the last
/// flag on the command line silently win.
fn select_benchmark_workload(
    selected: &mut Option<(String, PerformanceWorkload)>,
    flag: &str,
    workload: PerformanceWorkload,
) -> Result<()> {
    if let Some((selected_flag, selected_workload)) = selected
        && *selected_workload != workload
    {
        anyhow::bail!("{selected_flag} cannot be combined with {flag}");
    }
    *selected = Some((flag.to_owned(), workload));
    Ok(())
}

fn parse_benchmark_args(arguments: &[OsString]) -> Result<StartupArgs> {
    let mut mode = StartupMode::TerminalRenderingProfile;
    let mut profile_report = None;
    let mut profile_duration = None;
    let mut profile_pane_stress = false;
    let mut profile_workload = None;
    let mut profile_external_terminal = false;
    let mut args = arguments.iter();
    while let Some(argument) = args.next() {
        match argument.to_string_lossy().as_ref() {
            "--profile-pane-stress" | "-s" => profile_pane_stress = true,
            flag @ ("--profile-background-stress" | "-b") => select_benchmark_workload(
                &mut profile_workload,
                flag,
                PerformanceWorkload::CheckerboardBackground,
            )?,
            flag @ ("--profile-sparse-updates" | "-u") => select_benchmark_workload(
                &mut profile_workload,
                flag,
                PerformanceWorkload::SparseUpdates,
            )?,
            flag @ ("--profile-alt-screen-scroll" | "-a") => select_benchmark_workload(
                &mut profile_workload,
                flag,
                PerformanceWorkload::AltScreenScroll,
            )?,
            "--profile-external-terminal" | "-x" => profile_external_terminal = true,
            // Hidden, and long-only for that reason: these are the child half of
            // `--profile-external-terminal`, which `startup.rs` spells out when
            // it launches the producer in the user's own terminal. The
            // documented way to choose a workload is `--profile-background-
            // stress` and its siblings, which select the same producers.
            "--terminal-render-workload" => mode = StartupMode::TerminalRenderingWorkload,
            "--terminal-checkerboard-workload" => mode = StartupMode::TerminalCheckerboardWorkload,
            "--terminal-sparse-update-workload" => mode = StartupMode::TerminalSparseUpdateWorkload,
            "--terminal-alt-screen-scroll-workload" => {
                mode = StartupMode::TerminalAltScreenScrollWorkload;
            }
            "--profile-report" | "-r" => {
                profile_report = Some(
                    args.next()
                        .context("--profile-report requires a path")?
                        .into(),
                );
            }
            "--profile-duration" | "-d" => {
                let seconds = args
                    .next()
                    .context("--profile-duration requires seconds")?
                    .to_string_lossy()
                    .parse::<f64>()
                    .context("--profile-duration must be a number of seconds")?;
                anyhow::ensure!(
                    seconds.is_finite() && seconds > 0.0,
                    "--profile-duration must be greater than zero"
                );
                profile_duration = Some(Duration::from_secs_f64(seconds));
            }
            "--help" | "-h" => {
                println!("{}", benchmark_help());
                std::process::exit(0);
            }
            unknown => anyhow::bail!("unknown benchmark argument {unknown:?}"),
        }
    }
    anyhow::ensure!(
        !(profile_external_terminal && profile_report.is_some()),
        "--profile-external-terminal cannot be combined with --profile-report"
    );
    anyhow::ensure!(
        !(profile_external_terminal && profile_pane_stress),
        "--profile-external-terminal cannot be combined with --profile-pane-stress"
    );
    anyhow::ensure!(
        !profile_external_terminal || profile_duration.is_some(),
        "--profile-external-terminal requires --profile-duration"
    );
    anyhow::ensure!(
        profile_duration.is_none() || profile_report.is_some() || profile_external_terminal,
        "--profile-duration requires --profile-report or --profile-external-terminal"
    );
    if profile_report.is_some() && profile_duration.is_none() {
        profile_duration = Some(DEFAULT_PERFORMANCE_REPORT_DURATION);
    }
    Ok(StartupArgs {
        config_path: None,
        keymap_path: None,
        profile: None,
        split: None,
        replace_pane: false,
        theme_override: None,
        no_mux: false,
        mode,
        profile_report,
        profile_duration,
        profile_pane_stress,
        profile_workload: profile_workload
            .map(|(_, workload)| workload)
            .unwrap_or_default(),
        profile_external_terminal,
        tftp_command: None,
    })
}

#[cfg(test)]
#[path = "../../tests/startup/arg_parsing/benchmark.rs"]
mod tests;
