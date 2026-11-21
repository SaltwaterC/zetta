//! Requests that change a shared session: snapshots, operations, and the
//! panes viewers add to it.

use super::*;

impl Client {
    /// Allocates an operation ID that remains unique across request retries and
    /// subscription reconnects for this logical client.
    pub fn next_shared_operation_id(&self) -> SharedOperationId {
        SharedOperationId::new(
            self.client_id.clone(),
            self.operation_counter.fetch_add(1, Ordering::Relaxed),
        )
    }

    /// Fetches the daemon's complete shared-session snapshot.
    pub fn shared_snapshot(&self, session_id: u64) -> Result<SharedSessionState> {
        let secret = self.session_secret();
        let mut connection =
            self.open_with_session_secret(Request::SharedSnapshot { session_id }, secret.as_ref())?;
        match self.receive(&mut connection)?.0 {
            Response::SharedSnapshot { state } => Ok(state),
            Response::AuthenticationRequired => {
                anyhow::bail!("the shared session requires an authentication secret")
            }
            Response::AuthenticationFailed => anyhow::bail!("shared session authentication failed"),
            other => Err(refusal(other, "shared snapshot")),
        }
    }

    /// Leaves a shared session without changing any daemon-owned pane. This is
    /// the orderly form of closing a remote tab; dropping the pane streams is
    /// still the fallback when a window disappears abruptly.
    pub fn leave_shared(&self, session_id: u64) -> Result<()> {
        let secret = self.session_secret();
        let mut connection =
            self.open_with_session_secret(Request::LeaveShared { session_id }, secret.as_ref())?;
        match self.receive(&mut connection)?.0 {
            Response::Ok => Ok(()),
            Response::AuthenticationRequired => {
                anyhow::bail!("the shared session requires an authentication secret")
            }
            Response::AuthenticationFailed => anyhow::bail!("shared session authentication failed"),
            other => Err(refusal(other, "leaving shared session")),
        }
    }

    /// Applies a mutation to canonical shared state. Whole-layout operations
    /// require an exact base revision; operations with stable pane/divider
    /// targets are rebased by the daemon and return a conflict snapshot when
    /// that target disappeared.
    pub fn apply_shared(
        &self,
        session_id: u64,
        base_revision: SessionRevision,
        operation: SharedSessionOperation,
    ) -> Result<SharedOperationResult> {
        let request = SharedSessionOperationRequest {
            session_id,
            base_revision,
            operation_id: self.next_shared_operation_id(),
            operation,
        };
        self.apply_shared_with_request(request)
    }

    pub fn apply_shared_with_request(
        &self,
        request: SharedSessionOperationRequest,
    ) -> Result<SharedOperationResult> {
        let secret = self.session_secret();
        let mut connection =
            self.open_with_session_secret(Request::ApplyShared(request), secret.as_ref())?;
        match self.receive(&mut connection)?.0 {
            Response::SharedOperationApplied { state } => Ok(SharedOperationResult::Applied(state)),
            Response::SharedConflict { state } => Ok(SharedOperationResult::Conflict(state)),
            Response::AuthenticationRequired => {
                anyhow::bail!("the shared session requires an authentication secret")
            }
            Response::AuthenticationFailed => anyhow::bail!("shared session authentication failed"),
            other => Err(refusal(other, "shared operation")),
        }
    }

    /// Spawns a daemon-owned pane and keeps its shared byte stream open for the
    /// terminal. This is the remote equivalent of [`Self::spawn`].
    pub fn spawn_shared(&self, request: SharedSpawnRequest) -> Result<SharedSpawnedPane> {
        let secret = self.session_secret();
        let mut connection =
            self.open_with_session_secret(Request::SpawnShared(request.clone()), secret.as_ref())?;
        let (response, _) = self.receive(&mut connection)?;
        match response {
            Response::SharedSpawned {
                session_id,
                pane_id,
                child_pid,
                replay_length,
                shared_state,
                ..
            } => {
                let replay = connection.read_exact(replay_length)?;
                Ok(SharedSpawnedPane {
                    pane: SharedPane::from_connection(
                        session_id, pane_id, child_pid, connection, replay, None,
                    ),
                    state: shared_state,
                })
            }
            Response::SharedConflict { state } => anyhow::bail!(
                "shared spawn conflicted at revision {}; refresh the session snapshot",
                state.revision.0
            ),
            Response::AuthenticationRequired => {
                anyhow::bail!("the shared session requires an authentication secret")
            }
            Response::AuthenticationFailed => anyhow::bail!("shared session authentication failed"),
            other => Err(refusal(other, "shared spawn")),
        }
    }

    /// Creates a complete daemon-owned shared session without attaching any
    /// pane. A lost response is retried with the same operation ID, so the
    /// daemon returns the first committed session instead of starting another
    /// set of shells.
    pub fn create_shared(&self, request: CreateSharedRequest) -> Result<SharedCreated> {
        let secret = self.session_secret();
        let mut retried = false;
        loop {
            let mut connection = match self
                .open_with_session_secret(Request::CreateShared(request.clone()), secret.as_ref())
            {
                Ok(connection) => connection,
                Err(error) if !retried && is_transport_error(&error) => {
                    retried = true;
                    continue;
                }
                Err(error) => return Err(error),
            };
            match self.receive(&mut connection) {
                Ok((Response::SharedCreated { session_id, state }, _)) => {
                    return Ok(SharedCreated { session_id, state });
                }
                Ok((Response::Error { message }, _)) => anyhow::bail!("{message}"),
                Ok((other, _)) => {
                    anyhow::bail!("unexpected response to headless create: {other:?}")
                }
                Err(error) if !retried && is_transport_error(&error) => {
                    retried = true;
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Atomically commits daemon-owned panes, then leaves stream attachment to
    /// the ordinary shared attach path for both local and remote callers.
    pub fn spawn_shared_batch(
        &self,
        request: SharedSpawnBatchRequest,
    ) -> Result<SharedBatchResult> {
        let secret = self.session_secret();
        let mut connection =
            self.open_with_session_secret(Request::SpawnSharedBatch(request), secret.as_ref())?;
        match self.receive(&mut connection)?.0 {
            Response::SharedBatchSpawned { mappings, state } => {
                Ok(SharedBatchResult::Applied(SharedBatchSpawned {
                    mappings,
                    state,
                }))
            }
            Response::SharedConflict { state } => Ok(SharedBatchResult::Conflict(state)),
            Response::AuthenticationRequired => {
                anyhow::bail!("the shared session requires an authentication secret")
            }
            Response::AuthenticationFailed => anyhow::bail!("shared session authentication failed"),
            other => Err(refusal(other, "shared batch spawn")),
        }
    }

    /// Joins a pane through the shared relay even when this is a local client.
    /// This is the second half of a daemon-first shared batch transaction.
    pub fn attach_shared_with_secret(
        &self,
        session_id: u64,
        pane_id: u64,
        secret: Option<&SessionSecret>,
    ) -> Result<AttachOutcome> {
        self.attach_as_process(
            session_id,
            Some(pane_id),
            secret.cloned(),
            std::process::id(),
            true,
            None,
        )
    }

    /// Joins a pane as a relay showing it to `viewer`, which is a client
    /// somewhere else entirely: `zmux relay-pane` runs on the daemon's own host
    /// and copies the pane onto a Mosh link, so the window at the other end of
    /// that link never joins the pane's shared set itself.
    ///
    /// The daemon needs to be told, because a control request from that window
    /// — pasting an image — otherwise looks like one from a client that is not
    /// watching the pane at all. It holds for as long as this attachment does.
    pub fn attach_shared_relaying_for(
        &self,
        session_id: u64,
        pane_id: u64,
        secret: Option<&SessionSecret>,
        viewer: ClientId,
    ) -> Result<AttachOutcome> {
        self.attach_as_process(
            session_id,
            Some(pane_id),
            secret.cloned(),
            std::process::id(),
            true,
            Some(viewer),
        )
    }
}
