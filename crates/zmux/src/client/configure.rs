//! Configuring the running daemon, upgrading it in place, and shutting it
//! down.

use super::*;

impl Client {
    /// Applies the retention settings selected by the current client to a
    /// daemon that may have been started by an earlier client.
    ///
    /// A daemon from before `Configure` existed rejects the request as an
    /// unknown variant. On Unix, that is recoverable: replace the daemon in
    /// place, wait for the new image to answer, and send the same effective
    /// settings again. The retry deliberately uses [`Self::configure_raw`]
    /// rather than this method, because the replacement is expected to be
    /// current and must not trigger a second upgrade while the first one is
    /// settling.
    pub fn configure(
        &self,
        retention: Retention,
        persistence_recipients: Vec<String>,
    ) -> Result<()> {
        retention.validate()?;
        match self.configure_raw(retention, persistence_recipients.clone()) {
            Ok(()) => Ok(()),
            Err(error) if is_unsupported_configure(&error) => {
                #[cfg(unix)]
                {
                    self.upgrade()
                        .context("upgrading the multiplexer to apply retention settings")?;
                    self.configure_after_upgrade(retention, persistence_recipients)
                }
                #[cfg(not(unix))]
                {
                    Err(error)
                }
            }
            Err(error) => Err(error),
        }
    }

    /// Sends a single wire-level configure request.
    ///
    /// This is intentionally separate from [`Self::configure`]. The latter
    /// may have just replaced the daemon, so retrying it here would attempt a
    /// recursive upgrade instead of merely applying the requested settings to
    /// the replacement.
    pub(super) fn configure_raw(
        &self,
        retention: Retention,
        persistence_recipients: Vec<String>,
    ) -> Result<()> {
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        let request = Request::Configure {
            retention,
            persistence_recipients,
        };
        loop {
            self.ping_until_ready(deadline)?;
            let endpoint = self.endpoint_snapshot();
            let mut connection =
                self.open_as_with_endpoint(request.clone(), std::process::id(), &endpoint)?;
            let (response, _) = self.receive(&mut connection)?;
            if matches!(
                &response,
                Response::Error { message } if message == "invalid multiplexer token"
            ) {
                self.refresh_endpoint_after_token(&endpoint, deadline)
                    .context("refreshing the multiplexer endpoint after an invalid token")?;
                continue;
            }
            return match response {
                Response::Ok => Ok(()),
                other => Err(refusal(other, "daemon configuration")),
            };
        }
    }

    /// Resolves and applies the persistence settings from the current Zetta
    /// configuration without creating a second client connection.
    #[cfg(feature = "session-persistence")]
    pub fn configure_with_retention_and_persistence(
        &self,
        retention: Retention,
        persistence: PersistenceOptions,
    ) -> Result<()> {
        retention.validate()?;
        let recipients = if matches!(retention, Retention::Disk) {
            crate::persistence::resolve_recipient_strings(&persistence.recipients)?
        } else {
            Vec::new()
        };
        self.configure(retention, recipients)
    }

    /// Applies persistence settings while allowing a temporary GitHub lookup
    /// failure to leave the daemon in the supplied in-memory mode.
    #[cfg(feature = "session-persistence")]
    pub fn configure_with_retention_and_persistence_resilient(
        &self,
        retention: Retention,
        persistence: PersistenceOptions,
        fallback_retention: Retention,
    ) -> Result<RetentionConfiguration> {
        let resolved = ResolvedRetention::resolve(retention, &persistence, fallback_retention)?;
        self.configure_resolved(&resolved)
    }

    /// Sends an already resolved policy, so a caller configuring several
    /// daemons from one configuration resolves its recipients once.
    #[cfg(feature = "session-persistence")]
    pub fn configure_resolved(
        &self,
        resolved: &ResolvedRetention,
    ) -> Result<RetentionConfiguration> {
        self.configure(resolved.effective_retention, resolved.recipients.clone())?;
        Ok(resolved.configuration())
    }

    /// Asks the daemon to replace itself, keeping its sessions.
    pub fn upgrade(&self) -> Result<()> {
        let mut connection = self.open(Request::Upgrade)?;
        let response = self.receive(&mut connection)?.0;
        match response {
            Response::Ok => {
                Self::wait_for_upgrade_disconnect(connection.stream())?;
                Ok(())
            }
            other => Err(refusal(other, "upgrade")),
        }
    }

    /// Waits until the daemon that acknowledged an upgrade has left the request
    /// connection behind.
    ///
    /// The successful response is sent before execv, because the old image
    /// cannot answer after it replaces itself. Returning as soon as that
    /// response arrives leaves a small window in which a follow-up request can
    /// still land in the old listener — especially on macOS, where the endpoint
    /// and socket path are deliberately unchanged while the replacement
    /// rebinds. The close is the completion signal that distinguishes the old
    /// image from its replacement.
    pub(super) fn wait_for_upgrade_disconnect(mut stream: impl io::Read) -> Result<()> {
        let mut byte = [0; 1];
        loop {
            match stream.read(&mut byte) {
                Ok(0) => return Ok(()),
                Ok(_) => anyhow::bail!("the multiplexer sent data after accepting its upgrade"),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                // After the acknowledgement, a peer reset is also the old
                // daemon leaving this connection behind. Windows can report
                // WSAECONNRESET here instead of an orderly EOF during upgrade.
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::ConnectionReset
                            | io::ErrorKind::ConnectionAborted
                            | io::ErrorKind::BrokenPipe
                            | io::ErrorKind::UnexpectedEof
                    ) =>
                {
                    return Ok(());
                }
                Err(error) => {
                    return Err(error).context("waiting for the multiplexer to finish upgrading");
                }
            }
        }
    }

    pub fn shutdown(&self) -> Result<()> {
        let mut connection = self.open(Request::Shutdown)?;
        self.expect_ok(&mut connection, "shutdown")
    }
}
