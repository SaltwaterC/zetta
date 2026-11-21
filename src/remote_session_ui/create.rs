//! Creating a session on the remote host from the picker: the pane layout
//! the chosen template describes, and the request that starts it there.

use super::*;

pub(super) fn build_remote_create_spec(
    config: &Config,
    template_option: &RemoteSessionTemplate,
    default_profile: &str,
    remote_profiles: &[String],
) -> anyhow::Result<zmux::headless::CreateSpec> {
    let empty_environment = HashMap::new();
    let single_pane = PaneSplitTemplate::Pane(Box::default());
    let (layout_template, template_environment) = match template_option {
        RemoteSessionTemplate::SinglePane => (&single_pane, &empty_environment),
        RemoteSessionTemplate::Configured(template_name) => {
            let template = config
                .pane_split_templates
                .get(template_name)
                .with_context(|| {
                    format!("remote template {template_name:?} is no longer available")
                })?;
            (&template.layout, &template.env)
        }
    };
    let mut panes = Vec::with_capacity(layout_template.pane_count());
    let mut next_draft_id = 1;
    let layout = build_remote_create_layout(
        layout_template,
        template_environment,
        default_profile,
        remote_profiles,
        &mut panes,
        &mut next_draft_id,
    )?;
    let active_pane = panes
        .first()
        .map(|pane| zmux::messages::SharedPaneRef::Draft {
            draft_id: pane.draft_id,
        })
        .context("remote session template has no panes")?;
    Ok(zmux::headless::CreateSpec {
        title: String::new(),
        layout,
        active_pane,
        panes,
    })
}

pub(super) fn build_remote_create_layout(
    node: &PaneSplitTemplate,
    inherited_env: &HashMap<String, String>,
    default_profile: &str,
    remote_profiles: &[String],
    panes: &mut Vec<zmux::messages::SharedPaneDraft>,
    next_draft_id: &mut u64,
) -> anyhow::Result<zmux::messages::SharedDraftLayout> {
    match node {
        PaneSplitTemplate::Split {
            axis,
            first,
            second,
        } => {
            let first = build_remote_create_layout(
                first,
                inherited_env,
                default_profile,
                remote_profiles,
                panes,
                next_draft_id,
            )?;
            let second = build_remote_create_layout(
                second,
                inherited_env,
                default_profile,
                remote_profiles,
                panes,
                next_draft_id,
            )?;
            Ok(zmux::messages::SharedDraftLayout::Split {
                axis: axis.as_str().to_owned(),
                first_ratio: zmux::protocol::DEFAULT_BACKGROUND_PANE_SPLIT_RATIO,
                first: Box::new(first),
                second: Box::new(second),
            })
        }
        PaneSplitTemplate::Pane(pane) => {
            anyhow::ensure!(
                pane.theme.is_none()
                    && pane.dark_theme.is_none()
                    && pane.overlay.is_none()
                    && pane.stack.is_empty(),
                "remote headless creation does not support pane themes, overlays, or stacked commands"
            );
            let draft_id = *next_draft_id;
            *next_draft_id = next_draft_id.saturating_add(1);
            let profile = pane.profile.as_ref().map_or_else(
                || default_profile.to_owned(),
                |profile| profile.name.clone(),
            );
            let command = pane.command.as_ref().map(|command| {
                zetta_profiles::ProfileCommand::with_args(
                    command.program.clone(),
                    command.args.clone(),
                )
            });
            if command.is_none() {
                anyhow::ensure!(
                    remote_profiles
                        .iter()
                        .any(|remote| remote.eq_ignore_ascii_case(&profile)),
                    "profile {profile:?} is not available on the remote host"
                );
            }
            let mut env = inherited_env.clone();
            env.extend(pane.env.clone());
            let label = pane
                .label
                .clone()
                .unwrap_or_else(|| format!("pane-{draft_id}"));
            let application = command
                .as_ref()
                .and_then(|command| command.program.clone())
                .unwrap_or_else(|| profile.clone());
            panes.push(zmux::messages::SharedPaneDraft {
                draft_id,
                profile: profile.clone(),
                command,
                env,
                working_directory: None,
                inherit_working_directory_from: None,
                load_shell_integration: true,
                size: zmux::messages::TerminalSize {
                    columns: zmux::headless::DEFAULT_COLUMNS,
                    lines: zmux::headless::DEFAULT_LINES,
                    cell_width: 0,
                    cell_height: 0,
                },
                console_palette: terminal::ConsolePalette::default(),
                metadata: zmux::protocol::BackgroundPaneSummary {
                    id: 0,
                    label,
                    profile,
                    configured_command: String::new(),
                    application,
                    foreground_command: None,
                    terminal_title: None,
                    working_directory: None,
                    state: zmux::protocol::BackgroundPaneState::Starting,
                    exit: None,
                },
            });
            Ok(zmux::messages::SharedDraftLayout::Draft { draft_id })
        }
    }
}

impl Zetta {
    pub(crate) fn create_remote_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (values, can_create) = {
            let Some(picker) = self.remote_session_picker.as_ref() else {
                return;
            };
            let values = Self::remote_target_from_picker(picker).and_then(|target| {
                Ok((
                    target,
                    Self::remote_transport_from_picker(picker)?,
                    picker
                        .templates
                        .get(picker.selected_template)
                        .cloned()
                        .context("select a remote session template")?,
                    picker
                        .profiles
                        .get(picker.selected_profile)
                        .cloned()
                        .context("load remote profiles before creating a session")?,
                    picker.profiles.clone(),
                ))
            });
            (values, picker.can_create())
        };
        let (target, transport, template, profile_name, profiles) = match values {
            Ok(values) if can_create => values,
            Ok(_) => {
                if let Some(picker) = self.remote_session_picker.as_mut() {
                    picker.profile_error =
                        Some("Load the remote profiles before creating a session.".to_owned());
                }
                self.remote_session_focus.focus(window, cx);
                cx.notify();
                return;
            }
            Err(error) => {
                if let Some(picker) = self.remote_session_picker.as_mut() {
                    picker.error = Some(remote_error_message(&error));
                }
                self.remote_session_focus.focus(window, cx);
                cx.notify();
                return;
            }
        };
        let spec = match build_remote_create_spec(
            &self.launch_config,
            &template,
            &profile_name,
            &profiles,
        ) {
            Ok(spec) => spec,
            Err(error) => {
                if let Some(picker) = self.remote_session_picker.as_mut() {
                    picker.error = Some(remote_error_message(&error));
                }
                self.remote_session_focus.focus(window, cx);
                cx.notify();
                return;
            }
        };
        self.remote_session_target = Some(target.clone());
        self.remote_session_transport = transport;
        self.remote_session_create = Some(RemoteSessionCreate {
            target,
            transport,
            spec,
        });
        self.remote_session_picker = None;
        self.open_session_authentication_prompt(
            SessionAuthenticationPromptMode::RemoteCreate,
            window,
            cx,
        );
    }

    pub(crate) fn start_remote_session_create(
        &mut self,
        authentication: Option<SessionAuthentication>,
        secret: Option<SessionSecret>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(create) = self.remote_session_create.take() else {
            self.show_notice(
                "The remote session create request is no longer available.",
                cx,
            );
            self.remote_session_target = None;
            self.focus_active(window, cx);
            return;
        };
        let operation_generation = self.next_remote_session_operation_generation();
        let RemoteSessionCreate {
            target,
            transport,
            spec,
        } = create;
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let client = zmux::client::Client::connect_remote_for_creation(target.clone())
                        .context("connecting to the remote multiplexer")?;
                    let request = spec.request(
                        client.next_shared_operation_id(),
                        authentication.map(|authentication| {
                            authentication.verifier().to_owned()
                        }),
                    );
                    let created = client
                        .create_shared(request)
                        .context("creating the remote headless session")?;
                    let session_id = created.session_id;
                    let attached = load_remote_attach(
                        target,
                        session_id,
                        secret,
                        transport,
                    )
                    .map_err(|error| {
                        anyhow::anyhow!(
                            "created remote session {session_id}, but could not attach it: {error:#}"
                        )
                    })?;
                    Ok::<_, anyhow::Error>((session_id, attached))
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.apply_remote_create_result(
                    operation_generation,
                    result,
                    window,
                    cx,
                );
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn apply_remote_create_result(
        &mut self,
        operation_generation: u64,
        result: anyhow::Result<(u64, RemoteAttachOutcome)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.remote_session_operation_is_current(operation_generation) {
            return;
        }
        match result {
            Ok((session_id, RemoteAttachOutcome::Attached(data))) => {
                match self.finish_remote_multiplexer_session(*data, window, cx) {
                    Ok(crate::background_session_ui::AttachOutcomeSummary::Attached) => {
                        self.remote_session_target = None;
                        self.focus_active(window, cx);
                    }
                    Ok(_) => self.show_error_notice(
                        format!(
                            "Created remote session {session_id}, but it could not be shown here."
                        ),
                        cx,
                    ),
                    Err(error) => self.show_error_notice(
                        format!(
                            "Created remote session {session_id}, but could not show it: {}",
                            remote_error_message(&error)
                        ),
                        cx,
                    ),
                }
            }
            Ok((session_id, RemoteAttachOutcome::AuthenticationRequired))
            | Ok((session_id, RemoteAttachOutcome::AuthenticationFailed)) => {
                self.show_error_notice(
                    format!(
                        "Created remote session {session_id}, but authentication failed while attaching."
                    ),
                    cx,
                );
                self.remote_session_target = None;
            }
            Err(error) => {
                self.show_error_notice(
                    format!(
                        "Could not create a remote session: {}",
                        remote_error_message(&error)
                    ),
                    cx,
                );
                self.remote_session_target = None;
            }
        }
        cx.notify();
    }
}
