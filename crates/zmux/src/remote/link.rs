//! The bridge transport: one `ssh HOST zmux proxy-mux` carrying every
//! connection over its stdio, for Windows clients and for Windows hosts.

use super::*;

/// `zmux proxy-mux` on the far side: a Windows client's one SSH login, or a
/// session on a Unix client's master when the host is Windows.
pub(super) struct BridgeLink {
    pub(super) bridge: crate::mux_bridge::MuxBridge,
    pub(super) child: Child,
    pub(super) stderr: CapturedOutput,
}

impl BridgeLink {
    pub(super) fn is_alive(&mut self) -> bool {
        self.bridge.is_alive() && matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for BridgeLink {
    fn drop(&mut self) {
        terminate_child(&mut self.child);
    }
}

impl RemoteTransport {
    pub(super) fn connect_link(&self, state: &mut RemoteState) -> Result<(Endpoint, Stream)> {
        for attempt in 0..2 {
            let endpoint = self.install_link(state)?;
            let trusted = is_trusted(state.link_verified_at);
            let (opened, stderr) = {
                let link = state.link.as_ref().expect("the bridge was just installed");
                let opened = if trusted {
                    link.bridge
                        .open()
                        .map_err(|error| (error, "opening a stream over the zmux bridge"))
                } else {
                    self.probed_connection(&endpoint, || link.bridge.open())
                };
                (opened, link.stderr.clone())
            };
            match opened {
                Ok(stream) => {
                    if !trusted {
                        state.link_verified_at = Some(Instant::now());
                    }
                    return Ok((endpoint, stream));
                }
                Err((error, _)) if attempt == 0 => {
                    log::debug!(
                        "remote zmux bridge for {} is unusable: {error:#}; {}",
                        self.target.destination(),
                        captured_text(&stderr)
                    );
                    state.link = None;
                    state.link_endpoint = None;
                    state.link_verified_at = None;
                }
                Err((error, activity)) => {
                    let detail = ssh_detail(&captured_text(&stderr));
                    return Err(error).with_context(|| {
                        format!("{activity} for {}{detail}", self.target.destination())
                    });
                }
            }
        }
        anyhow::bail!(
            "could not connect to the zmux bridge for {}",
            self.target.destination()
        )
    }

    pub(super) fn refresh_link(&self, state: &mut RemoteState) -> Result<Endpoint> {
        state.link_endpoint = None;
        state.link_verified_at = None;
        self.install_link(state)
    }

    /// Makes sure the bridge is up and knows the daemon's endpoint.
    ///
    /// A bridge started while no daemon was running reports no endpoint; it
    /// is asked again over the same link rather than replaced, so waiting
    /// for a daemon to start costs a round trip per poll, not a login.
    pub(super) fn install_link(&self, state: &mut RemoteState) -> Result<Endpoint> {
        if state.link.as_mut().is_some_and(|link| !link.is_alive()) {
            if let Some(link) = state.link.as_ref() {
                log::debug!(
                    "the zmux bridge for {} ended: {}",
                    self.target.destination(),
                    captured_text(&link.stderr)
                );
            }
            state.link = None;
            state.link_endpoint = None;
            state.link_verified_at = None;
        }
        if state.link.is_none() {
            state.login_connection = None;
            let link = self.start_link(state)?;
            let info = link.bridge.initial_info().clone();
            state.link = Some(link);
            self.apply_bridge_info(state, info)?;
        }
        if let Some(endpoint) = state.link_endpoint.clone() {
            return Ok(endpoint);
        }
        let info = state
            .link
            .as_ref()
            .expect("the bridge was just installed")
            .bridge
            .query_info(ENDPOINT_TIMEOUT)?;
        self.apply_bridge_info(state, info)
    }

    pub(super) fn apply_bridge_info(
        &self,
        state: &mut RemoteState,
        info: crate::mux_bridge::BridgeInfo,
    ) -> Result<Endpoint> {
        if let Some(program) = info.program {
            state.program = Some(program);
        }
        let endpoint = info.endpoint.with_context(|| {
            info.error
                .unwrap_or_else(|| "the remote host reported no multiplexer".to_owned())
        })?;
        validate_endpoint(&endpoint)?;
        state.link_endpoint = Some(endpoint.clone());
        state.link_verified_at = None;
        Ok(endpoint)
    }

    /// Starts the bridge in the host's dialect. A Windows client may not know
    /// it yet: the POSIX bridge failing is what finds out.
    #[cfg_attr(
        windows,
        expect(
            unused_variables,
            reason = "Windows has no SSH control master to start"
        )
    )]
    pub(super) fn start_link(&self, state: &mut RemoteState) -> Result<BridgeLink> {
        #[cfg(unix)]
        let control = Some(self.ensure_master(state)?);
        #[cfg(windows)]
        let control: Option<PathBuf> = { None };
        let known = remote_host::learned(&self.target);
        let platform = known.unwrap_or(HostPlatform::Posix);
        match self.spawn_link(control.as_deref(), platform) {
            Ok(link) => {
                remote_host::learn(&self.target, platform);
                Ok(link)
            }
            Err(error) if known.is_none() => {
                let error = self.explain_failure(control.as_deref(), error);
                if !error.is::<WindowsHost>() {
                    return Err(error);
                }
                self.spawn_link(control.as_deref(), HostPlatform::Windows)
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn spawn_link(
        &self,
        control: Option<&Path>,
        platform: HostPlatform,
    ) -> Result<BridgeLink> {
        let mut child = Command::new(&self.ssh_program)
            .args(bridge_arguments(&self.target, control, platform))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("starting SSH for {}", self.target.destination))?;
        let (Some(input), Some(output), Some(errors)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            terminate_child(&mut child);
            anyhow::bail!("SSH did not expose its standard streams");
        };
        let stderr = capture_output(errors);
        match crate::mux_bridge::MuxBridge::connect(output, input, ENDPOINT_TIMEOUT) {
            Ok(bridge) => Ok(BridgeLink {
                bridge,
                child,
                stderr,
            }),
            Err(error) => {
                terminate_child(&mut child);
                Err(error).with_context(|| {
                    format!(
                        "starting the zmux bridge on {}{}",
                        self.target.destination,
                        ssh_detail(&captured_text(&stderr))
                    )
                })
            }
        }
    }
}
