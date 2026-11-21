//! Reaching a multiplexer: starting or finding the local daemon, or a remote
//! one over SSH, and the identity a client presents to it.

use super::*;
use zetta_sync::RecoverPoison as _;

impl Client {
    /// Connects to the running multiplexer, starting one if there is none.
    pub fn connect() -> Result<Self> {
        Self::connect_with_retention(Retention::default())
    }

    /// Connects to the running multiplexer, applying `retention` whether the
    /// daemon already exists or has just been started. The application
    /// resolves this before the first pane is spawned, so a constrained build
    /// cannot silently start or reuse a daemon with a different retention
    /// policy.
    pub fn connect_with_retention(retention: Retention) -> Result<Self> {
        #[cfg(feature = "session-persistence")]
        {
            Self::connect_with_retention_and_persistence(retention, PersistenceOptions::default())
        }
        #[cfg(not(feature = "session-persistence"))]
        {
            retention.validate()?;
            Self::connect_at_with_retention(&session_catalog_dir(), retention)
        }
    }

    #[cfg(feature = "session-persistence")]
    pub fn connect_with_retention_and_persistence(
        retention: Retention,
        persistence: PersistenceOptions,
    ) -> Result<Self> {
        retention.validate()?;
        Self::connect_at_with_retention_and_persistence(
            &session_catalog_dir(),
            retention,
            persistence,
        )
    }

    /// Connects with the application's requested retention, temporarily using
    /// `fallback_retention` when a valid GitHub recipient cannot be resolved
    /// because of a network failure.
    ///
    /// This is intentionally separate from [`Self::connect_with_retention_and_persistence`]:
    /// disk resume and CLI commands must keep reporting configuration and
    /// connectivity errors instead of silently changing their durability.
    #[cfg(feature = "session-persistence")]
    pub fn connect_with_retention_and_persistence_resilient(
        retention: Retention,
        persistence: PersistenceOptions,
        fallback_retention: Retention,
    ) -> Result<ConfiguredClient> {
        retention.validate()?;
        fallback_retention.validate()?;
        anyhow::ensure!(
            matches!(fallback_retention, Retention::Memory { .. }),
            "retention fallback must be an in-memory policy"
        );
        Self::connect_at_with_retention_and_persistence_resilient(
            &session_catalog_dir(),
            retention,
            persistence,
            fallback_retention,
        )
    }

    #[cfg(feature = "session-persistence")]
    pub fn connect_with_retention_for_resume(retention: Retention) -> Result<Self> {
        Self::connect_at_with_retention_for_resume(&session_catalog_dir(), retention)
    }

    /// Connects only if a multiplexer is already running.
    pub fn connect_existing() -> Result<Option<Self>> {
        Self::connect_existing_at(&session_catalog_dir())
    }

    /// Connects to a multiplexer whose protocol this build may not speak.
    ///
    /// Only for `--upgrade`, which is the request that exists to cross that
    /// boundary: the daemon accepts it from a client that disagrees about the
    /// version, and refusing to *connect* on those grounds made the one way out
    /// of a mismatch unreachable — the client reported the very mismatch the
    /// command was there to resolve.
    pub fn connect_for_upgrade() -> Result<Option<Self>> {
        Self::connect_for_upgrade_at(&session_catalog_dir())
    }

    /// As [`Self::connect_for_upgrade`], against an explicit session directory.
    pub fn connect_for_upgrade_at(directory: &std::path::Path) -> Result<Option<Self>> {
        Self::connect_endpoint(directory, VersionCheck::Tolerated)
    }

    /// As [`Self::connect`], against an explicit session directory.
    ///
    /// The directory is a parameter rather than read from the environment so
    /// that tests — and, later, more than one multiplexer on a host — do not
    /// have to mutate process-global state to choose one.
    pub fn connect_at(directory: &std::path::Path) -> Result<Self> {
        Self::connect_at_with_retention(directory, Retention::default())
    }

    /// As [`Self::connect_with_retention`], against an explicit session
    /// directory. Tests use this so each daemon has isolated state.
    pub fn connect_at_with_retention(
        directory: &std::path::Path,
        retention: Retention,
    ) -> Result<Self> {
        #[cfg(feature = "session-persistence")]
        return Self::connect_at_with_retention_and_persistence(
            directory,
            retention,
            PersistenceOptions::default(),
        );
        #[cfg(not(feature = "session-persistence"))]
        {
            retention.validate()?;
            if let Some(client) = Self::connect_ready_at(directory)? {
                client.configure(retention, Vec::new())?;
                return Ok(client);
            }
            start_daemon(directory, None)?;
            let deadline = Instant::now() + STARTUP_TIMEOUT;
            loop {
                if let Some(client) = Self::connect_ready_at(directory)? {
                    client.configure(retention, Vec::new())?;
                    return Ok(client);
                }
                anyhow::ensure!(
                    Instant::now() < deadline,
                    "the multiplexer did not start within {STARTUP_TIMEOUT:?}"
                );
                thread::sleep(STARTUP_POLL);
            }
        }
    }

    #[cfg(feature = "session-persistence")]
    pub fn connect_at_with_retention_and_persistence(
        directory: &std::path::Path,
        retention: Retention,
        persistence: PersistenceOptions,
    ) -> Result<Self> {
        retention.validate()?;
        let resolved_recipients = if matches!(retention, Retention::Disk) {
            crate::persistence::resolve_recipient_strings(&persistence.recipients)?
        } else {
            Vec::new()
        };
        Self::connect_at_with_resolved_retention_and_persistence(
            directory,
            retention,
            resolved_recipients,
        )
    }

    #[cfg(feature = "session-persistence")]
    pub fn connect_at_with_retention_and_persistence_resilient(
        directory: &std::path::Path,
        retention: Retention,
        persistence: PersistenceOptions,
        fallback_retention: Retention,
    ) -> Result<ConfiguredClient> {
        retention.validate()?;
        fallback_retention.validate()?;
        anyhow::ensure!(
            matches!(fallback_retention, Retention::Memory { .. }),
            "retention fallback must be an in-memory policy"
        );
        let (effective_retention, degraded_reason, recipients) = resolve_effective_retention(
            retention,
            &persistence.recipients,
            fallback_retention,
            crate::persistence::resolve_recipient_strings_for_startup,
        )?;
        let client = Self::connect_at_with_resolved_retention_and_persistence(
            directory,
            effective_retention,
            recipients,
        )?;
        Ok(ConfiguredClient {
            client,
            requested_retention: retention,
            effective_retention,
            degraded_reason,
        })
    }

    #[cfg(feature = "session-persistence")]
    pub(super) fn connect_at_with_resolved_retention_and_persistence(
        directory: &std::path::Path,
        retention: Retention,
        resolved_recipients: Vec<String>,
    ) -> Result<Self> {
        if let Some(client) = Self::connect_ready_at(directory)? {
            client.configure(retention, resolved_recipients)?;
            return Ok(client);
        }
        start_daemon(directory, None, Some(resolved_recipients.clone()))?;
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            if let Some(client) = Self::connect_ready_at(directory)? {
                client.configure(retention, resolved_recipients.clone())?;
                return Ok(client);
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "the multiplexer did not start within {STARTUP_TIMEOUT:?}"
            );
            thread::sleep(STARTUP_POLL);
        }
    }

    #[cfg(unix)]
    pub(super) fn configure_after_upgrade(
        &self,
        retention: Retention,
        persistence_recipients: Vec<String>,
    ) -> Result<()> {
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        let mut last_error = None;
        loop {
            if let Some(client) = Self::connect_existing_at(&self.directory)? {
                match client.configure_raw(retention, persistence_recipients.clone()) {
                    Ok(()) => return Ok(()),
                    Err(error) if is_closed(&error) || is_unsupported_configure(&error) => {
                        last_error = Some(error);
                    }
                    Err(error) => return Err(error),
                }
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "the multiplexer did not apply the new session retention settings within \
                 {STARTUP_TIMEOUT:?}{}",
                last_error
                    .as_ref()
                    .map(|error| format!(": {error:#}"))
                    .unwrap_or_default()
            );
            thread::sleep(STARTUP_POLL);
        }
    }

    /// Starts a disk daemon in recovery mode, allowing it to reuse the saved
    /// public recipient set for a `resume` command that has no Zetta config in
    /// the client process.
    #[cfg(feature = "session-persistence")]
    pub fn connect_at_with_retention_for_resume(
        directory: &std::path::Path,
        retention: Retention,
    ) -> Result<Self> {
        retention.validate()?;
        if let Some(client) = Self::connect_existing_at(directory)? {
            return Ok(client);
        }
        // Standalone `zmux resume` has no Zetta configuration to apply after
        // startup, so it still uses the explicit disk bootstrap mode. The
        // normal Zetta connection path starts with the daemon's default and
        // configures it from the loaded application settings instead.
        start_daemon(directory, Some(retention), None)?;
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            if let Some(client) = Self::connect_existing_at(directory)? {
                return Ok(client);
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "the multiplexer did not start within {STARTUP_TIMEOUT:?}"
            );
            thread::sleep(STARTUP_POLL);
        }
    }

    pub fn connect_existing_at(directory: &std::path::Path) -> Result<Option<Self>> {
        Self::connect_endpoint(directory, VersionCheck::Required)
    }

    /// Connects to a multiplexer only after it has answered a request.
    ///
    /// A socket can accept a connection before the daemon has finished its
    /// startup work. The ping makes readiness mean that the request loop is
    /// actually serving the current protocol, rather than merely that a
    /// listener has been bound. `connect_for_upgrade_at` deliberately remains
    /// liveness-only so it can reach a daemon across a protocol boundary.
    pub fn connect_ready_at(directory: &std::path::Path) -> Result<Option<Self>> {
        let Some(client) = Self::connect_endpoint(directory, VersionCheck::Required)? else {
            return Ok(None);
        };
        client.ping_until_ready(Instant::now() + STARTUP_TIMEOUT)?;
        Ok(Some(client))
    }

    /// The process ID published by the daemon endpoint. It is part of a
    /// session's stable catalog identifier and lets administrative commands
    /// reject an identifier belonging to a different catalog in the same
    /// session directory.
    pub fn process_id(&self) -> u32 {
        self.endpoint_snapshot().process_id
    }

    pub(super) fn connect_endpoint(
        directory: &std::path::Path,
        version_check: VersionCheck,
    ) -> Result<Option<Self>> {
        let Ok(endpoint) = Endpoint::read(&endpoint_path(directory)) else {
            return Ok(None);
        };
        // Liveness first, and only then the version. An endpoint outlives the
        // daemon that wrote it, so checking the version first would let a dead
        // multiplexer's leftover file refuse every attempt to start a live
        // one — permanently, and with an error about a process that no longer
        // exists.
        if Stream::connect(&endpoint.socket_path).is_err() {
            return Ok(None);
        }
        // A multiplexer left over from an earlier build cannot serve this
        // client. Finding that out here lets the application report the
        // ownership conflict before it creates a terminal outside the daemon
        // contract.
        anyhow::ensure!(
            matches!(version_check, VersionCheck::Tolerated)
                || endpoint.protocol_version == PROTOCOL_VERSION,
            "the multiplexer running as process {} speaks protocol version {}, not \
             {PROTOCOL_VERSION}. Sessions it holds are still listed by `zmux list` but \
             cannot be attached until it is replaced; `zmux --upgrade` replaces it in place, \
             keeping them.",
            endpoint.process_id,
            endpoint.protocol_version
        );
        Ok(Some(Self {
            endpoint: Mutex::new(endpoint),
            directory: directory.to_owned(),
            remote: None,
            client_id: ClientId::random()?,
            stream_only: false,
            session_secret: Arc::new(Mutex::new(None)),
            operation_counter: Arc::new(AtomicU64::new(1)),
        }))
    }

    /// Connects to a remote daemon over SSH, starting the deployed standalone
    /// multiplexer when none is reachable. Never upgrades a running daemon
    /// or asks the remote host for a descriptor.
    ///
    /// The SSH login is the process's shared one for `target`, so a client
    /// made moments after another (the picker's, then the attach's) reuses it.
    pub fn connect_remote(target: RemoteTarget) -> Result<Self> {
        let remote = RemoteTransport::shared(target)?;
        let endpoint = remote.ensure_daemon()?;
        Ok(Self {
            endpoint: Mutex::new(endpoint),
            directory: PathBuf::new(),
            client_id: ClientId::random()?,
            stream_only: true,
            remote: Some(remote),
            session_secret: Arc::new(Mutex::new(None)),
            operation_counter: Arc::new(AtomicU64::new(1)),
        })
    }

    /// Connects to a remote daemon for a headless create operation.
    pub fn connect_remote_for_creation(target: RemoteTarget) -> Result<Self> {
        Self::connect_remote(target)
    }

    /// Testable variant of [`Self::connect_remote`] that uses a supplied SSH
    /// executable.
    #[cfg(any(test, feature = "test-support"))]
    pub fn connect_remote_with_ssh_program(
        target: RemoteTarget,
        ssh_program: impl Into<PathBuf>,
    ) -> Result<Self> {
        let remote = Arc::new(RemoteTransport::for_creation_with_ssh_program(
            target,
            ssh_program,
        )?);
        let endpoint = remote.ensure_daemon()?;
        Ok(Self {
            endpoint: Mutex::new(endpoint),
            directory: PathBuf::new(),
            client_id: ClientId::random()?,
            stream_only: true,
            remote: Some(remote),
            session_secret: Arc::new(Mutex::new(None)),
            operation_counter: Arc::new(AtomicU64::new(1)),
        })
    }

    /// Testable variant of [`Self::connect_remote_for_creation`].
    #[cfg(any(test, feature = "test-support"))]
    pub fn connect_remote_for_creation_with_ssh_program(
        target: RemoteTarget,
        ssh_program: impl Into<PathBuf>,
    ) -> Result<Self> {
        Self::connect_remote_with_ssh_program(target, ssh_program)
    }

    #[cfg(all(test, unix))]
    pub(crate) fn from_remote_transport_for_test(
        remote: Arc<RemoteTransport>,
        endpoint: Endpoint,
    ) -> Self {
        Self {
            endpoint: Mutex::new(endpoint),
            directory: PathBuf::new(),
            client_id: ClientId::new("test-client"),
            stream_only: true,
            remote: Some(remote),
            session_secret: Arc::new(Mutex::new(None)),
            operation_counter: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn is_remote(&self) -> bool {
        self.remote.is_some()
    }

    pub fn stream_only(&self) -> bool {
        self.stream_only
    }

    /// This client's logical identity, which every request it makes carries and
    /// which the daemon matches a pane's viewers against.
    ///
    /// Needed by a caller that has to name this client to something running
    /// elsewhere: a Mosh pane's relay attaches on the daemon's host and has to
    /// say whose view of the pane it is.
    pub fn client_id(&self) -> &ClientId {
        &self.client_id
    }

    pub fn remote_target(&self) -> Option<&RemoteTarget> {
        self.remote.as_ref().map(|remote| remote.target())
    }

    /// The control socket of the SSH login this remote client uses, for a
    /// command that should run on the same host without logging in again.
    /// See [`RemoteTransport::control_path`].
    pub fn remote_control_path(&self) -> Option<PathBuf> {
        self.remote
            .as_ref()
            .and_then(|remote| remote.control_path())
    }

    /// Whether this client's remote host is Windows.
    pub fn remote_host_is_windows(&self) -> bool {
        self.remote
            .as_ref()
            .is_some_and(|remote| remote.is_windows_host())
    }

    /// Where the remote host keeps its own `zmux`.
    ///
    /// Learned with the endpoint, so normally answered without asking the
    /// remote host again. It is what lets a command started outside an SSH command's
    /// environment — inside a Mosh server, for instance — run the same
    /// multiplexer this client is already talking to.
    pub fn resolve_remote_program(&self) -> Result<PathBuf> {
        // Usually learned with the endpoint, and then free.
        self.remote
            .as_ref()
            .context("this client is not connected to a remote multiplexer")?
            .resolve_remote_program()
    }

    /// Retains the session key needed by a remote runtime's later control
    /// requests. It is never persisted or included in an SSH argument.
    pub fn set_session_secret(&self, secret: Option<&SessionSecret>) {
        if self.stream_only {
            *self.session_secret.lock().recover_poison() = secret.cloned();
        }
    }

    pub fn session_secret(&self) -> Option<SessionSecret> {
        self.session_secret.lock().recover_poison().clone()
    }

    /// Makes a request-capable clone that retains the same logical client ID.
    /// Subscription recovery uses this so a daemon replacement or remote SSH
    /// forward restart does not look like a second viewer.
    pub(super) fn reconnect_client(&self) -> Self {
        Self {
            endpoint: Mutex::new(self.endpoint_snapshot()),
            directory: self.directory.clone(),
            remote: self.remote.clone(),
            client_id: self.client_id.clone(),
            stream_only: self.stream_only,
            session_secret: self.session_secret.clone(),
            operation_counter: self.operation_counter.clone(),
        }
    }
}
