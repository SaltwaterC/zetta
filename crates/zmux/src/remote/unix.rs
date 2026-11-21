//! The Unix client's transport: a private OpenSSH `ControlMaster`, and the
//! stream-local forward added to it that every connection goes through.

use super::*;

/// The one SSH login a Unix transport makes, shared by everything else.
///
/// The child and its private directory are owned by this value: the control
/// socket and every forward socket live in that directory, so dropping it
/// ends the login and removes every local path it exposed.
pub(super) struct Master {
    pub(super) child: Child,
    pub(super) directory: tempfile::TempDir,
    pub(super) control_path: PathBuf,
    pub(super) stderr: CapturedOutput,
}

impl Master {
    pub(super) fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for Master {
    fn drop(&mut self) {
        terminate_child(&mut self.child);
        // Keep the field explicit: TempDir removes the sockets and directory
        // after the child is gone, so no stale endpoint survives a failed SSH
        // startup or a dropped tab.
        let _ = &self.directory;
    }
}

/// One stream-local forward on the master, to the daemon it last reported.
pub(super) struct ForwardState {
    pub(super) local_socket: PathBuf,
    /// The `-L` specification, kept so a refresh can cancel it.
    pub(super) forwarding: String,
    pub(super) endpoint: Endpoint,
    pub(super) agent_holder: Option<Child>,
    pub(super) verified_at: Option<Instant>,
}

impl Drop for ForwardState {
    fn drop(&mut self) {
        if let Some(mut child) = self.agent_holder.take() {
            terminate_child(&mut child);
        }
    }
}

pub(super) fn restrict_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("restricting SSH control directory {}", path.display()))
}

impl RemoteTransport {
    /// Runs `forward` unless the host is known to need the bridge, and runs
    /// `link` instead when `forward` is what found that out.
    pub(super) fn forward_or_link<T>(
        &self,
        state: &mut RemoteState,
        forward: impl FnOnce(&mut RemoteState) -> Result<T>,
        link: impl FnOnce(&mut RemoteState) -> Result<T>,
    ) -> Result<T> {
        if !self.uses_link() {
            match forward(state) {
                Err(error) if error.is::<WindowsHost>() => {}
                result => return result,
            }
        }
        link(state)
    }

    pub(super) fn connect_forward(&self, state: &mut RemoteState) -> Result<(Endpoint, Stream)> {
        for attempt in 0..2 {
            self.install_forward(state)?;
            let forward = state
                .forward
                .as_ref()
                .expect("the remote forward was just installed");
            let local_socket = forward.local_socket.clone();
            let endpoint = forward.endpoint.clone();
            let trusted = is_trusted(forward.verified_at);
            let opened = if trusted {
                Stream::connect(&local_socket)
                    .context("connecting to the SSH forward")
                    .map_err(|error| (error, "connecting to the SSH forward"))
            } else {
                self.probed_connection(&endpoint, || {
                    Stream::connect(&local_socket).context("connecting to the SSH forward")
                })
            };
            match opened {
                Ok(stream) => {
                    if !trusted && let Some(forward) = state.forward.as_mut() {
                        forward.verified_at = Some(Instant::now());
                    }
                    return Ok((endpoint, stream));
                }
                Err((error, _)) if attempt == 0 => {
                    let detail = state
                        .master
                        .as_ref()
                        .map(|master| captured_text(&master.stderr))
                        .unwrap_or_default();
                    log::debug!(
                        "remote SSH connection for {} is unusable: {error:#}; {detail}",
                        self.target.destination()
                    );
                    // A master whose TCP connection went quiet still accepts
                    // locally, so a failure here does not prove only the
                    // forward is stale. Start the login over, as a fresh SSH
                    // process always used to.
                    state.forward = None;
                    state.master = None;
                }
                Err((error, activity)) => {
                    return Err(error)
                        .with_context(|| format!("{activity} for {}", self.target.destination()));
                }
            }
        }
        anyhow::bail!(
            "could not connect to the SSH forward for {}",
            self.target.destination()
        )
    }

    pub(super) fn refresh_forward(&self, state: &mut RemoteState) -> Result<Endpoint> {
        if let Some(forward) = state.forward.take()
            && let Some(master) = state.master.as_mut()
            && master.is_alive()
        {
            let arguments = forward_request_arguments(
                &self.target,
                &master.control_path,
                "cancel",
                &forward.forwarding,
            );
            if let Err(error) = run_capture(&self.ssh_program, &arguments, FORWARD_TIMEOUT) {
                log::debug!(
                    "could not cancel the previous forward for {}: {error:#}",
                    self.target.destination()
                );
            }
        }
        self.ensure_endpoint_locked(state)
    }

    pub(super) fn ensure_endpoint_locked(&self, state: &mut RemoteState) -> Result<Endpoint> {
        self.install_forward(state)?;
        Ok(state
            .forward
            .as_ref()
            .expect("the forward was just installed")
            .endpoint
            .clone())
    }

    /// Makes sure the SSH login is up, returning its control socket.
    pub(super) fn ensure_master(&self, state: &mut RemoteState) -> Result<PathBuf> {
        if let Some(master) = state.master.as_mut() {
            if master.is_alive() {
                return Ok(master.control_path.clone());
            }
            log::debug!(
                "the SSH login for {} ended: {}",
                self.target.destination(),
                captured_text(&master.stderr)
            );
            state.forward = None;
            state.master = None;
        }
        state.login_connection = None;
        let master = self.start_master()?;
        let control_path = master.control_path.clone();
        state.master = Some(master);
        Ok(control_path)
    }

    pub(super) fn start_master(&self) -> Result<Master> {
        let directory = tempfile::Builder::new()
            .prefix("zetta-zmux-")
            .tempdir()
            .context("creating the private SSH control directory")?;
        restrict_directory(directory.path())?;
        let control_path = directory.path().join("ctl");
        let mut child = Command::new(&self.ssh_program)
            .args(master_arguments(&self.target, &control_path))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("starting SSH for {}", self.target.destination))?;
        let stderr = capture_output(child.stderr.take().context("SSH stderr was not captured")?);

        // OpenSSH listens on a temporary name and links it into place, so the
        // control path existing means the login is done and the socket is
        // accepting.
        let deadline = Instant::now() + ENDPOINT_TIMEOUT;
        loop {
            if let Some(status) = child.try_wait().context("checking SSH status")? {
                // Give the reader a moment to collect what SSH said last.
                thread::sleep(POLL_INTERVAL);
                anyhow::bail!(
                    "SSH exited with {status} while logging in to {}{}",
                    self.target.destination,
                    ssh_detail(&captured_text(&stderr))
                );
            }
            if control_path.exists() {
                return Ok(Master {
                    child,
                    directory,
                    control_path,
                    stderr,
                });
            }
            if Instant::now() >= deadline {
                terminate_child(&mut child);
                anyhow::bail!(
                    "SSH did not finish logging in to {} within {ENDPOINT_TIMEOUT:?}{}",
                    self.target.destination,
                    ssh_detail(&captured_text(&stderr))
                );
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    /// Makes sure a forward to the daemon exists on a live login.
    pub(super) fn install_forward(&self, state: &mut RemoteState) -> Result<()> {
        if state
            .master
            .as_mut()
            .is_none_or(|master| !master.is_alive())
        {
            state.forward = None;
        }
        if state.forward.is_none() {
            let forward = self.start_forward(state)?;
            state.forward = Some(forward);
        }
        Ok(())
    }

    pub(super) fn start_forward(&self, state: &mut RemoteState) -> Result<ForwardState> {
        static FORWARDS: AtomicU64 = AtomicU64::new(0);

        let control_path = self.ensure_master(state)?;
        let directory = state
            .master
            .as_ref()
            .expect("the login was just established")
            .directory
            .path()
            .to_owned();
        let (program, remote_endpoint) = self.query_endpoint(Some(&control_path))?;
        state.program = Some(program);
        let local_socket = directory.join(format!(
            "mux-{}.sock",
            FORWARDS.fetch_add(1, Ordering::Relaxed)
        ));
        let forwarding = format!(
            "{}:{}",
            local_socket.display(),
            remote_endpoint.socket_path.display()
        );
        run_capture(
            &self.ssh_program,
            &forward_request_arguments(&self.target, &control_path, "forward", &forwarding),
            FORWARD_TIMEOUT,
        )
        .with_context(|| {
            format!(
                "adding the stream-local forward for {}",
                self.target.destination
            )
        })?;
        anyhow::ensure!(
            local_socket.exists(),
            "SSH accepted the stream-local forward for {} but did not create it",
            self.target.destination
        );
        let agent_holder = match (
            self.target.forward_agent,
            remote_endpoint.socket_path.parent(),
        ) {
            (Some(true), Some(remote_directory)) => {
                let socket = remote_directory.join("forwarded-agent.sock");
                let child = Command::new(&self.ssh_program)
                    .args(agent_holder_arguments(&self.target, &control_path, &socket))
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .context("starting SSH agent forwarding")?;
                Some(child)
            }
            _ => None,
        };
        Ok(ForwardState {
            endpoint: Endpoint {
                version: remote_endpoint.version,
                protocol_version: remote_endpoint.protocol_version,
                process_id: remote_endpoint.process_id,
                socket_path: local_socket.clone(),
                token: remote_endpoint.token,
            },
            local_socket,
            forwarding,
            agent_holder,
            verified_at: None,
        })
    }

    /// Asks the remote host where `zmux` is and which daemon it is running.
    /// Fails with [`WindowsHost`] when the host turns out to need the bridge.
    pub(super) fn query_endpoint(&self, control: Option<&Path>) -> Result<(PathBuf, Endpoint)> {
        let arguments = endpoint_arguments(&self.target, control);
        match run_capture(&self.ssh_program, &arguments, ENDPOINT_TIMEOUT) {
            Ok(output) => {
                remote_host::learn(&self.target, HostPlatform::Posix);
                parse_endpoint_output(&output.stdout)
            }
            Err(error) => Err(self.explain_failure(control, error)),
        }
    }
}
