use super::*;
#[cfg(feature = "zmux")]
use crate::mux::MuxPaneIds;
use crate::worktree_detection::terminal_event_requires_worktree_detection;

/// Returns the shell command used to load this process's shell integration
/// into an interactive native shell.  The command is sent after the shell's
/// startup files have completed so a stale `zetta` found earlier on PATH
/// cannot leave the pane with CWD-only tracking.
fn shell_integration_startup_command(shell: &Shell) -> Option<Vec<u8>> {
    let (program, arguments) = shell.program_and_args();
    if zetta_profiles::runs_a_command(arguments) {
        return None;
    }

    let shell_name = Path::new(&program)
        .file_name()?
        .to_string_lossy()
        .to_ascii_lowercase();
    #[cfg(not(windows))]
    let command = match shell_name.as_str() {
        "bash" | "bash.exe" => {
            r#"if [[ ${__ZETTA_LIFECYCLE_TRACKING_INSTALLED:-0} != 1 || ${__ZETTA_LIFECYCLE_TRACKING_ENABLED:-0} != 1 ]]; then eval "$(command zetta init bash)"; fi"#
        }
        "zsh" | "zsh.exe" => {
            r#"if [[ ${__ZETTA_LIFECYCLE_TRACKING_VERSION:-0} != 3 || ( -n ${ZETTA_PANE_ROUTING_ID:-${ZETTA_PANE_ID:-}} && ${__ZETTA_LIFECYCLE_TRACKING_ENABLED:-0} != 1 ) ]]; then eval "$(command zetta init zsh)"; fi"#
        }
        "fish" | "fish.exe" => {
            r#"if not set -q __ZETTA_LIFECYCLE_TRACKING_INSTALLED; or test "$__ZETTA_LIFECYCLE_TRACKING_ENABLED" != 1; command zetta init fish | source; end"#
        }
        _ => return None,
    };
    #[cfg(windows)]
    let command = match shell_name.as_str() {
        "powershell" | "powershell.exe" | "pwsh" | "pwsh.exe" => {
            r#"if (-not $global:__ZettaLifecycleTrackerInstalled -or -not $global:__ZettaLifecycleTrackingEnabled) { & $env:ZETTA_HOST_EXECUTABLE init powershell | Out-String | Invoke-Expression }"#
        }
        _ => return None,
    };

    let mut command = command.as_bytes().to_vec();
    command.push(b'\r');
    Some(command)
}

#[derive(Clone)]
pub(crate) struct RestoredTerminalOptions {
    replay: Option<Vec<u8>>,
    prefill: Option<String>,
}
/// What wiring a stacked command's terminal needs once its builder resolves.
///
/// The stacked equivalent of [`SpawnedTerminal`]: a stacked entry belongs to a
/// pane's stack rather than to the pane itself, so it carries the entry it is
/// becoming. The task state that reports the command's exit is consumed by the
/// builder and does not travel with it.
struct SpawnedStackedTerminal {
    tab_id: u64,
    pane_id: u64,
    entry_id: u64,
    attention_id: u64,
    pane_routing_id: u64,
    terminal_theme: Option<Arc<Theme>>,
    mux_provider: Option<Arc<crate::mux::MuxPtyProvider>>,
    image_paste_handler: Arc<crate::ssh_image_paste::SshImagePasteHandler>,
}

/// What the spawn callback still needs once the terminal builder resolves.
///
/// The builder is awaited off the render path, so everything the wiring after
/// it depends on has to cross into that callback. These are those values, as
/// one bundle rather than eleven captured locals — which is also what lets the
/// success and failure paths be named functions rather than two arms of a
/// closure.
struct SpawnedTerminal {
    tab_id: u64,
    pane_id: u64,
    /// Reported to shell integration so `zetta attention` can address the pane.
    attention_id: u64,
    pane_routing_id: u64,
    terminal_theme: Option<Arc<Theme>>,
    restore_options: Option<RestoredTerminalOptions>,
    /// Only set while restoring: the directory the stored session was in.
    restored_working_directory: Option<PathBuf>,
    mux_provider: Option<Arc<crate::mux::MuxPtyProvider>>,
    #[cfg(feature = "zmux")]
    shared_pane: Option<Arc<zmux::client::SharedPane>>,
    #[cfg(feature = "zmux")]
    shared_runtime: Option<crate::mux::MuxRuntime>,
    #[cfg(feature = "zmux")]
    shared_state: Option<zmux::messages::SharedSessionState>,
    shell_integration_startup_command: Option<Vec<u8>>,
    tracked_multi_command_launch: bool,
    image_paste_handler: Option<Arc<dyn terminal::ImagePasteHandler>>,
}

struct LocalTerminalLaunch {
    tab_id: u64,
    pane_id: u64,
    attention_id: u64,
    pane_routing_id: u64,
    terminal_theme: Option<Arc<Theme>>,
    shell: Shell,
    environment: HashMap<String, String>,
    working_directory: Option<PathBuf>,
    path_hyperlink_regexes: Vec<String>,
    restore_options: Option<RestoredTerminalOptions>,
    mux_provider: Option<Arc<crate::mux::MuxPtyProvider>>,
    initial_console_palette: Option<terminal::ConsolePalette>,
    shell_integration_startup_command: Option<Vec<u8>>,
    tracked_multi_command_launch: bool,
}

#[cfg(feature = "zmux")]
#[derive(Clone)]
pub(crate) struct SharedTerminalLaunch {
    tab_id: u64,
    pane_id: u64,
    attention_id: u64,
    pane_routing_id: u64,
    terminal_theme: Option<Arc<Theme>>,
    provider: Arc<crate::mux::MuxPtyProvider>,
    shell: Shell,
    environment: HashMap<String, String>,
    working_directory: Option<PathBuf>,
    title: String,
    profile_name: String,
    cursor_shape: terminal::terminal_settings::CursorShape,
    alternate_scroll: terminal::terminal_settings::AlternateScroll,
    max_scroll_history_lines: Option<usize>,
    shell_integration_startup_command: Option<Vec<u8>>,
    tracked_multi_command_launch: bool,
    base_revision: zmux::messages::SessionRevision,
    console_palette: terminal::ConsolePalette,
    /// Whether this launch has already been resynchronised once. A session
    /// whose daemon was replaced leaves every window proposing pane ids that no
    /// longer exist; the first refusal brings the window up to date and the
    /// launch is made again, but only once, so a session that refuses for some
    /// other reason cannot put the window in a loop.
    resynchronized: bool,
}

#[cfg(feature = "zmux")]
enum SharedTerminalBuild {
    Ready(Box<TerminalBuilder>, Box<SpawnedTerminal>),
    Conflict {
        tab_id: u64,
        pane_id: u64,
        state: Box<zmux::messages::SharedSessionState>,
    },
    /// The session refused the proposed geometry, and a fresh snapshot came
    /// back with it. This window's picture of the session was wrong, so the
    /// snapshot is applied before the draft is given up — otherwise the same
    /// wrong picture produces the same refusal for every later split.
    Desynchronized {
        tab_id: u64,
        pane_id: u64,
        state: Box<zmux::messages::SharedSessionState>,
        reason: String,
    },
}

#[cfg(feature = "zmux")]
enum SharedTerminalBatchBuild {
    Ready {
        session_id: u64,
        state: zmux::messages::SharedSessionState,
        terminals: Vec<(TerminalBuilder, SpawnedTerminal)>,
    },
    Conflict {
        state: zmux::messages::SharedSessionState,
        panes: Vec<(u64, u64)>,
    },
    /// As [`SharedTerminalBuild::Desynchronized`]: the panes are the session's
    /// now, whatever went wrong here, so the window converges on the committed
    /// state rather than dropping them.
    Desynchronized {
        state: zmux::messages::SharedSessionState,
        panes: Vec<(u64, u64)>,
        reason: String,
    },
}

#[cfg(feature = "zmux")]
fn build_shared_terminal_batch(
    launches: Vec<SharedTerminalLaunch>,
    runtime: crate::mux::MuxRuntime,
    request: zmux::messages::SharedSpawnBatchRequest,
    terminal_executor: &gpui::BackgroundExecutor,
) -> Result<SharedTerminalBatchBuild> {
    let session_id = request.session_id;
    let client = runtime.client().clone();
    // Taken before the launches are consumed below: every recovery path has to
    // name the drafts, including the ones that run after the loop has started.
    let panes = launches
        .iter()
        .map(|launch| (launch.tab_id, launch.pane_id))
        .collect::<Vec<_>>();
    let result = match client.spawn_shared_batch(request) {
        Ok(result) => result,
        Err(error) => {
            // The session would not take this geometry. Bring back what it
            // actually holds so the window stops proposing the same thing.
            let state = client.shared_snapshot(session_id)?;
            return Ok(SharedTerminalBatchBuild::Desynchronized {
                state,
                panes,
                reason: format!("{error:#}"),
            });
        }
    };
    let mut committed = match result {
        zmux::client::SharedBatchResult::Applied(committed) => committed,
        zmux::client::SharedBatchResult::Conflict(state) => {
            return Ok(SharedTerminalBatchBuild::Conflict { state, panes });
        }
    };
    let mut terminals = Vec::with_capacity(launches.len());
    for launch in launches {
        let mapping = committed
            .mappings
            .iter()
            .find(|mapping| mapping.draft_id == launch.pane_id)
            .with_context(|| format!("shared batch omitted draft pane {}", launch.pane_id))?;
        launch
            .provider
            .record_shared_opened(session_id, mapping.pane_id);
        // Committed already: the panes belong to the session whatever happens
        // here, so a failed attachment converges the window on them instead of
        // dropping panes that are running for every other viewer.
        let pane = match attach_committed_shared_pane(
            &client,
            &runtime,
            session_id,
            mapping.pane_id,
            &mut committed.state,
        ) {
            Ok(pane) => pane,
            Err(error) => {
                return Ok(SharedTerminalBatchBuild::Desynchronized {
                    state: committed.state,
                    panes,
                    reason: format!("{error:#}"),
                });
            }
        };
        let pane = Arc::new(pane);
        let image_paste_handler: Arc<dyn terminal::ImagePasteHandler> = if runtime.is_remote() {
            Arc::new(
                crate::background_session_ui::image_paste::RemoteImagePasteHandler::new(
                    &runtime,
                    session_id,
                    mapping.pane_id,
                ),
            )
        } else {
            Arc::new(crate::ssh_image_paste::SshImagePasteHandler::new(
                launch.shell.clone(),
                launch.environment.clone(),
                launch.working_directory.clone(),
            ))
        };
        let builder = TerminalBuilder::new_byte_stream(
            Box::new(pane.reader()),
            Box::new(
                crate::background_session_ui::shared_panes::SharedPaneWriter { pane: pane.clone() },
            ),
            launch.title,
            launch.cursor_shape,
            launch.alternate_scroll,
            launch.max_scroll_history_lines,
            0,
            terminal_executor,
            PathStyle::local(),
        )
        .with_working_directory(launch.working_directory)
        .with_replay(pane.replay.clone())
        .with_pty_control(crate::mux::mux_pty_control_with_secret(
            client.clone(),
            session_id,
            mapping.pane_id,
            runtime.session_secret(),
        ))
        .with_image_paste_handler(image_paste_handler)
        .with_init_command_startup_shell(launch.shell);
        terminals.push((
            builder,
            SpawnedTerminal {
                tab_id: launch.tab_id,
                pane_id: launch.pane_id,
                attention_id: launch.attention_id,
                pane_routing_id: launch.pane_routing_id,
                terminal_theme: launch.terminal_theme,
                restore_options: None,
                restored_working_directory: None,
                mux_provider: None,
                shared_pane: Some(pane),
                shared_runtime: Some(runtime.clone()),
                shared_state: None,
                shell_integration_startup_command: launch.shell_integration_startup_command,
                tracked_multi_command_launch: launch.tracked_multi_command_launch,
                image_paste_handler: None,
            },
        ));
    }
    Ok(SharedTerminalBatchBuild::Ready {
        session_id,
        state: committed.state,
        terminals,
    })
}

#[cfg(feature = "zmux")]
fn attach_committed_shared_pane(
    client: &Arc<zmux::client::Client>,
    runtime: &crate::mux::MuxRuntime,
    session_id: u64,
    pane_id: u64,
    state: &mut zmux::messages::SharedSessionState,
) -> Result<zmux::client::SharedPane> {
    let mut last_error = None;
    for attempt in 0..3 {
        if attempt > 0 {
            *state = client.shared_snapshot(session_id)?;
            anyhow::ensure!(
                state.contains_pane(pane_id),
                "committed shared pane {pane_id} was removed before it could attach"
            );
        }
        match client.attach_shared_with_secret(
            session_id,
            pane_id,
            runtime.session_secret().as_ref(),
        ) {
            Ok(zmux::client::AttachOutcome::SharedAttached { pane, .. }) => return Ok(pane),
            Ok(_) => {
                last_error = Some(anyhow::anyhow!(
                    "committed shared pane did not attach as a shared stream"
                ));
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.expect("three shared attach attempts produce an outcome"))
        .with_context(|| format!("attaching committed shared pane {pane_id}"))
}

/// What a shared pane draft may say about the process it asks for.
///
/// A pane in a shared session is started by the daemon, on the daemon's host.
/// When that host is this machine, this window resolved the profile against the
/// very configuration the daemon would read, and its resolution carries more
/// than a name can — the working-directory tracking wrappers a WSL, MSYS2 or
/// Cygwin profile needs — so it is sent and used as-is.
///
/// When the host is another machine, none of that holds. The program path, the
/// `PATH` around it and the tracking files all name this machine, and sending
/// them is exactly how a Linux client came to start macOS's `/bin/bash` 3.2
/// with a Linux `PATH` in front of it. Only the profile name and the pane's
/// routing identity cross; the daemon resolves the rest.
#[cfg(feature = "zmux")]
fn shared_draft_process(
    remote: bool,
    shell: &Shell,
    environment: &HashMap<String, String>,
) -> (
    Option<zetta_profiles::ProfileCommand>,
    HashMap<String, String>,
) {
    if remote {
        (
            None,
            zmux::messages::shared_draft_environment(environment.clone()),
        )
    } else {
        (
            Some(crate::config::profile_command(shell)),
            environment.clone(),
        )
    }
}

/// The size a pane's pty starts at, before any viewer has laid the pane out and
/// reported a real one.
///
/// The daemon arbitrates a shared pane down to the smallest of its viewers, and
/// falls back to this while none of them has measured. A fixed 80x24 meant a
/// new pane's first screens were drawn — and wrapped — at a width no window was
/// showing, so the source pane's own size is used instead: a split leaves the
/// two halves near enough that the reflow, when the real sizes arrive, is
/// small.
#[cfg(feature = "zmux")]
fn shared_spawn_stand_in_size(
    tab: Option<&Tab>,
    source_pane_id: u64,
    cx: &App,
) -> zmux::messages::TerminalSize {
    let measured = tab
        .and_then(|tab| tab.pane(source_pane_id))
        .and_then(TerminalPane::selected_terminal)
        .map(|terminal| terminal.read(cx).last_content().terminal_bounds)
        .filter(|bounds| bounds.num_columns() > 0 && bounds.num_lines() > 0);
    zmux::messages::TerminalSize {
        columns: measured.map_or(80, |bounds| bounds.num_columns() as u16),
        lines: measured.map_or(24, |bounds| bounds.num_lines() as u16),
        cell_width: 0,
        cell_height: 0,
    }
}

/// Every pane a proposed layout names as already existing.
///
/// These are the ids the session is asked to place, translated from this
/// window's own map. The daemon refuses the whole request if one of them names
/// a pane it does not hold, which is why they are worth checking first.
#[cfg(feature = "zmux")]
fn shared_draft_layout_existing_ids(
    layout: &zmux::messages::SharedDraftLayout,
    ids: &mut Vec<u64>,
) {
    match layout {
        zmux::messages::SharedDraftLayout::Draft { .. } => {}
        zmux::messages::SharedDraftLayout::Existing { pane_id } => ids.push(*pane_id),
        zmux::messages::SharedDraftLayout::Split { first, second, .. } => {
            shared_draft_layout_existing_ids(first, ids);
            shared_draft_layout_existing_ids(second, ids);
        }
    }
}

#[cfg(feature = "zmux")]
fn shared_draft_layout(
    layout: &PaneLayout,
    draft_pane_id: u64,
    mux_ids: &HashMap<u64, u64>,
) -> Result<zmux::messages::SharedDraftLayout> {
    Ok(match layout {
        PaneLayout::Pane(pane_id) if *pane_id == draft_pane_id => {
            zmux::messages::SharedDraftLayout::Draft {
                draft_id: draft_pane_id,
            }
        }
        PaneLayout::Pane(pane_id) => zmux::messages::SharedDraftLayout::Existing {
            pane_id: *mux_ids
                .get(pane_id)
                .with_context(|| format!("shared pane {pane_id} has no multiplexer id"))?,
        },
        PaneLayout::Split {
            axis,
            first_ratio,
            first,
            second,
        } => zmux::messages::SharedDraftLayout::Split {
            axis: match axis {
                SplitAxis::Horizontal => "horizontal",
                SplitAxis::Vertical => "vertical",
            }
            .to_owned(),
            first_ratio: *first_ratio,
            first: Box::new(shared_draft_layout(first, draft_pane_id, mux_ids)?),
            second: Box::new(shared_draft_layout(second, draft_pane_id, mux_ids)?),
        },
    })
}

#[cfg(feature = "zmux")]
fn shared_complete_draft_layout(
    layout: &PaneLayout,
    draft_pane_ids: &HashSet<u64>,
    mux_ids: &HashMap<u64, u64>,
) -> Result<zmux::messages::SharedDraftLayout> {
    Ok(match layout {
        PaneLayout::Pane(pane_id) if draft_pane_ids.contains(pane_id) => {
            zmux::messages::SharedDraftLayout::Draft { draft_id: *pane_id }
        }
        PaneLayout::Pane(pane_id) => zmux::messages::SharedDraftLayout::Existing {
            pane_id: *mux_ids
                .get(pane_id)
                .with_context(|| format!("shared pane {pane_id} has no multiplexer id"))?,
        },
        PaneLayout::Split {
            axis,
            first_ratio,
            first,
            second,
        } => zmux::messages::SharedDraftLayout::Split {
            axis: match axis {
                SplitAxis::Horizontal => "horizontal",
                SplitAxis::Vertical => "vertical",
            }
            .to_owned(),
            first_ratio: *first_ratio,
            first: Box::new(shared_complete_draft_layout(
                first,
                draft_pane_ids,
                mux_ids,
            )?),
            second: Box::new(shared_complete_draft_layout(
                second,
                draft_pane_ids,
                mux_ids,
            )?),
        },
    })
}

#[cfg(feature = "zmux")]
fn shared_spawn_replacement(
    layout: &PaneLayout,
    draft_pane_id: u64,
    mux_ids: &HashMap<u64, u64>,
) -> Result<(Option<u64>, zmux::messages::SharedDraftLayout)> {
    match layout {
        PaneLayout::Split { first, second, .. }
            if matches!(first.as_ref(), PaneLayout::Pane(id) if *id == draft_pane_id)
                && matches!(second.as_ref(), PaneLayout::Pane(_)) =>
        {
            let PaneLayout::Pane(target) = second.as_ref() else {
                unreachable!()
            };
            Ok((
                Some(
                    *mux_ids
                        .get(target)
                        .context("shared split target has no mux id")?,
                ),
                shared_draft_layout(layout, draft_pane_id, mux_ids)?,
            ))
        }
        PaneLayout::Split { first, second, .. }
            if matches!(second.as_ref(), PaneLayout::Pane(id) if *id == draft_pane_id)
                && matches!(first.as_ref(), PaneLayout::Pane(_)) =>
        {
            let PaneLayout::Pane(target) = first.as_ref() else {
                unreachable!()
            };
            Ok((
                Some(
                    *mux_ids
                        .get(target)
                        .context("shared split target has no mux id")?,
                ),
                shared_draft_layout(layout, draft_pane_id, mux_ids)?,
            ))
        }
        PaneLayout::Split { first, second, .. } => {
            if pane_layout_contains(first, draft_pane_id) {
                shared_spawn_replacement(first, draft_pane_id, mux_ids)
            } else if pane_layout_contains(second, draft_pane_id) {
                shared_spawn_replacement(second, draft_pane_id, mux_ids)
            } else {
                anyhow::bail!("shared draft pane is not in the tab layout")
            }
        }
        PaneLayout::Pane(_) => Ok((None, shared_draft_layout(layout, draft_pane_id, mux_ids)?)),
    }
}

#[cfg(feature = "zmux")]
fn pane_layout_contains(layout: &PaneLayout, pane_id: u64) -> bool {
    match layout {
        PaneLayout::Pane(id) => *id == pane_id,
        PaneLayout::Split { first, second, .. } => {
            pane_layout_contains(first, pane_id) || pane_layout_contains(second, pane_id)
        }
    }
}

/// Everything one interactive-terminal spawn needs. The tab, the pane and the
/// profile are always known; every other input has a default, so a caller
/// names only what it varies. This replaced a ladder of forwarding
/// constructors that differed from each other by one defaulted argument.
pub(crate) struct TerminalSpawnRequest {
    pub(crate) tab_id: u64,
    pub(crate) pane_id: u64,
    pub(crate) profile: Profile,
    /// The shell to run. `None` derives it from `profile.command`, wrapping it
    /// in the WSL or Cygwin CWD-tracking launcher when the profile needs one.
    /// A caller that has already built a shell — a `zetta pane` command, say —
    /// passes it here, and both the derivation and `wsl_directory`, which only
    /// feeds it, are skipped.
    pub(crate) shell: Option<Shell>,
    pub(crate) working_directory: Option<PathBuf>,
    pub(crate) wsl_directory: Option<String>,
    pub(crate) wsl_cwd_file: Option<PathBuf>,
    pub(crate) terminal_theme: Option<Arc<Theme>>,
    pub(crate) path_hyperlink_regexes: Vec<String>,
    /// Environment overrides layered over the tab's project environment.
    pub(crate) environment: HashMap<String, String>,
    pub(crate) tracked_multi_command_launch: bool,
    /// Set through [`TerminalSpawnRequest::restored`]; only visible so that
    /// callers can use struct-update syntax against `new`.
    pub(crate) restore: Option<RestoredTerminalOptions>,
}

impl TerminalSpawnRequest {
    pub(crate) fn new(tab_id: u64, pane_id: u64, profile: Profile) -> Self {
        Self {
            tab_id,
            pane_id,
            profile,
            shell: None,
            working_directory: None,
            wsl_directory: None,
            wsl_cwd_file: None,
            terminal_theme: None,
            path_hyperlink_regexes: Vec::new(),
            environment: HashMap::new(),
            tracked_multi_command_launch: false,
            restore: None,
        }
    }

    /// Starts the shell in a daemon-created restore session. The saved screen
    /// is handed to the provider as a one-shot replay, while the shell itself
    /// is always created by the daemon from the saved profile and CWD.
    #[cfg(feature = "session-persistence")]
    pub(crate) fn restored(mut self, replay: Option<Vec<u8>>, prefill: Option<String>) -> Self {
        self.restore = Some(RestoredTerminalOptions { replay, prefill });
        self
    }
}

/// The inputs the environment of a spawned terminal is built from.
struct TerminalEnvironment<'a> {
    /// The profile's own command, not the shell derived from it: the CWD
    /// tracking a profile needs is chosen from how the user configured it.
    profile: &'a Shell,
    /// Overrides layered over the inherited environment. `ZETTA_`-prefixed
    /// names are ignored; see [`apply_terminal_environment_overrides`].
    overrides: &'a HashMap<String, String>,
    attention_id: u64,
    /// The terminal's identity for shell integration: the pane id for an
    /// interactive shell, the stack entry id for a command terminal.
    tracking_id: u64,
    routing_id: u64,
    wsl_cwd_file: Option<&'a Path>,
    theme_name: &'a str,
    no_mux: bool,
}

impl TerminalEnvironment<'_> {
    /// Builds the environment: the inherited native environment plus the
    /// profile's CWD-tracking variables, the caller's overrides, and the
    /// pane's routing identity. Errors already carry the "Could not configure
    /// … CWD tracking" context a pane's error field wants.
    ///
    /// The hasher is the caller's: the root package does not depend on Zed's
    /// `collections` crate, so the map's `FxBuildHasher` can only be inferred
    /// from the [`TerminalBuilder`] the environment is handed to.
    fn build<S>(self) -> Result<HashMap<String, String, S>>
    where
        S: std::hash::BuildHasher + Default,
    {
        let is_wsl = is_wsl_shell(self.profile);
        let mut environment = if is_wsl {
            HashMap::default()
        } else {
            let native_environment = native_terminal_environment();
            #[cfg(windows)]
            let inherited_path = native_environment
                .iter()
                .find(|(name, _)| name == "PATH")
                .map(|(_, value)| value.clone());
            let msys2_environment =
                msys2_cwd_tracking_environment(self.profile, self.tracking_id, &env::temp_dir())
                    .context("Could not configure MSYS2 CWD tracking")?;
            #[cfg(windows)]
            let cygwin_environment = cygwin_cwd_tracking_environment_with_path(
                self.profile,
                self.tracking_id,
                &env::temp_dir(),
                inherited_path.as_deref(),
            )
            .context("Could not configure Cygwin CWD tracking")?;
            #[cfg(not(windows))]
            let cygwin_environment = Vec::new();
            native_environment
                .into_iter()
                .chain(msys2_environment)
                .chain(cygwin_environment)
                .collect()
        };
        if is_wsl {
            wsl_terminal_environment(&mut environment, self.wsl_cwd_file);
        }
        apply_terminal_environment_overrides(
            &mut environment,
            self.overrides,
            std::process::id(),
            self.attention_id,
            self.tracking_id,
            self.routing_id,
            self.no_mux,
        );
        #[cfg(windows)]
        ensure_cygwin_environment(self.profile, &mut environment);
        environment.insert("ZETTA_THEME".to_owned(), self.theme_name.to_owned());
        if is_wsl {
            add_wsl_environment_variable_names(
                &mut environment,
                self.overrides.keys().map(String::as_str),
            );
            add_wsl_environment_variables(&mut environment);
        }
        Ok(environment)
    }
}

impl Zetta {
    /// Spawns the terminal for a pane, resolving the pane's theme and this
    /// process's current terminal settings first — so `terminal_theme` and
    /// `path_hyperlink_regexes` are filled in here and anything the caller put
    /// in them is replaced. A caller that spawns a batch of panes resolves both
    /// once for the batch and calls [`Zetta::spawn_terminal`] instead.
    pub(crate) fn spawn_terminal_for_pane(
        &mut self,
        mut request: TerminalSpawnRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(terminal_theme) =
            self.resolve_pane_spawn_theme(request.tab_id, request.pane_id, &request.profile, cx)
        else {
            return;
        };
        let mut settings = TerminalSpawnSettings::current(cx);
        request.path_hyperlink_regexes = settings.path_hyperlink_regexes(true);
        request.terminal_theme = terminal_theme;
        self.spawn_terminal(request, &settings, window, cx);
    }

    /// Resolves the theme a pane's terminal starts with. `None` means the
    /// failure has already been reported on the pane and the spawn must stop.
    fn resolve_pane_spawn_theme(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        profile: &Profile,
        cx: &mut Context<Self>,
    ) -> Option<Option<Arc<Theme>>> {
        let (pane_theme_override, tab_theme_override) = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map_or((None, None), |tab| {
                (
                    tab.pane(pane_id)
                        .and_then(|pane| pane.theme_override.as_deref()),
                    tab.theme_override.as_deref(),
                )
            });
        match resolve_terminal_theme(
            pane_theme_override,
            tab_theme_override,
            profile,
            self.project_config_for_tab(tab_id).map(Arc::as_ref),
            cx,
        ) {
            Ok(theme) => Some(theme),
            Err(error) => {
                self.report_pane_spawn_error(
                    tab_id,
                    pane_id,
                    format!("Could not apply profile theme: {error:#}"),
                    cx,
                );
                None
            }
        }
    }

    /// Reports a synchronous spawn failure on the pane the terminal was meant
    /// for. Failures raised once the spawn is asynchronous instead coalesce
    /// their redraw through [`Zetta::schedule_terminal_spawn_notify`].
    pub(crate) fn report_pane_spawn_error(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        error: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(pane) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane_mut(pane_id))
        {
            pane.error = Some(error);
        }
        cx.notify();
    }

    #[cfg(windows)]
    pub(crate) fn spawn_windows_handoff_terminal(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        profile: Profile,
        request: crate::windows_integration::WindowsHandoffRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(terminal_theme) = self.resolve_pane_spawn_theme(tab_id, pane_id, &profile, cx)
        else {
            return;
        };
        let mut settings = TerminalSpawnSettings::current(cx);
        let path_hyperlink_regexes = settings.path_hyperlink_regexes(true);
        let child_handle = match request.duplicate_child_handle() {
            Ok(handle) => handle,
            Err(error) => {
                self.report_pane_spawn_error(
                    tab_id,
                    pane_id,
                    format!("Could not monitor the handed-over process: {error}"),
                    cx,
                );
                return;
            }
        };
        let options = terminal::AttachedOptions {
            shell: profile.command.clone(),
            env: native_terminal_environment().into_iter().collect(),
            cursor_shape: settings.cursor_shape,
            alternate_scroll: settings.alternate_scroll,
            max_scroll_history_lines: settings.max_scroll_history_lines,
            path_hyperlink_regexes,
            path_hyperlink_timeout_ms: settings.path_hyperlink_timeout_ms,
            window_id: cx.entity_id().as_u64(),
        };
        let image_paste_handler = Arc::new(crate::ssh_image_paste::SshImagePasteHandler::new(
            options.shell.clone(),
            options.env.clone(),
            None,
        ));
        let handover = request.into_handover();
        let run_identity = self.run_pane_identity(tab_id, pane_id);
        let build_executor = cx.background_executor().clone();
        let terminal_executor = build_executor.clone();
        let build = build_executor.spawn(async move {
            TerminalBuilder::new_attached(handover, options, &terminal_executor, PathStyle::local())
        });
        let this = cx.entity().downgrade();
        let terminal_theme_for_task = terminal_theme.clone();
        window
            .spawn(cx, async move |cx| match build.await {
                Ok(attached) => {
                    let terminal::AttachedTerminal {
                        builder,
                        child_events,
                    } = attached;
                    let builder = builder.with_image_paste_handler(image_paste_handler);
                    crate::windows_integration::monitor_handoff_child(child_handle, child_events);
                    this.update_in(cx, |this, window, cx| {
                        let terminal = cx.new(|cx| builder.subscribe(cx));
                        let view = cx.new(|cx| {
                            TerminalView::new_with_theme(
                                terminal.clone(),
                                terminal_theme_for_task,
                                window,
                                cx,
                            )
                        });
                        this.configure_terminal_view_silent_mode(tab_id, &view, cx);
                        this.subscribe_spawned_terminal(
                            tab_id,
                            pane_id,
                            run_identity,
                            &terminal,
                            window,
                            cx,
                        );
                        this.subscribe_spawned_terminal_view(tab_id, pane_id, &view, window, cx);
                        let should_focus = this
                            .configure_spawned_terminal_focus(tab_id, pane_id, &view, window, cx);
                        let tab_index = this.tabs.iter().position(|tab| tab.id == tab_id);
                        if let Some(pane) = tab_index
                            .and_then(|index| this.tabs.get_mut(index))
                            .and_then(|tab| tab.pane_mut(pane_id))
                        {
                            pane.terminal = Some(terminal.clone());
                            pane.view = Some(view.clone());
                            pane.base_exited = false;
                            pane.error = None;
                            pane.exit = None;
                        }
                        this.schedule_worktree_detection_for_pane(tab_id, pane_id, cx);
                        this.schedule_project_detection_for_pane(tab_id, pane_id, window, cx);
                        if should_focus {
                            let focus_handle = view.focus_handle(cx);
                            this.focus_terminal_if_allowed(&focus_handle, window, cx);
                        }
                        this.sync_visible_terminals(cx);
                        this.schedule_terminal_spawn_notify(cx);
                    })
                    .ok();
                }
                Err(error) => {
                    this.update_in(cx, |this, _, cx| {
                        if let Some(pane) = this
                            .tabs
                            .iter_mut()
                            .find(|tab| tab.id == tab_id)
                            .and_then(|tab| tab.pane_mut(pane_id))
                        {
                            pane.error = Some(format!("{error:#}"));
                        }
                        this.schedule_terminal_spawn_notify(cx);
                    })
                    .ok();
                }
            })
            .detach();
    }

    /// Wires a freshly built terminal to the pane that owns it: the run
    /// registry's view of the pane, unexpected exits, resize requests, and the
    /// grid-size notify the cached title bar depends on.
    ///
    /// Shared with the Windows console handover, which builds its terminal from
    /// an inherited console rather than by spawning one but observes it
    /// identically. `run_identity` is absent only for a pane whose shell
    /// integration cannot report, which is what the handover starts as.
    fn subscribe_spawned_terminal(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        run_identity: Option<crate::run_command::RunPaneIdentity>,
        terminal: &Entity<Terminal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let run_registry = crate::run_command::process_run_registry();
        if let Some(identity) = run_identity {
            run_registry.pane_reopened(identity);
        }
        cx.subscribe_in(
            terminal,
            window,
            move |this, _, event: &TerminalEvent, window, cx| {
                if let Some(identity) = run_identity {
                    match event {
                        TerminalEvent::TrackingReady => run_registry.tracking_ready(identity),
                        TerminalEvent::CommandStarted { command } => {
                            run_registry.command_started(identity, command.clone());
                            this.update_active_command(
                                tab_id,
                                pane_id,
                                crate::session_state::valid_restore_command(command),
                            );
                        }
                        TerminalEvent::CommandFinished { exit_code } => {
                            run_registry.command_finished(identity, *exit_code);
                            this.update_active_command(tab_id, pane_id, None);
                        }
                        TerminalEvent::TerminalExited(_) => {
                            run_registry.terminal_lost(identity);
                            this.update_active_command(tab_id, pane_id, None);
                        }
                        _ => {}
                    }
                }
                match event {
                    TerminalEvent::TerminalExited(exit)
                        if exit.is_unexpected()
                            && this.retain_unexpected_terminal_exit(tab_id, pane_id, exit, cx) =>
                    {
                        this.publish_background_session_catalog(cx);
                        this.sync_visible_terminals(cx);
                        this.focus_active(window, cx);
                    }
                    TerminalEvent::ResizeRequested { rows, columns } => {
                        this.resize_pane_to(
                            tab_id,
                            pane_id,
                            Some(*columns),
                            Some(*rows),
                            window,
                            cx,
                        );
                    }
                    // The title bar reports the active pane's grid size, and
                    // it renders inside a cached boundary that only a notify
                    // on `Zetta` busts. Terminal output must not reach here;
                    // only an actual change of the grid's dimensions does.
                    TerminalEvent::GridSizeChanged => cx.notify(),
                    event if terminal_event_requires_worktree_detection(event) => {
                        // A program can change the terminal's ordinary OSC
                        // title without changing its process metadata. Treat it
                        // as a worktree-detection trigger too, so that a title
                        // such as Codex's `switched-source` cannot become the
                        // tab title while the shell remains in a linked
                        // worktree.
                        this.schedule_worktree_detection_for_pane(tab_id, pane_id, cx);
                        this.schedule_project_detection_for_pane(tab_id, pane_id, window, cx);
                        cx.notify();
                    }
                    _ => {}
                }
            },
        )
        .detach();
    }

    /// Wires the view built over that terminal: close, title changes,
    /// broadcast input, and the editor a pane opens.
    fn subscribe_spawned_terminal_view(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        view: &Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.subscribe_in(
            view,
            window,
            move |this, _, event, window, cx| match event {
                TerminalViewEvent::Close => {
                    this.terminal_closed(tab_id, pane_id, window, cx);
                }
                TerminalViewEvent::TitleChanged => {
                    this.schedule_worktree_detection_for_pane(tab_id, pane_id, cx);
                    this.schedule_project_detection_for_pane(tab_id, pane_id, window, cx);
                    cx.notify();
                }
                TerminalViewEvent::PasteError(error) => this.show_notice(error.clone(), cx),
                TerminalViewEvent::Input(input) => {
                    this.broadcast_input(tab_id, pane_id, input, cx);
                }
                TerminalViewEvent::OpenEditor(request) => {
                    this.open_editor_in_new_pane(tab_id, pane_id, request.clone(), window, cx);
                }
            },
        )
        .detach();
    }

    /// Applies the tab's input state to a new view, routes its focus, and
    /// reports whether this pane is the one that should take focus now.
    ///
    /// Answered before the view is stored on the pane, because it asks what the
    /// tab looked like when the spawn was requested: a pane that is no longer
    /// active by the time its terminal arrives must not steal focus.
    fn configure_spawned_terminal_focus(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        view: &Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let this = self;
        let focus_handle = view.focus_handle(cx);
        let emit_input_events = this
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .is_some_and(|tab| tab.broadcast_input);
        let input_enabled = this.terminal_input_enabled();
        view.update(cx, |view, cx| {
            view.set_emit_input_events(emit_input_events);
            view.set_input_enabled(input_enabled, cx);
        });
        cx.on_focus_in(&focus_handle, window, move |this, window, cx| {
            if let Some(tab) = this
                .tabs
                .iter_mut()
                .find(|tab| tab.id == tab_id)
                .filter(|tab| tab.pane(pane_id).is_some_and(|pane| !pane.base_exited))
            {
                tab.activate_stack_entry(pane_id, PaneStackSelection::Base);
                cx.notify();
            }
            this.activate_current_project(window, cx);
            this.clear_active_tab_attention_if_focused(window, cx);
        })
        .detach();
        this.tabs
            .iter()
            .position(|tab| tab.id == tab_id)
            .is_some_and(|index| {
                index == this.active_tab
                    && this.tabs[index].active_pane == pane_id
                    && this.tabs[index]
                        .pane(pane_id)
                        .is_some_and(|pane| pane.stack.selected_is_base())
            })
    }

    /// Spawns the terminal a [`TerminalSpawnRequest`] describes. The single
    /// spawn path: every interactive pane, split, profile replacement, pane
    /// template and restored session reaches the PTY through here.
    pub(crate) fn spawn_terminal(
        &mut self,
        request: TerminalSpawnRequest,
        settings: &TerminalSpawnSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let TerminalSpawnRequest {
            tab_id,
            pane_id,
            profile,
            shell,
            working_directory,
            wsl_directory,
            wsl_cwd_file,
            terminal_theme,
            path_hyperlink_regexes,
            environment: environment_overrides,
            tracked_multi_command_launch,
            restore: restore_options,
        } = request;
        let Some(shell) = self.resolve_spawn_shell(
            &profile,
            shell,
            tab_id,
            pane_id,
            wsl_directory.as_deref(),
            wsl_cwd_file.as_deref(),
            cx,
        ) else {
            return;
        };
        let mut combined_environment = self.project_environment_for_tab(tab_id);
        combined_environment.extend(environment_overrides);
        let is_wsl = is_wsl_shell(&profile.command);
        let Some((attention_id, pane_routing_id)) = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| {
                tab.pane(pane_id)
                    .map(|pane| (tab.attention_id, pane.routing_id))
            })
        else {
            self.report_pane_spawn_error(
                tab_id,
                pane_id,
                "Could not identify the terminal's Zetta tab".to_owned(),
                cx,
            );
            return;
        };
        let effective_theme = terminal_theme.clone().unwrap_or_else(|| cx.theme().clone());
        // The `mut` is for the zsh history step below, which is Unix-only.
        #[cfg_attr(windows, allow(unused_mut))]
        let mut environment: HashMap<String, String> = match (TerminalEnvironment {
            profile: &profile.command,
            overrides: &combined_environment,
            attention_id,
            tracking_id: pane_id,
            routing_id: pane_routing_id,
            wsl_cwd_file: wsl_cwd_file.as_deref(),
            theme_name: &effective_theme.name,
            no_mux: self.no_mux,
        })
        .build()
        {
            Ok(environment) => environment,
            Err(error) => {
                self.report_pane_spawn_error(tab_id, pane_id, format!("{error:#}"), cx);
                return;
            }
        };
        #[cfg(not(windows))]
        if let Err(error) = configure_zsh_history_environment(&shell, &mut environment, pane_id) {
            log::warn!("could not configure early zsh history filtering: {error:#}");
        }
        let shell_integration_startup_command = (!is_wsl)
            .then(|| shell_integration_startup_command(&shell))
            .flatten();
        let restore_replay = restore_options
            .as_ref()
            .and_then(|options| options.replay.clone());
        let mux_provider =
            match self.mux_provider_for_tab_with_restore_replay(tab_id, restore_replay, cx) {
                Ok(provider) => provider,
                Err(error) => {
                    self.report_pane_spawn_error(
                    tab_id,
                    pane_id,
                    format!(
                        "Could not start the terminal through the session multiplexer: {error:#}"
                    ),
                    cx,
                );
                    return;
                }
            };
        let initial_console_palette =
            (!is_wsl).then(|| terminal::console_palette_for_theme(effective_theme.as_ref()));
        #[cfg(feature = "zmux")]
        if let Some(provider) = mux_provider.as_ref().filter(|provider| {
            provider
                .session_id()
                .is_some_and(|session_id| self.shared_collaboration.is_bound(session_id))
        }) {
            let Some(session_id) = provider.session_id() else {
                self.report_pane_spawn_error(
                    tab_id,
                    pane_id,
                    "Could not identify the shared session".to_owned(),
                    cx,
                );
                return;
            };
            let base_revision = self
                .shared_collaboration
                .state(session_id)
                .map_or(zmux::messages::SessionRevision::INITIAL, |state| {
                    state.revision
                });
            let title = self
                .tabs
                .iter()
                .find(|tab| tab.id == tab_id)
                .and_then(|tab| tab.process_title.clone())
                .unwrap_or_else(|| profile.name.clone());
            self.spawn_shared_terminal(
                SharedTerminalLaunch {
                    tab_id,
                    pane_id,
                    attention_id,
                    pane_routing_id,
                    terminal_theme,
                    provider: provider.clone(),
                    shell,
                    environment,
                    working_directory,
                    title,
                    profile_name: profile.name.clone(),
                    cursor_shape: settings.cursor_shape,
                    alternate_scroll: settings.alternate_scroll,
                    max_scroll_history_lines: settings.max_scroll_history_lines,
                    // The shell integration loads through the `zetta` on the
                    // pane's own `PATH` and reports to the routing ids in its
                    // environment. For a pane running on another machine both
                    // name the wrong side, so it runs without the integration
                    // rather than with one that answers nobody.
                    shell_integration_startup_command: shell_integration_startup_command
                        .filter(|_| !provider.runtime().is_remote()),
                    tracked_multi_command_launch,
                    base_revision,
                    console_palette: initial_console_palette.unwrap_or_default(),
                    resynchronized: false,
                },
                window,
                cx,
            );
            return;
        }
        self.spawn_prepared_local_terminal(
            LocalTerminalLaunch {
                tab_id,
                pane_id,
                attention_id,
                pane_routing_id,
                terminal_theme,
                shell,
                environment,
                working_directory,
                path_hyperlink_regexes,
                restore_options,
                mux_provider,
                initial_console_palette,
                shell_integration_startup_command,
                tracked_multi_command_launch,
            },
            settings,
            window,
            cx,
        );
    }

    fn spawn_prepared_local_terminal(
        &mut self,
        launch: LocalTerminalLaunch,
        settings: &TerminalSpawnSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let LocalTerminalLaunch {
            tab_id,
            pane_id,
            attention_id,
            pane_routing_id,
            terminal_theme,
            shell,
            environment,
            working_directory,
            path_hyperlink_regexes,
            restore_options,
            mux_provider,
            initial_console_palette,
            shell_integration_startup_command,
            tracked_multi_command_launch,
        } = launch;
        let image_paste_handler = Arc::new(crate::ssh_image_paste::SshImagePasteHandler::new(
            shell.clone(),
            environment.clone(),
            working_directory.clone(),
        ));
        let restored_working_directory = restore_options
            .is_some()
            .then(|| working_directory.clone())
            .flatten();
        // The restoring and ordinary constructors take the same arguments and
        // differ only in whether the shell starts fresh, so the choice is a
        // function pointer rather than two spellings of the same 20 arguments.
        // The environment's hasher is `collections::FxBuildHasher`, which this
        // crate cannot name, so this stays inline where it is inferred.
        let construct = if restore_options.is_some() {
            TerminalBuilder::new_with_console_palette_for_restore
        } else {
            TerminalBuilder::new_with_console_palette
        };
        let builder = construct(
            working_directory,
            None,
            shell,
            environment.into_iter().collect(),
            settings.cursor_shape,
            settings.alternate_scroll,
            settings.max_scroll_history_lines,
            path_hyperlink_regexes,
            settings.path_hyperlink_timeout_ms,
            false,
            cx.entity_id().as_u64(),
            None,
            cx,
            Vec::new(),
            PathStyle::local(),
            mux_provider
                .clone()
                .map(|provider| provider as Arc<dyn terminal::PtyProvider>),
            initial_console_palette,
        );
        let this = cx.entity().downgrade();
        let spawned = SpawnedTerminal {
            tab_id,
            pane_id,
            attention_id,
            pane_routing_id,
            terminal_theme,
            restore_options,
            restored_working_directory,
            mux_provider,
            #[cfg(feature = "zmux")]
            shared_pane: None,
            #[cfg(feature = "zmux")]
            shared_runtime: None,
            #[cfg(feature = "zmux")]
            shared_state: None,
            shell_integration_startup_command,
            tracked_multi_command_launch,
            image_paste_handler: Some(image_paste_handler),
        };
        window
            .spawn(cx, async move |cx| match builder.await {
                Ok(mut builder) => {
                    if let Some(options) = spawned.restore_options.as_ref() {
                        builder = builder
                            .with_fresh_shell_restore()
                            .with_restore_prefill(options.prefill.clone())
                            .with_working_directory(spawned.restored_working_directory.clone());
                    }
                    this.update_in(cx, |this, window, cx| {
                        this.finish_terminal_spawn(builder, spawned, window, cx);
                    })
                    .ok();
                }
                Err(error) => {
                    this.update_in(cx, |this, window, cx| {
                        this.report_terminal_spawn_failure(&spawned, &error, window, cx);
                    })
                    .ok();
                }
            })
            .detach();
    }

    #[cfg(feature = "zmux")]
    fn spawn_shared_terminal(
        &mut self,
        launch: SharedTerminalLaunch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab_id = launch.tab_id;
        self.pending_shared_terminal_launches
            .entry(tab_id)
            .or_default()
            .push(launch);
        if !self.shared_spawn_batch_scheduled.insert(tab_id) {
            return;
        }
        let executor = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            executor.timer(Duration::ZERO).await;
            this.update_in(cx, |this, window, cx| {
                this.shared_spawn_batch_scheduled.remove(&tab_id);
                let launches = this
                    .pending_shared_terminal_launches
                    .remove(&tab_id)
                    .unwrap_or_default();
                if launches.len() == 1 {
                    this.spawn_shared_terminal_now(
                        launches.into_iter().next().expect("one launch was queued"),
                        window,
                        cx,
                    );
                } else if !launches.is_empty() {
                    this.spawn_shared_terminal_batch(launches, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    #[cfg(feature = "zmux")]
    fn spawn_shared_terminal_now(
        &mut self,
        launch: SharedTerminalLaunch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Kept so a refusal that turns out to be this window being behind can
        // be answered by making the same launch again, once, against the
        // session as it actually is.
        let retry = (!launch.resynchronized).then(|| SharedTerminalLaunch {
            resynchronized: true,
            ..launch.clone()
        });
        let SharedTerminalLaunch {
            tab_id,
            pane_id,
            attention_id,
            pane_routing_id,
            terminal_theme,
            provider,
            shell,
            environment,
            working_directory,
            title,
            profile_name,
            cursor_shape,
            alternate_scroll,
            max_scroll_history_lines,
            shell_integration_startup_command,
            tracked_multi_command_launch,
            base_revision,
            console_palette,
            resynchronized: _,
        } = launch;
        // Before the geometry is read out of the tab, not after the session has
        // refused it: the proposal is built from this window's translation of
        // its panes, so a translation the session has outlived has to go first.
        self.reconcile_shared_tab(tab_id, cx);
        let spawn_geometry = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .with_context(|| format!("shared tab {tab_id} disappeared"))
            .and_then(|tab| {
                shared_spawn_replacement(&tab.layout, pane_id, self.mux_panes.ids()).map(
                    |(target_pane_id, replacement)| {
                        let active_pane = if tab.active_pane == pane_id {
                            Some(zmux::messages::SharedPaneRef::Draft { draft_id: pane_id })
                        } else {
                            self.mux_panes
                                .mux_pane_id(tab.active_pane)
                                .map(|pane_id| zmux::messages::SharedPaneRef::Existing { pane_id })
                        };
                        (target_pane_id, replacement, active_pane)
                    },
                )
            });
        let Ok((target_pane_id, replacement, active_pane)) = spawn_geometry else {
            self.report_pane_spawn_error(
                tab_id,
                pane_id,
                format!(
                    "Could not prepare the shared pane layout: {:#}",
                    spawn_geometry.unwrap_err()
                ),
                cx,
            );
            return;
        };
        // The proposal is checked against what this window believes the session
        // holds before it is sent, and the window is reconciled with that same
        // belief on the way. A stale translation would otherwise be refused by
        // the daemon for the whole request, and refused again for every split
        // after it, because nothing here would have learned anything.
        let mut named = target_pane_id.into_iter().collect::<Vec<_>>();
        shared_draft_layout_existing_ids(&replacement, &mut named);
        if let Some(zmux::messages::SharedPaneRef::Existing { pane_id }) = active_pane {
            named.push(pane_id);
        }
        let session_id = provider.session_id();
        if !session_id.is_some_and(|session_id| self.shared_geometry_is_current(session_id, &named))
        {
            self.discard_uncommitted_shared_pane(
                tab_id,
                pane_id,
                "The shared session changed while this pane was being created. Try again."
                    .to_owned(),
                cx,
            );
            return;
        }
        let remote = provider.runtime().is_remote();
        let (draft_command, draft_environment) = shared_draft_process(remote, &shell, &environment);
        // Loaded by the daemon, before the pane has an attachment or anything
        // retained. The wrapper that loads it is delivered as if typed, so the
        // terminal echoes it to every viewer of a shared pane — and a viewer
        // that did not write it has no way to know what it is looking at.
        let daemon_loads_shell_integration = shell_integration_startup_command.is_some();
        let shell_integration_startup_command = None;
        let stand_in_size = shared_spawn_stand_in_size(
            self.tabs.iter().find(|tab| tab.id == tab_id),
            target_pane_id
                .and_then(|mux_pane_id| self.mux_panes.local_pane_id(mux_pane_id))
                .unwrap_or(pane_id),
            cx,
        );
        let executor = cx.background_executor().clone();
        let terminal_executor = executor.clone();
        let build = executor.spawn(async move {
            let runtime = provider.runtime().clone();
            let startup_shell = shell.clone();
            let (program, _args) = shell.program_and_args();
            let client = runtime.client().clone();
            let operation_id = client.next_shared_operation_id();
            let session_id = provider
                .session_id()
                .context("shared pane has no multiplexer session")?;
            let request = zmux::messages::SharedSpawnBatchRequest {
                session_id,
                base_revision,
                operation_id,
                target_pane_id,
                replacement,
                panes: vec![zmux::messages::SharedPaneDraft {
                    draft_id: pane_id,
                    profile: profile_name.clone(),
                    command: draft_command.clone(),
                    env: draft_environment.clone(),
                    working_directory: working_directory.clone(),
                    // The pane being split. Only used when this window had no
                    // directory of its own to send, which is the case for a
                    // pane on another machine: it reports its directory to the
                    // window that runs it, not to this one.
                    inherit_working_directory_from: target_pane_id,
                    load_shell_integration: daemon_loads_shell_integration,
                    size: stand_in_size,
                    console_palette,
                    metadata: BackgroundPaneSummary {
                        id: 0,
                        label: title.clone(),
                        profile: profile_name,
                        configured_command: String::new(),
                        application: program,
                        foreground_command: None,
                        terminal_title: None,
                        working_directory: working_directory.clone(),
                        state: BackgroundPaneState::Running,
                        exit: None,
                    },
                }],
                active_pane,
            };
            let spawned = match client.spawn_shared_batch(request) {
                Ok(spawned) => spawned,
                Err(error) => {
                    // The session would not take the geometry. Whatever this
                    // window believed about the session was wrong, so bring
                    // back what is actually there rather than reporting and
                    // leaving the same belief in place to fail again.
                    let Ok(state) = client.shared_snapshot(session_id) else {
                        return Err(error);
                    };
                    return Ok(SharedTerminalBuild::Desynchronized {
                        tab_id,
                        pane_id,
                        state: Box::new(state),
                        reason: format!("{error:#}"),
                    });
                }
            };
            let spawned = match spawned {
                zmux::client::SharedBatchResult::Applied(spawned) => spawned,
                zmux::client::SharedBatchResult::Conflict(state) => {
                    return Ok::<_, anyhow::Error>(SharedTerminalBuild::Conflict {
                        tab_id,
                        pane_id,
                        state: Box::new(state),
                    });
                }
            };
            let mapping = spawned
                .mappings
                .iter()
                .find(|mapping| mapping.draft_id == pane_id)
                .context("shared batch did not commit its requested pane")?;
            provider.record_shared_opened(session_id, mapping.pane_id);
            let mut committed_state = spawned.state;
            // Past this point the pane exists for every viewer, so a failure
            // here is this window's alone. Handing back the committed state
            // rather than an error is what lets the pane arrive the ordinary
            // way — as one the session holds and this window has yet to
            // attach — instead of being dropped locally and left running.
            let pane = match attach_committed_shared_pane(
                &client,
                &runtime,
                session_id,
                mapping.pane_id,
                &mut committed_state,
            ) {
                Ok(pane) => pane,
                Err(error) => {
                    return Ok(SharedTerminalBuild::Desynchronized {
                        tab_id,
                        pane_id,
                        state: Box::new(committed_state),
                        reason: format!("{error:#}"),
                    });
                }
            };
            let pane = Arc::new(pane);
            let image_paste_handler: Arc<dyn terminal::ImagePasteHandler> = if runtime.is_remote() {
                Arc::new(
                    crate::background_session_ui::image_paste::RemoteImagePasteHandler::new(
                        &runtime,
                        pane.session_id(),
                        pane.pane_id(),
                    ),
                )
            } else {
                Arc::new(crate::ssh_image_paste::SshImagePasteHandler::new(
                    shell,
                    environment,
                    working_directory.clone(),
                ))
            };
            let builder = TerminalBuilder::new_byte_stream(
                Box::new(pane.reader()),
                Box::new(
                    crate::background_session_ui::shared_panes::SharedPaneWriter {
                        pane: pane.clone(),
                    },
                ),
                title,
                cursor_shape,
                alternate_scroll,
                max_scroll_history_lines,
                0,
                &terminal_executor,
                PathStyle::local(),
            )
            .with_working_directory(working_directory)
            .with_replay(pane.replay.clone())
            .with_pty_control(crate::mux::mux_pty_control_with_secret(
                runtime.client().clone(),
                pane.session_id(),
                pane.pane_id(),
                runtime.session_secret(),
            ))
            .with_image_paste_handler(image_paste_handler)
            .with_init_command_startup_shell(startup_shell);
            Ok::<_, anyhow::Error>(SharedTerminalBuild::Ready(
                Box::new(builder),
                Box::new(SpawnedTerminal {
                    tab_id,
                    pane_id,
                    attention_id,
                    pane_routing_id,
                    terminal_theme,
                    restore_options: None,
                    restored_working_directory: None,
                    mux_provider: None,
                    shared_pane: Some(pane),
                    shared_runtime: Some(runtime),
                    shared_state: Some(committed_state),
                    shell_integration_startup_command,
                    tracked_multi_command_launch,
                    image_paste_handler: None,
                }),
            ))
        });
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| match build.await {
                Ok(SharedTerminalBuild::Ready(builder, spawned)) => {
                    this.update_in(cx, |this, window, cx| {
                        this.finish_terminal_spawn(*builder, *spawned, window, cx);
                    })
                    .ok();
                }
                Ok(SharedTerminalBuild::Desynchronized {
                    tab_id,
                    pane_id,
                    state,
                    reason,
                }) => {
                    this.update_in(cx, |this, window, cx| {
                        log::warn!(
                            "shared session {} refused this window's pane layout: {reason}",
                            state.session_id
                        );
                        // The snapshot first: it is the session as it is, and
                        // applying it is what makes the retry below propose
                        // something the session can actually accept.
                        this.apply_shared_snapshot(state.session_id, *state, window, cx);
                        let Some(mut retry) = retry else {
                            this.discard_uncommitted_shared_pane(
                                tab_id,
                                pane_id,
                                format!("Could not create the shared pane: {reason}"),
                                cx,
                            );
                            return;
                        };
                        // Rebased on what the session is at now. The revision
                        // the launch was built with belongs to the picture that
                        // has just been replaced.
                        if let Some(revision) = retry
                            .provider
                            .session_id()
                            .and_then(|session_id| this.shared_collaboration.state(session_id))
                            .map(|state| state.revision)
                        {
                            retry.base_revision = revision;
                        }
                        this.spawn_shared_terminal_now(retry, window, cx);
                    })
                    .ok();
                }
                Ok(SharedTerminalBuild::Conflict {
                    tab_id,
                    pane_id,
                    state,
                }) => {
                    this.update_in(cx, |this, window, cx| {
                        this.apply_shared_snapshot(state.session_id, *state, window, cx);
                        this.discard_uncommitted_shared_pane(
                            tab_id,
                            pane_id,
                            "The shared layout changed before this pane could be created."
                                .to_owned(),
                            cx,
                        );
                    })
                    .ok();
                }
                Err(error) => {
                    this.update_in(cx, |this, window, cx| {
                        this.discard_uncommitted_shared_pane(
                            tab_id,
                            pane_id,
                            format!("Could not create the shared pane: {error:#}"),
                            cx,
                        );
                        if tracked_multi_command_launch {
                            this.finish_multi_command_launch(window, cx);
                        }
                    })
                    .ok();
                }
            })
            .detach();
    }

    #[cfg(feature = "zmux")]
    fn spawn_shared_terminal_batch(
        &mut self,
        launches: Vec<SharedTerminalLaunch>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab_id = launches[0].tab_id;
        self.reconcile_shared_tab(tab_id, cx);
        let draft_ids = launches
            .iter()
            .map(|launch| launch.pane_id)
            .collect::<HashSet<_>>();
        let layout = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .with_context(|| format!("shared tab {tab_id} disappeared"))
            .and_then(|tab| {
                shared_complete_draft_layout(&tab.layout, &draft_ids, self.mux_panes.ids()).map(
                    |layout| {
                        let active = if draft_ids.contains(&tab.active_pane) {
                            zmux::messages::SharedPaneRef::Draft {
                                draft_id: tab.active_pane,
                            }
                        } else {
                            zmux::messages::SharedPaneRef::Existing {
                                pane_id: self
                                    .mux_panes
                                    .mux_pane_id(tab.active_pane)
                                    .expect("non-draft shared pane has a mux id"),
                            }
                        };
                        (layout, active)
                    },
                )
            });
        let Ok((replacement, active_pane)) = layout else {
            let error = layout.unwrap_err();
            for launch in launches {
                self.report_pane_spawn_error(
                    tab_id,
                    launch.pane_id,
                    format!("Could not prepare the shared pane batch: {error:#}"),
                    cx,
                );
            }
            return;
        };
        // A batch replaces the session's whole layout rather than one node of
        // it, so every pane the tab holds is named. That makes it the request
        // most exposed to a translation this window kept for a pane the session
        // dropped — one such id and the session refuses all of it.
        let mut named = Vec::new();
        shared_draft_layout_existing_ids(&replacement, &mut named);
        if let zmux::messages::SharedPaneRef::Existing { pane_id } = active_pane {
            named.push(pane_id);
        }
        let current = launches[0]
            .provider
            .session_id()
            .is_some_and(|session_id| self.shared_geometry_is_current(session_id, &named));
        if !current {
            for launch in &launches {
                self.discard_uncommitted_shared_pane(
                    tab_id,
                    launch.pane_id,
                    "The shared session changed while these panes were being created. Try again."
                        .to_owned(),
                    cx,
                );
            }
            return;
        }
        let runtime = launches[0].provider.runtime().clone();
        let session_id = launches[0]
            .provider
            .session_id()
            .expect("a shared launch has a session id");
        let base_revision = launches[0].base_revision;
        let remote = runtime.is_remote();
        let client = runtime.client().clone();
        let operation_id = client.next_shared_operation_id();
        let stand_in_size = shared_spawn_stand_in_size(
            self.tabs.iter().find(|tab| tab.id == tab_id),
            launches[0].pane_id,
            cx,
        );
        // A batch replaces the whole layout rather than splitting one pane, so
        // the directory to fall back to is the tab's active pane's.
        let inherit_directory_from = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| self.mux_panes.mux_pane_id(tab.active_pane));
        let panes = launches
            .iter()
            .map(|launch| {
                let (program, _args) = launch.shell.program_and_args();
                let (command, env) =
                    shared_draft_process(remote, &launch.shell, &launch.environment);
                zmux::messages::SharedPaneDraft {
                    draft_id: launch.pane_id,
                    profile: launch.profile_name.clone(),
                    command,
                    env,
                    working_directory: launch.working_directory.clone(),
                    inherit_working_directory_from: inherit_directory_from,
                    load_shell_integration: launch.shell_integration_startup_command.is_some(),
                    size: stand_in_size,
                    console_palette: launch.console_palette,
                    metadata: BackgroundPaneSummary {
                        id: 0,
                        label: launch.title.clone(),
                        profile: launch.profile_name.clone(),
                        configured_command: String::new(),
                        application: program,
                        foreground_command: None,
                        terminal_title: None,
                        working_directory: launch.working_directory.clone(),
                        state: BackgroundPaneState::Running,
                        exit: None,
                    },
                }
            })
            .collect();
        let executor = cx.background_executor().clone();
        let terminal_executor = executor.clone();
        let request = zmux::messages::SharedSpawnBatchRequest {
            session_id,
            base_revision,
            operation_id,
            target_pane_id: None,
            replacement,
            panes,
            active_pane: Some(active_pane),
        };
        let build = executor.spawn(async move {
            build_shared_terminal_batch(launches, runtime, request, &terminal_executor)
        });
        cx.spawn_in(window, async move |this, cx| match build.await {
            Ok(SharedTerminalBatchBuild::Ready {
                session_id,
                state,
                mut terminals,
            }) => {
                this.update_in(cx, |this, window, cx| {
                    for (_, spawned) in &terminals {
                        let pane = spawned.shared_pane.as_ref().expect("batch pane is shared");
                        this.mux_panes.record(spawned.pane_id, pane.pane_id());
                        this.shared_collaboration.record_pane(
                            session_id,
                            pane.pane_id(),
                            spawned.pane_id,
                        );
                    }
                    this.apply_shared_snapshot(session_id, state, window, cx);
                    for (builder, spawned) in terminals.drain(..) {
                        this.finish_terminal_spawn(builder, spawned, window, cx);
                    }
                })
                .ok();
            }
            Ok(SharedTerminalBatchBuild::Desynchronized {
                state,
                panes,
                reason,
            }) => {
                this.update_in(cx, |this, window, cx| {
                    log::warn!(
                        "shared session {} refused this window's pane batch: {reason}",
                        state.session_id
                    );
                    this.apply_shared_snapshot(state.session_id, state, window, cx);
                    for (tab_id, pane_id) in panes {
                        this.discard_uncommitted_shared_pane(
                            tab_id,
                            pane_id,
                            "This window's view of the shared session was out of date. It has \
                             been brought up to date — try again."
                                .to_owned(),
                            cx,
                        );
                    }
                })
                .ok();
            }
            Ok(SharedTerminalBatchBuild::Conflict { state, panes }) => {
                this.update_in(cx, |this, window, cx| {
                    this.apply_shared_snapshot(state.session_id, state, window, cx);
                    for (tab_id, pane_id) in panes {
                        this.discard_uncommitted_shared_pane(
                            tab_id,
                            pane_id,
                            "The shared layout changed before this pane batch committed."
                                .to_owned(),
                            cx,
                        );
                    }
                })
                .ok();
            }
            Err(error) => {
                this.update_in(cx, |this, _window, cx| {
                    for pane_id in draft_ids {
                        this.discard_uncommitted_shared_pane(
                            tab_id,
                            pane_id,
                            format!("Could not create the shared panes: {error:#}"),
                            cx,
                        );
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    /// The shell a spawn runs, deriving it from the profile when the request did
    /// not name one.
    ///
    /// Returns `None` once it has already reported the failure on the pane —
    /// only the Cygwin CWD-tracking path can fail, and it does so with a message
    /// naming what could not be configured.
    #[expect(
        clippy::too_many_arguments,
        reason = "six values the spawn is resolved from, plus the GPUI context"
    )]
    fn resolve_spawn_shell(
        &mut self,
        profile: &Profile,
        shell: Option<Shell>,
        tab_id: u64,
        pane_id: u64,
        wsl_directory: Option<&str>,
        wsl_cwd_file: Option<&Path>,
        cx: &mut Context<Self>,
    ) -> Option<Shell> {
        // Only the Cygwin arm below reads these, and it is Windows-only.
        #[cfg(not(windows))]
        let (_, _, _) = (tab_id, pane_id, &cx);
        Some(match shell {
            Some(shell) => shell,
            None if is_wsl_shell(&profile.command) => {
                wsl_shell_with_tracking(profile.command.clone(), wsl_directory, wsl_cwd_file)
            }
            None if cfg!(windows) && cygwin_profile(&profile.command).is_some() => {
                #[cfg(windows)]
                {
                    match cygwin_shell_with_tracking(
                        profile.command.clone(),
                        pane_id,
                        &env::temp_dir(),
                    ) {
                        Ok(shell) => shell,
                        Err(error) => {
                            self.report_pane_spawn_error(
                                tab_id,
                                pane_id,
                                format!("Could not configure Cygwin CWD tracking: {error:#}"),
                                cx,
                            );
                            return None;
                        }
                    }
                }
                #[cfg(not(windows))]
                unreachable!()
            }
            None => profile.command.clone(),
        })
    }

    /// Wires a terminal whose builder has resolved into the pane that asked for
    /// it, and runs whatever has to happen once it exists.
    ///
    /// The pane may have been closed or detached while the builder was in
    /// flight, which is why the install below falls back to the background
    /// session that now owns it.
    fn finish_terminal_spawn(
        &mut self,
        mut builder: TerminalBuilder,
        spawned: SpawnedTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SpawnedTerminal {
            tab_id,
            pane_id,
            attention_id,
            pane_routing_id,
            terminal_theme,
            restore_options,
            shell_integration_startup_command,
            tracked_multi_command_launch,
            mux_provider,
            #[cfg(feature = "zmux")]
            shared_pane,
            #[cfg(feature = "zmux")]
            shared_runtime,
            #[cfg(feature = "zmux")]
            shared_state,
            image_paste_handler,
            ..
        } = spawned;
        if let Some(image_paste_handler) = image_paste_handler {
            builder = builder.with_image_paste_handler(image_paste_handler);
        }
        #[cfg(feature = "zmux")]
        if let (Some(shared_pane), Some(runtime)) = (&shared_pane, &shared_runtime) {
            let mux_pane_id = shared_pane.pane_id();
            self.mux_panes.adopt_session_with_runtime(
                tab_id,
                shared_pane.session_id(),
                runtime.clone(),
            );
            self.mux_panes.record(pane_id, mux_pane_id);
            self.shared_collaboration
                .record_pane(shared_pane.session_id(), mux_pane_id, pane_id);
            if let Some(state) = shared_state {
                let _ = self.shared_collaboration.accept_pane_added(
                    shared_pane.session_id(),
                    state.clone(),
                    mux_pane_id,
                    pane_id,
                );
                self.apply_shared_snapshot(shared_pane.session_id(), state, window, cx);
            }
        }
        let this = self;
        this.adopt_mux_pane(
            tab_id,
            pane_id,
            mux_provider.as_deref(),
            &mut builder,
            window,
            cx,
        );
        let terminal = cx.new(|cx| builder.subscribe(cx));
        let view =
            cx.new(|cx| TerminalView::new_with_theme(terminal.clone(), terminal_theme, window, cx));
        this.configure_terminal_view_silent_mode(tab_id, &view, cx);
        this.subscribe_spawned_terminal(
            tab_id,
            pane_id,
            Some(crate::run_command::RunPaneIdentity::new(
                attention_id,
                pane_routing_id,
            )),
            &terminal,
            window,
            cx,
        );
        this.subscribe_spawned_terminal_view(tab_id, pane_id, &view, window, cx);
        let should_focus =
            this.configure_spawned_terminal_focus(tab_id, pane_id, &view, window, cx);
        let tab_index = this.tabs.iter().position(|tab| tab.id == tab_id);
        if let Some(pane) = tab_index
            .and_then(|index| this.tabs.get_mut(index))
            .and_then(|tab| tab.pane_mut(pane_id))
        {
            pane.terminal = Some(terminal.clone());
            pane.view = Some(view.clone());
            pane.base_exited = false;
            pane.error = None;
            pane.exit = None;
            if restore_options.is_none()
                && let Some(command) = pane.pending_command.take()
            {
                view.update(cx, |view, cx| {
                    view.apply_input(&TerminalInput::Text(format!("{command}\r")), cx);
                });
            }
        } else {
            let stored_in_background = {
                let pane = this
                    .background_sessions
                    .iter_mut()
                    .find(|tab| tab.id == tab_id)
                    .and_then(|tab| tab.pane_mut(pane_id));
                if let Some(pane) = pane {
                    pane.terminal = Some(terminal.clone());
                    true
                } else {
                    false
                }
            };
            if stored_in_background {
                this.observe_background_terminal(tab_id, pane_id, terminal.clone(), cx);
                this.publish_background_session_catalog(cx);
            } else {
                // Either this pane, or its whole tab, closed while the spawn
                // above was still in flight. `adopt_mux_pane` just registered
                // it as held by this process, and nothing else will ever ask
                // the multiplexer to let it go — dropping `terminal` below
                // only closes this process's copy of the descriptor, not the
                // daemon's, so without this the pane stays wedged as ours
                // until the window exits. See `Zetta::release_mux_pane`.
                this.release_mux_pane(tab_id, pane_id, cx);
                // Only when the whole tab is gone: a pane closed on its own
                // still leaves the tab's other panes sharing this session, so
                // their mapping to it must not be erased here.
                if tab_index.is_none() {
                    this.mux_panes.forget_tab(tab_id);
                }
            }
        }
        #[cfg(feature = "zmux")]
        if let (Some(shared_pane), Some(runtime)) = (&shared_pane, &shared_runtime) {
            this.register_shared_pane(
                MuxPaneIds {
                    tab_id,
                    pane_id,
                    session_id: shared_pane.session_id(),
                    mux_pane_id: shared_pane.pane_id(),
                },
                shared_pane,
                runtime,
                window,
                cx,
            );
            // Geometry and pane metadata were committed in the batch. Once the
            // terminal exists, publish the remaining opaque PaneState that needs
            // its stable local-to-mux mapping.
            this.sync_shared_tab_state(tab_id, cx);
        }
        this.schedule_worktree_detection_for_pane(tab_id, pane_id, cx);
        this.schedule_project_detection_for_pane(tab_id, pane_id, window, cx);
        if should_focus {
            let focus_handle = view.focus_handle(cx);
            this.focus_terminal_if_allowed(&focus_handle, window, cx);
        }
        this.sync_visible_terminals(cx);
        this.schedule_terminal_spawn_notify(cx);
        if tracked_multi_command_launch {
            this.finish_multi_command_launch(window, cx);
        }
        if restore_options.is_some() {
            let terminal_for_restore = terminal.clone();
            if let Some(command) = shell_integration_startup_command.as_ref() {
                let startup_handshake = terminal.update(cx, |terminal, _| {
                    terminal.start_init_command_startup_handshake()
                });
                let command = command.clone();
                cx.spawn(async move |_this, cx| {
                    startup_handshake.await;
                    terminal_for_restore.update(cx, |terminal, cx| {
                        terminal.write_init_command_after_startup(command, cx);
                        terminal.finish_fresh_shell_restore(cx);
                    });
                })
                .detach();
            } else {
                terminal_for_restore.update(cx, |terminal, cx| {
                    terminal.finish_fresh_shell_restore(cx);
                });
            }
        } else if let Some(command) = shell_integration_startup_command.as_ref() {
            let startup_handshake = terminal.update(cx, |terminal, _| {
                terminal.start_init_command_startup_handshake()
            });
            let command = command.clone();
            let terminal_for_startup = terminal.clone();
            cx.spawn(async move |_this, cx| {
                startup_handshake.await;
                terminal_for_startup.update(cx, |terminal, cx| {
                    terminal.write_init_command_after_startup(command, cx);
                });
            })
            .detach();
        }
    }

    /// A spawn that never produced a terminal: the pane shows the error, and a
    /// multi-command batch waiting on this launch is released so the rest of it
    /// is not left pending.
    fn report_terminal_spawn_failure(
        &mut self,
        spawned: &SpawnedTerminal,
        error: &anyhow::Error,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SpawnedTerminal {
            tab_id,
            pane_id,
            tracked_multi_command_launch,
            ..
        } = *spawned;
        let this = self;
        if let Some(pane) = this
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane_mut(pane_id))
        {
            pane.error = Some(format!("{error:#}"));
        }
        this.schedule_terminal_spawn_notify(cx);
        if tracked_multi_command_launch {
            this.finish_multi_command_launch(window, cx);
        }
    }

    pub(crate) fn schedule_terminal_spawn_notify(&mut self, cx: &mut Context<Self>) {
        if !begin_coalesced_notification(&mut self.terminal_spawn_notify_pending) {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(TERMINAL_SPAWN_NOTIFY_INTERVAL)
                .await;
            this.update(cx, |this, cx| {
                this.terminal_spawn_notify_pending = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

pub(crate) fn apply_terminal_environment_overrides<S>(
    environment: &mut HashMap<String, String, S>,
    overrides: &HashMap<String, String>,
    process_id: u32,
    attention_id: u64,
    pane_id: u64,
    pane_routing_id: u64,
    no_mux: bool,
) where
    S: std::hash::BuildHasher,
{
    for (name, value) in overrides {
        if !name
            .get(..6)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("ZETTA_"))
        {
            environment.insert(name.clone(), value.clone());
        }
    }
    environment.insert("ZETTA_PROCESS_ID".to_owned(), process_id.to_string());
    environment.insert("ZETTA_ATTENTION_ID".to_owned(), attention_id.to_string());
    environment.insert("ZETTA_PANE_ID".to_owned(), pane_id.to_string());
    environment.insert(
        "ZETTA_PANE_ROUTING_ID".to_owned(),
        pane_routing_id.to_string(),
    );
    environment.insert(
        "ZETTA_NO_MUX".to_owned(),
        if no_mux { "1" } else { "0" }.to_owned(),
    );
}

/// Builds the shell invocation used by a stacked command. Native profiles go
/// through the same shell-aware builder as Zed tasks. WSL, MSYS2, and Cygwin
/// profiles preserve their launcher or executable so the command runs inside
/// the configured POSIX environment rather than in the Windows command shell.
pub(crate) fn stacked_task_shell(
    profile: &Shell,
    command: &str,
    wsl_directory: Option<&str>,
) -> Shell {
    if is_wsl_shell(profile) {
        return match wsl_shell_with_tracking(profile.clone(), wsl_directory, None) {
            Shell::WithArguments {
                program,
                mut args,
                title_override,
            } => {
                args.extend([
                    "--exec".to_owned(),
                    "/bin/sh".to_owned(),
                    "-i".to_owned(),
                    "-c".to_owned(),
                    command.to_owned(),
                ]);
                Shell::WithArguments {
                    program,
                    args,
                    title_override,
                }
            }
            shell => shell,
        };
    }

    #[cfg(windows)]
    if let Some((root, shell)) = msys2_profile(profile) {
        let shell_name = match shell {
            Msys2Shell::Bash => "bash.exe",
            Msys2Shell::Zsh => "zsh.exe",
        };
        let shell = Shell::Program(
            root.join("usr")
                .join("bin")
                .join(shell_name)
                .display()
                .to_string(),
        );
        let (program, args) =
            ShellBuilder::new(&shell, true).build_no_quote(Some(command.to_owned()), &[]);
        return Shell::WithArguments {
            program,
            args,
            title_override: None,
        };
    }

    #[cfg(windows)]
    if cygwin_profile(profile).is_some() {
        let (program, args) =
            ShellBuilder::new(profile, true).build_no_quote(Some(command.to_owned()), &[]);
        return Shell::WithArguments {
            program,
            args,
            title_override: None,
        };
    }

    let (program, args) =
        ShellBuilder::new(profile, cfg!(windows)).build_no_quote(Some(command.to_owned()), &[]);
    Shell::WithArguments {
        program,
        args,
        title_override: None,
    }
}

#[cfg(test)]
#[path = "tests/terminal_spawn.rs"]
mod tests;

/// The inputs one stacked command terminal needs. Its `entry_id` addresses
/// the [`PaneStack`] entry that shares `pane_id`'s layout region.
pub(crate) struct StackedTerminalSpawnRequest {
    pub(crate) tab_id: u64,
    pub(crate) pane_id: u64,
    pub(crate) entry_id: u64,
    pub(crate) command: String,
    pub(crate) profile: Profile,
    pub(crate) working_directory: Option<PathBuf>,
    pub(crate) wsl_directory: Option<String>,
    pub(crate) terminal_theme: Option<Arc<Theme>>,
}

impl Zetta {
    pub(crate) fn spawn_stacked_terminal(
        &mut self,
        request: StackedTerminalSpawnRequest,
        settings: &mut TerminalSpawnSettings,
        // `final_spawn` lets the last terminal of a batch move the shared
        // hyperlink regexes instead of cloning them.
        final_spawn: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let StackedTerminalSpawnRequest {
            tab_id,
            pane_id,
            entry_id,
            command,
            profile,
            working_directory,
            wsl_directory,
            terminal_theme,
        } = request;
        let is_wsl = is_wsl_shell(&profile.command);
        let Some(attention_id) = self.attention_id_for_tab(tab_id) else {
            self.stacked_terminal_failed(
                tab_id,
                pane_id,
                entry_id,
                "Could not identify the terminal's Zetta tab".to_owned(),
                cx,
            );
            return;
        };
        let pane_routing_id = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| {
                tab.pane(pane_id).and_then(|pane| {
                    pane.stack
                        .entries
                        .iter()
                        .find(|entry| entry.id == entry_id)
                        .map(|entry| entry.routing_id)
                })
            })
            .unwrap_or(entry_id);
        let shell = stacked_task_shell(&profile.command, &command, wsl_directory.as_deref());
        let project_environment = self.project_environment_for_tab(tab_id);
        let effective_theme = terminal_theme.clone().unwrap_or_else(|| cx.theme().clone());
        let environment = match (TerminalEnvironment {
            profile: &profile.command,
            overrides: &project_environment,
            attention_id,
            tracking_id: entry_id,
            routing_id: pane_routing_id,
            wsl_cwd_file: None,
            theme_name: &effective_theme.name,
            no_mux: self.no_mux,
        })
        .build()
        {
            Ok(environment) => environment,
            Err(error) => {
                self.stacked_terminal_failed(tab_id, pane_id, entry_id, format!("{error:#}"), cx);
                return;
            }
        };

        let (completion_tx, completion_rx) = async_channel::unbounded();
        let task = SpawnInTerminal {
            id: TaskId(format!("zetta-stacked-{tab_id}-{entry_id}")),
            full_label: command.clone(),
            label: command.clone(),
            command: Some(command.clone()),
            args: Vec::new(),
            command_label: command.clone(),
            cwd: working_directory.clone(),
            env: SpawnInTerminal::default().env,
            use_new_terminal: false,
            allow_concurrent_runs: true,
            reveal: task::RevealStrategy::Never,
            reveal_target: task::RevealTarget::Dock,
            hide: task::HideStrategy::Never,
            shell: shell.clone(),
            show_summary: false,
            show_command: false,
            show_rerun: false,
            save: task::SaveStrategy::None,
        };
        let task_state = TaskState {
            status: TaskStatus::Running,
            completion_rx,
            spawned_task: task,
        };
        let mux_provider = match self.mux_provider_for_tab(tab_id, cx) {
            Ok(provider) => provider,
            Err(error) => {
                self.report_pane_spawn_error(
                    tab_id,
                    pane_id,
                    format!(
                        "Could not start the stacked terminal through the session multiplexer: {error:#}"
                    ),
                    cx,
                );
                return;
            }
        };
        let initial_console_palette =
            (!is_wsl).then(|| terminal::console_palette_for_theme(effective_theme.as_ref()));
        let image_paste_handler = Arc::new(crate::ssh_image_paste::SshImagePasteHandler::new(
            shell.clone(),
            environment.clone(),
            working_directory.clone(),
        ));
        let builder = TerminalBuilder::new_with_console_palette(
            working_directory,
            Some(task_state),
            shell,
            environment,
            settings.cursor_shape,
            settings.alternate_scroll,
            settings.max_scroll_history_lines,
            settings.path_hyperlink_regexes(final_spawn),
            settings.path_hyperlink_timeout_ms,
            false,
            cx.entity_id().as_u64(),
            Some(completion_tx),
            cx,
            Vec::new(),
            PathStyle::local(),
            mux_provider
                .clone()
                .map(|provider| provider as Arc<dyn terminal::PtyProvider>),
            initial_console_palette,
        );

        let this = cx.entity().downgrade();
        let spawned = SpawnedStackedTerminal {
            tab_id,
            pane_id,
            entry_id,
            attention_id,
            pane_routing_id,
            terminal_theme,
            mux_provider,
            image_paste_handler,
        };
        window
            .spawn(cx, async move |cx| match builder.await {
                Ok(builder) => {
                    this.update_in(cx, |this, window, cx| {
                        this.finish_stacked_terminal_spawn(builder, spawned, window, cx);
                    })
                    .ok();
                }
                Err(error) => {
                    this.update_in(cx, |this, _window, cx| {
                        this.stacked_terminal_failed(
                            spawned.tab_id,
                            spawned.pane_id,
                            spawned.entry_id,
                            format!("{error:#}"),
                            cx,
                        );
                        this.schedule_terminal_spawn_notify(cx);
                    })
                    .ok();
                }
            })
            .detach();
    }

    /// Wires a stacked command's terminal into the pane stack that asked for it.
    ///
    /// Unlike an interactive pane, a stacked entry routes its exit and its focus
    /// through the stack's selection, which is why this does not reuse
    /// `finish_terminal_spawn`.
    fn finish_stacked_terminal_spawn(
        &mut self,
        mut builder: TerminalBuilder,
        spawned: SpawnedStackedTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SpawnedStackedTerminal {
            tab_id,
            pane_id,
            entry_id,
            attention_id,
            pane_routing_id,
            terminal_theme,
            mux_provider,
            image_paste_handler,
        } = spawned;
        builder = builder.with_image_paste_handler(image_paste_handler);
        let this = self;
        this.adopt_mux_pane(
            tab_id,
            entry_id,
            mux_provider.as_deref(),
            &mut builder,
            window,
            cx,
        );
        let terminal = cx.new(|cx| builder.subscribe(cx));
        let view =
            cx.new(|cx| TerminalView::new_with_theme(terminal.clone(), terminal_theme, window, cx));
        this.configure_terminal_view_silent_mode(tab_id, &view, cx);
        let run_registry = crate::run_command::process_run_registry();
        let run_identity = crate::run_command::RunPaneIdentity::new(attention_id, pane_routing_id);
        run_registry.pane_reopened(run_identity);
        cx.subscribe_in(
            &terminal,
            window,
            move |this, terminal, event: &TerminalEvent, _window, cx| match event {
                TerminalEvent::TrackingReady => {
                    run_registry.tracking_ready(run_identity);
                }
                TerminalEvent::CommandStarted { command } => {
                    run_registry.command_started(run_identity, command.clone());
                }
                TerminalEvent::CommandFinished { exit_code } => {
                    run_registry.command_finished(run_identity, *exit_code);
                }
                TerminalEvent::TerminalExited(_) => {
                    run_registry.terminal_lost(run_identity);
                }
                TerminalEvent::TaskFinished { exit_code } => {
                    this.stacked_task_finished(tab_id, pane_id, entry_id, *exit_code, cx);
                }
                TerminalEvent::ResizeRequested { .. } => {
                    terminal.update(cx, |terminal, _| {
                        terminal.truncate_on_next_resize();
                    });
                }
                _ => {}
            },
        )
        .detach();
        cx.subscribe_in(
            &view,
            window,
            move |this, _, event, window, cx| match event {
                TerminalViewEvent::Close => {
                    this.stacked_terminal_closed(tab_id, pane_id, entry_id, window, cx);
                }
                TerminalViewEvent::TitleChanged => cx.notify(),
                TerminalViewEvent::PasteError(error) => this.show_notice(error.clone(), cx),
                TerminalViewEvent::Input(_) => {}
                TerminalViewEvent::OpenEditor(request) => {
                    this.open_editor_in_new_pane(tab_id, pane_id, request.clone(), window, cx);
                }
            },
        )
        .detach();
        let input_enabled = this.terminal_input_enabled();
        view.update(cx, |view, cx| {
            view.set_emit_input_events(false);
            view.set_input_enabled(input_enabled, cx);
        });
        let focus_handle = view.focus_handle(cx);
        cx.on_focus_in(&focus_handle, window, move |this, window, cx| {
            if let Some(tab) = this.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                tab.activate_stack_entry(pane_id, PaneStackSelection::Stacked(entry_id));
                cx.notify();
            }
            this.activate_current_project(window, cx);
            this.clear_active_tab_attention_if_focused(window, cx);
        })
        .detach();
        let tab_index = this.tabs.iter().position(|tab| tab.id == tab_id);
        let should_focus = tab_index.is_some_and(|index| {
            index == this.active_tab
                && this.tabs[index].active_pane == pane_id
                && this.tabs[index].pane(pane_id).is_some_and(|pane| {
                    pane.stack.selected == PaneStackSelection::Stacked(entry_id)
                })
        });
        let inserted = tab_index
            .and_then(|index| this.tabs.get_mut(index))
            .and_then(|tab| tab.pane_mut(pane_id))
            .and_then(|pane| {
                let entry = pane
                    .stack
                    .entries
                    .iter_mut()
                    .find(|entry| entry.id == entry_id)?;
                entry.terminal = Some(terminal.clone());
                entry.view = Some(view.clone());
                entry.state = StackedPaneState::Running;
                Some(())
            })
            .is_some();
        if !inserted {
            let stored_in_background = this
                .background_sessions
                .iter_mut()
                .find(|tab| tab.id == tab_id)
                .and_then(|tab| tab.pane_mut(pane_id))
                .and_then(|pane| {
                    let entry = pane
                        .stack
                        .entries
                        .iter_mut()
                        .find(|entry| entry.id == entry_id)?;
                    entry.terminal = Some(terminal.clone());
                    entry.state = StackedPaneState::Running;
                    Some(())
                })
                .is_some();
            if stored_in_background {
                terminal.update(cx, |terminal, cx| {
                    terminal.set_ui_visible(false, cx);
                });
                this.observe_background_stacked_terminal(pane_id, entry_id, terminal.clone(), cx);
                this.publish_background_session_catalog(cx);
            }
        }
        if should_focus {
            let focus_handle = view.focus_handle(cx);
            this.focus_terminal_if_allowed(&focus_handle, window, cx);
        }
        this.sync_visible_terminals(cx);
        this.schedule_terminal_spawn_notify(cx);
    }

    pub(crate) fn stacked_terminal_failed(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        entry_id: u64,
        error: String,
        cx: &mut Context<Self>,
    ) {
        let entry = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane_mut(pane_id))
            .and_then(|pane| {
                pane.stack
                    .entries
                    .iter_mut()
                    .find(|entry| entry.id == entry_id)
            });
        if let Some(entry) = entry {
            entry.state = StackedPaneState::Failed;
            entry.error = Some(error);
            cx.notify();
            return;
        }
        let updated_background = self
            .background_sessions
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane_mut(pane_id))
            .and_then(|pane| {
                pane.stack
                    .entries
                    .iter_mut()
                    .find(|entry| entry.id == entry_id)
            })
            .map(|entry| {
                entry.state = StackedPaneState::Failed;
                entry.error = Some(error);
            })
            .is_some();
        if updated_background {
            self.publish_background_session_catalog(cx);
        }
    }
}

#[cfg(not(windows))]
fn configure_zsh_history_environment<S>(
    shell: &Shell,
    environment: &mut HashMap<String, String, S>,
    pane_id: u64,
) -> Result<()>
where
    S: std::hash::BuildHasher,
{
    let program = shell.program();
    let is_zsh = Path::new(&program)
        .file_stem()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("zsh"));
    if !is_zsh || shell_integration_startup_command(shell).is_none() {
        return Ok(());
    }

    let directory = tempfile::Builder::new()
        .prefix(&format!("zetta-zsh-history-{pane_id}-"))
        .tempdir()
        .context("creating temporary zsh history directory")?;
    let zshenv = directory.path().join(".zshenv");
    fs::write(&zshenv, ZSH_EARLY_HISTORY_INTEGRATION.as_bytes())
        .with_context(|| format!("writing {}", zshenv.display()))?;
    let directory = directory.keep();
    let directory = directory
        .to_str()
        .context("temporary zsh history directory is not valid UTF-8")?
        .to_owned();

    let original_zdotdir = environment
        .get("ZDOTDIR")
        .cloned()
        .filter(|value| !value.is_empty());
    environment.insert("ZETTA_ZSH_HISTORY_ZDOTDIR".to_owned(), directory.clone());
    environment.insert(
        "ZETTA_ZSH_ORIGINAL_ZDOTDIR_SET".to_owned(),
        u8::from(original_zdotdir.is_some()).to_string(),
    );
    if let Some(original_zdotdir) = original_zdotdir {
        environment.insert("ZETTA_ZSH_ORIGINAL_ZDOTDIR".to_owned(), original_zdotdir);
    } else {
        environment.remove("ZETTA_ZSH_ORIGINAL_ZDOTDIR");
    }
    environment.insert("ZDOTDIR".to_owned(), directory);
    Ok(())
}
