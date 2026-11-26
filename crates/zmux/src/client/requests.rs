//! One request on its own connection: opening it, attesting the peer where
//! the platform needs that, framing the envelope, and reading the reply.

use super::*;
use zetta_sync::RecoverPoison as _;

impl Client {
    /// Attaches a pane on behalf of another process.
    ///
    /// Only useful for exercising what happens when the process holding a
    /// pane is not the one that asked for it — which is what a window dying
    /// mid-attach looks like from the multiplexer's side.
    #[cfg(any(test, feature = "test-support"))]
    pub fn attach_as(
        &self,
        session_id: u64,
        pane_id: u64,
        client_process_id: u32,
        secret: Option<String>,
    ) -> Result<AttachOutcome> {
        // Test callers use one `Client` to stand in for several processes. A
        // real process has one logical client, but those stand-ins still need
        // independent IDs so closing or resizing one shared connection cannot
        // address the other one.
        let mut client = self.reconnect_client();
        client.client_id = ClientId::random()?;
        client.attach_as_process(
            session_id,
            Some(pane_id),
            secret.map(SessionSecret::new),
            client_process_id,
            false,
            None,
        )
    }

    /// Joins a pane through the shared relay on behalf of another process.
    ///
    /// [`Self::attach_as`] with the shared attach [`Self::attach_shared_with_secret`]
    /// forces, for the same reason: a test that needs a viewer the daemon will
    /// not confuse with the test's own client needs it to carry both a
    /// different process and a different client ID.
    #[cfg(any(test, feature = "test-support"))]
    pub fn attach_shared_as(
        &self,
        session_id: u64,
        pane_id: u64,
        client_process_id: u32,
        secret: Option<String>,
    ) -> Result<AttachOutcome> {
        let mut client = self.reconnect_client();
        client.client_id = ClientId::random()?;
        client.attach_as_process(
            session_id,
            Some(pane_id),
            secret.map(SessionSecret::new),
            client_process_id,
            true,
            None,
        )
    }

    pub(super) fn attach_as_process(
        &self,
        session_id: u64,
        pane_id: Option<u64>,
        secret: Option<SessionSecret>,
        client_process_id: u32,
        force_shared: bool,
        relaying_for: Option<ClientId>,
    ) -> Result<AttachOutcome> {
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        let request = Request::Attach(crate::messages::AttachRequest {
            session_id,
            pane_id,
            secret,
            force_shared,
            relaying_for,
        });
        loop {
            self.ping_until_ready(deadline)?;
            let endpoint = self.endpoint_snapshot();
            let mut connection =
                self.open_as_with_endpoint(request.clone(), client_process_id, &endpoint)?;
            let (response, mut descriptors) = self.receive(&mut connection)?;
            if matches!(
                &response,
                Response::Error { message } if message == "invalid multiplexer token"
            ) {
                self.refresh_endpoint_after_token(&endpoint, deadline)
                    .context("refreshing the multiplexer endpoint after an invalid token")?;
                continue;
            }
            return match response {
                Response::Attached {
                    pane_id,
                    child_pid,
                    replay_length,
                    state,
                    summary,
                    handles,
                } => {
                    let terminal = claim_terminal(&mut descriptors, handles)?;
                    let replay = connection.read_exact(replay_length)?;
                    Ok(AttachOutcome::Attached {
                        pane: AttachedPane {
                            session_id,
                            pane_id,
                            child_pid,
                            attachment_client_id: self.client_id.clone(),
                            #[cfg(unix)]
                            descriptor: terminal,
                            #[cfg(windows)]
                            conout: terminal.0,
                            #[cfg(windows)]
                            conin: terminal.1,
                            replay,
                        },
                        state,
                        summary: *summary,
                    })
                }
                Response::SharedAttached {
                    pane_id,
                    child_pid,
                    replay_length,
                    state,
                    summary,
                    columns,
                    lines,
                } => {
                    let replay = connection.read_exact(replay_length)?;
                    Ok(AttachOutcome::SharedAttached {
                        pane: SharedPane::from_connection(
                            session_id,
                            pane_id,
                            child_pid,
                            connection,
                            replay,
                            Some((columns, lines)),
                        ),
                        state,
                        summary: *summary,
                    })
                }
                Response::AuthenticationRequired => Ok(AttachOutcome::AuthenticationRequired),
                Response::AuthenticationFailed => Ok(AttachOutcome::AuthenticationFailed),
                other => Err(refusal(other, "attach")),
            };
        }
    }

    pub(super) fn open(&self, request: Request) -> Result<Connection> {
        self.open_as(request, std::process::id())
    }

    /// The protocol version the local multiplexer publishes right now.
    ///
    /// Re-read rather than taken from the cached endpoint, because the only
    /// caller asks precisely when a replacement has published a different one.
    /// A remote client has no local endpoint file to read, and the answer is
    /// only ever used to phrase a message, so `None` is a fine answer.
    pub(super) fn published_protocol_version(&self) -> Option<u32> {
        if self.remote.is_some() {
            return None;
        }
        Endpoint::read(&endpoint_path(&self.directory))
            .ok()
            .map(|endpoint| endpoint.protocol_version)
    }

    pub(super) fn endpoint_snapshot(&self) -> Endpoint {
        self.endpoint.lock().recover_poison().clone()
    }

    pub(super) fn replace_endpoint(&self, endpoint: Endpoint) {
        *self.endpoint.lock().recover_poison() = endpoint;
    }

    /// Refreshes the cached local endpoint when a transport failure finds that
    /// the daemon has published a replacement socket or token. A missing or
    /// partially written endpoint is transient during a restart, so leave the
    /// current value in place and let the retry loop try again.
    pub(super) fn refresh_endpoint_if_changed(&self, previous: &Endpoint) -> bool {
        if self.remote.is_some() {
            return false;
        }
        let Ok(endpoint) = Endpoint::read(&endpoint_path(&self.directory)) else {
            return false;
        };
        if endpoint == *previous {
            return false;
        }
        self.replace_endpoint(endpoint);
        true
    }

    /// Refreshes the cached endpoint after the daemon has rejected a request
    /// before dispatching it.  Waiting for the file to change avoids replaying
    /// the same request against the same stale token in a tight loop.
    pub(super) fn refresh_endpoint_after_token(
        &self,
        previous: &Endpoint,
        deadline: Instant,
    ) -> Result<()> {
        if let Some(remote) = &self.remote {
            let endpoint = remote.refresh()?;
            self.replace_endpoint(endpoint);
            return Ok(());
        }
        let path = endpoint_path(&self.directory);
        let mut last_error = None;
        loop {
            match Endpoint::read(&path) {
                Ok(endpoint) if endpoint != *previous => {
                    self.replace_endpoint(endpoint);
                    return Ok(());
                }
                Ok(_) => {}
                Err(error) => last_error = Some(error),
            }
            if Instant::now() >= deadline {
                return Err(last_error.unwrap_or_else(|| {
                    anyhow::anyhow!(
                        "the multiplexer endpoint did not change after an invalid token"
                    )
                }));
            }
            thread::sleep(STARTUP_POLL);
        }
    }

    pub(super) fn open_ready(&self, request: Request) -> Result<Connection> {
        self.ping_until_ready(Instant::now() + STARTUP_TIMEOUT)?;
        self.open(request)
    }

    /// Opens a connection whose peer identity is settled before the request
    /// goes out.
    ///
    /// For the requests that stream raw bytes after their message: the
    /// multiplexer cannot interject a challenge into one of those, because the
    /// bytes would arrive where it was expecting the answer. Windows-only in
    /// effect — elsewhere the kernel identifies the peer from the socket and the
    /// exchange is a single round trip that changes nothing.
    pub(super) fn open_attested(
        &self,
        request: Request,
        client_process_id: u32,
    ) -> Result<Connection> {
        self.open_attested_with_secret(request, client_process_id, None)
    }

    /// Opens a streamed request after establishing the peer identity, while
    /// carrying the session secret in the real request envelope. The
    /// attestation handshake must happen first because streamed payload bytes
    /// leave no place for the daemon to challenge the peer.
    pub(super) fn open_attested_with_secret(
        &self,
        request: Request,
        client_process_id: u32,
        secret: Option<&SessionSecret>,
    ) -> Result<Connection> {
        #[cfg(windows)]
        {
            let mut connection = self.open_as(Request::Attest, client_process_id)?;
            self.expect_ok(&mut connection, "attestation")?;
            let endpoint = self.endpoint_snapshot();
            connection.send(&self.envelope(endpoint.token, client_process_id, request, secret))?;
            Ok(connection)
        }
        #[cfg(unix)]
        self.open_as_with_endpoint_and_secret(
            request,
            client_process_id,
            &self.endpoint_snapshot(),
            secret,
        )
    }

    /// Receives a response, answering an attestation challenge first if the
    /// multiplexer asks for one.
    ///
    /// Only a Windows release whose kernel does not answer "which process is
    /// on the other end of this socket" ever asks; everywhere else the kernel
    /// answers it about the socket itself. See
    /// [`crate::transport::PeerChallenge`].
    ///
    /// A remote client also tells its transport what the answer proved: any
    /// response but a token rejection shows the endpoint is current, which is
    /// what lets the next connection skip its probe, and a rejection or a
    /// broken connection means the next one has to be probed again.
    /// Receives the answer to a request whose only success is `Ok`.
    pub(super) fn expect_ok(&self, connection: &mut Connection, request: &str) -> Result<()> {
        match self.receive(connection)?.0 {
            Response::Ok => Ok(()),
            other => Err(refusal(other, request)),
        }
    }

    pub(super) fn receive(&self, connection: &mut Connection) -> Result<(Response, Descriptors)> {
        let received = Self::receive_response(connection);
        if let Some(remote) = &self.remote {
            match &received {
                Ok((Response::Error { message }, _)) if message == "invalid multiplexer token" => {
                    remote.distrust();
                }
                Ok(_) => remote.confirm(&self.endpoint_snapshot()),
                Err(error) if is_transport_error(error) => remote.distrust(),
                Err(_) => {}
            }
        }
        received
    }

    pub(super) fn receive_response(connection: &mut Connection) -> Result<(Response, Descriptors)> {
        let received = connection.receive::<Response>()?;
        #[cfg(windows)]
        if let Response::AttestationRequired { handle } = received.0 {
            let nonce = crate::transport::answer_challenge(handle)?;
            connection.send(&Request::Attested { nonce })?;
            return connection.receive::<Response>();
        }
        Ok(received)
    }

    pub(super) fn open_as(&self, request: Request, client_process_id: u32) -> Result<Connection> {
        let endpoint = self.endpoint_snapshot();
        self.open_as_with_endpoint(request, client_process_id, &endpoint)
    }

    pub(super) fn open_with_session_secret(
        &self,
        request: Request,
        secret: Option<&SessionSecret>,
    ) -> Result<Connection> {
        self.open_as_with_endpoint_and_secret(
            request,
            std::process::id(),
            &self.endpoint_snapshot(),
            secret,
        )
    }

    pub(super) fn open_as_with_endpoint(
        &self,
        request: Request,
        client_process_id: u32,
        endpoint: &Endpoint,
    ) -> Result<Connection> {
        self.open_as_with_endpoint_and_secret(request, client_process_id, endpoint, None)
    }

    pub(super) fn open_as_with_endpoint_and_secret(
        &self,
        mut request: Request,
        client_process_id: u32,
        endpoint: &Endpoint,
        secret: Option<&SessionSecret>,
    ) -> Result<Connection> {
        let (endpoint, stream) = if let Some(remote) = &self.remote {
            let (endpoint, stream) = remote.connect()?;
            self.replace_endpoint(endpoint.clone());
            (endpoint, stream)
        } else {
            (
                endpoint.clone(),
                Stream::connect(&endpoint.socket_path).context("connecting to the multiplexer")?,
            )
        };
        if let Some(remote) = &self.remote {
            remote.inherit_login_context(&mut request)?;
        }
        stream
            .set_read_timeout(Some(REQUEST_TIMEOUT))
            .context("setting the multiplexer request read timeout")?;
        stream
            .set_write_timeout(Some(REQUEST_TIMEOUT))
            .context("setting the multiplexer request write timeout")?;
        let mut connection = Connection::new(stream);
        connection.send(&self.envelope(endpoint.token, client_process_id, request, secret))?;
        Ok(connection)
    }

    pub(super) fn envelope(
        &self,
        token: String,
        client_process_id: u32,
        request: Request,
        secret: Option<&SessionSecret>,
    ) -> Envelope {
        Envelope {
            version: PROTOCOL_VERSION,
            token,
            // Named so a platform without descriptor passing can duplicate a
            // terminal's handles into this process instead.
            client_process_id,
            client_id: self.client_id.clone(),
            stream_only: self.stream_only,
            session_secret: secret.cloned(),
            request,
        }
    }

    pub(super) fn ping_with_endpoint(&self, endpoint: &Endpoint) -> Result<()> {
        let mut connection =
            self.open_as_with_endpoint(Request::Ping, std::process::id(), endpoint)?;
        self.expect_ok(&mut connection, "ping")
    }

    pub(super) fn ping_until_ready(&self, deadline: Instant) -> Result<()> {
        // `RemoteTransport::connect` already probes the forwarded daemon
        // whenever nothing has recently proven it. Running this separate ping
        // as well would add a probe to every remote request. Keep the local
        // retry loop, where the endpoint socket itself is the readiness
        // signal, but let the transport's probe be the remote one.
        if self.remote.is_some() {
            return Ok(());
        }
        loop {
            let endpoint = self.endpoint_snapshot();
            match self.ping_with_endpoint(&endpoint) {
                Ok(()) => {
                    // A daemon serves nothing until it has published its
                    // endpoint, so the file now names the one that answered.
                    // The snapshot may predate that: a Windows replacement
                    // keeps the socket and token, binds, and only then
                    // publishes its own process ID over its predecessor's.
                    self.refresh_endpoint_if_changed(&endpoint);
                    return Ok(());
                }
                Err(error) if is_invalid_token_error(&error) => {
                    self.refresh_endpoint_after_token(&endpoint, deadline)
                        .with_context(
                            || "refreshing the multiplexer endpoint after an invalid token",
                        )?;
                }
                Err(error) if is_transport_error(&error) && Instant::now() < deadline => {
                    if self.refresh_endpoint_if_changed(&endpoint) {
                        continue;
                    }
                    thread::sleep(STARTUP_POLL);
                }
                Err(error) => return Err(error),
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "the multiplexer did not become ready within {STARTUP_TIMEOUT:?}"
            );
        }
    }
}
