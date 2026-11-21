//! Requests about a session or a pane held by the multiplexer: spawning,
//! attaching, detaching, sharing, resuming, ending and resizing them.

use super::*;

impl Client {
    /// Tells the multiplexer that an attached pane was resized.
    ///
    /// Only meaningful where the console belongs to the multiplexer; on Unix
    /// the resize has already taken effect through the descriptor.
    pub fn resize(&self, session_id: u64, pane_id: u64, columns: u16, lines: u16) -> Result<()> {
        self.resize_with_secret(session_id, pane_id, columns, lines, None)
    }

    pub fn resize_with_secret(
        &self,
        session_id: u64,
        pane_id: u64,
        columns: u16,
        lines: u16,
        secret: Option<&SessionSecret>,
    ) -> Result<()> {
        let mut connection = self.open_with_session_secret(
            Request::Resize {
                session_id,
                pane_id,
                revision: None,
                columns,
                lines,
            },
            secret,
        )?;
        self.expect_ok(&mut connection, "resize")
    }

    pub fn set_console_palette(
        &self,
        session_id: u64,
        pane_id: u64,
        palette: ConsolePalette,
    ) -> Result<()> {
        self.set_console_palette_with_secret(session_id, pane_id, palette, None)
    }

    pub fn set_console_palette_with_secret(
        &self,
        session_id: u64,
        pane_id: u64,
        palette: ConsolePalette,
        secret: Option<&SessionSecret>,
    ) -> Result<()> {
        let mut connection = self.open_with_session_secret(
            Request::SetConsolePalette {
                session_id,
                pane_id,
                palette,
            },
            secret,
        )?;
        match self.receive(&mut connection)?.0 {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected palette response: {other:?}"),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn set_console_palette_as(
        &self,
        session_id: u64,
        pane_id: u64,
        palette: ConsolePalette,
        client_process_id: u32,
    ) -> Result<()> {
        let mut connection = self.open_as(
            Request::SetConsolePalette {
                session_id,
                pane_id,
                palette,
            },
            client_process_id,
        )?;
        match self.receive(&mut connection)?.0 {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected palette response: {other:?}"),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_console_palette_for_process(
        &self,
        session_id: u64,
        pane_id: u64,
        palette: ConsolePalette,
        client_process_id: u32,
    ) -> Result<()> {
        self.set_console_palette_as(session_id, pane_id, palette, client_process_id)
    }

    /// Sends a screen checkpoint for a pane this process is showing.
    ///
    /// During a revoke this is the screen handed back so the multiplexer can
    /// resume reading and relay the pane to every client that attaches. A live
    /// share uses the same request before publishing the session, while this
    /// process remains exclusive, so disk persistence has a current screen to
    /// save even if the window never gets as far as backgrounding the tab.
    ///
    /// `columns`/`lines` are the size the pane was being shown at, which the
    /// multiplexer records as its current applied size. A shared client still
    /// reports its own initialized layout separately after it joins.
    pub fn send_snapshot(
        &self,
        session_id: u64,
        pane_id: u64,
        bytes: Vec<u8>,
        columns: u16,
        lines: u16,
    ) -> Result<()> {
        let mut connection = self.open_attested(
            Request::Snapshot(crate::messages::SnapshotRequest {
                session_id,
                pane_id,
                length: bytes.len(),
                columns,
                lines,
            }),
            std::process::id(),
        )?;
        connection.write_all(&bytes)?;
        self.expect_ok(&mut connection, "snapshot")
    }

    /// Stores a normalized clipboard image for a shared pane and returns the
    /// absolute path written by the daemon. The upload uses its own
    /// authenticated request connection so it cannot interfere with the
    /// pane's one-way output/input relay.
    pub fn store_image(&self, session_id: u64, pane_id: u64, bytes: Vec<u8>) -> Result<String> {
        anyhow::ensure!(
            !bytes.is_empty() && bytes.len() <= MAX_IMAGE_BYTES,
            "image payload must be between 1 byte and {MAX_IMAGE_BYTES} bytes"
        );
        let secret = self.session_secret();
        let mut connection = self.open_attested_with_secret(
            Request::StoreImage(crate::messages::StoreImageRequest {
                session_id,
                pane_id,
                length: bytes.len(),
            }),
            std::process::id(),
            secret.as_ref(),
        )?;
        // The daemon refuses an unknown session, or a pane this client is not
        // watching, as soon as it has read the request — which can be before
        // this payload has finished being written. The write then fails with a
        // broken pipe that says nothing about why, and that is what the user
        // would be shown instead of the refusal. So the response is the
        // authority: a failed write is only reported when there is no response
        // to read.
        let written = connection.write_all(&bytes);
        let response = match self.receive(&mut connection) {
            Ok((response, _)) => response,
            Err(error) => return Err(written.err().map_or(error, anyhow::Error::from)),
        };
        match response {
            Response::ImageStored { path } => Ok(path),
            Response::AuthenticationRequired => {
                anyhow::bail!("the remote session requires authentication for image paste")
            }
            Response::AuthenticationFailed => {
                anyhow::bail!("the remote session rejected its authentication secret")
            }
            other => Err(refusal(other, "image paste")),
        }
    }

    /// Starts a process under the multiplexer and takes its terminal.
    pub fn spawn(&self, request: SpawnRequest) -> Result<AttachedPane> {
        let mut connection = self.open(Request::Spawn(request))?;
        let (response, mut descriptors) = self.receive(&mut connection)?;
        match response {
            Response::Spawned {
                session_id,
                pane_id,
                child_pid,
                handles,
            } => {
                let terminal = claim_terminal(&mut descriptors, handles)?;
                Ok(AttachedPane {
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
                    replay: Vec::new(),
                })
            }
            other => Err(refusal(other, "spawn")),
        }
    }

    /// Takes a held pane's terminal back, with everything retained since it was
    /// detached.
    /// Takes a held pane back. `pane_id` is `None` for the session's first
    /// pane, which is where an attach starts.
    pub fn attach(
        &self,
        session_id: u64,
        pane_id: Option<u64>,
        secret: Option<String>,
    ) -> Result<AttachOutcome> {
        self.attach_as_process(
            session_id,
            pane_id,
            secret.map(SessionSecret::new),
            std::process::id(),
            false,
            None,
        )
    }

    /// Attaches with a secret kept in zeroizing memory by the caller. The
    /// serialized request is discarded as soon as the handshake is complete.
    pub fn attach_with_secret(
        &self,
        session_id: u64,
        pane_id: Option<u64>,
        secret: Option<&SessionSecret>,
    ) -> Result<AttachOutcome> {
        self.attach_as_process(
            session_id,
            pane_id,
            secret.cloned(),
            std::process::id(),
            false,
            None,
        )
    }

    /// Gives a session back to the multiplexer to hold.
    ///
    /// The caller must already have dropped the panes' descriptors: the
    /// multiplexer resumes reading as soon as this returns, and two readers
    /// would split the output between them.
    pub fn detach(
        &self,
        session_id: u64,
        summary: BackgroundSessionSummary,
        state: serde_json::Value,
        protection: Option<&SessionAuthentication>,
        snapshots: Vec<(u64, Vec<u8>)>,
    ) -> Result<()> {
        let request = DetachRequest {
            session_id,
            summary,
            state,
            verifier: protection.map(|protection| protection.verifier().to_owned()),
            key_envelope: protection
                .and_then(|protection| protection.key_envelope().map(str::to_owned)),
            snapshots: snapshots
                .iter()
                .map(|(pane_id, bytes)| PaneSnapshot {
                    pane_id: *pane_id,
                    length: bytes.len(),
                })
                .collect(),
        };
        let mut connection = self.open_attested(Request::Detach(request), std::process::id())?;
        for (_, bytes) in &snapshots {
            connection.write_all(bytes)?;
        }
        match self.receive(&mut connection)?.0 {
            Response::Detached => Ok(()),
            other => Err(refusal(other, "detach")),
        }
    }

    /// Detaches on behalf of another process.
    ///
    /// Only useful for exercising what a session's scope does after the window
    /// that backgrounded it has gone: the session belongs to the process that
    /// detached it, and a test cannot exit itself to prove that outlives it.
    #[cfg(any(test, feature = "test-support"))]
    pub fn detach_as(
        &self,
        session_id: u64,
        summary: BackgroundSessionSummary,
        state: serde_json::Value,
        protection: Option<&SessionAuthentication>,
        client_process_id: u32,
    ) -> Result<()> {
        let request = DetachRequest {
            session_id,
            summary,
            state,
            verifier: protection.map(|protection| protection.verifier().to_owned()),
            key_envelope: protection
                .and_then(|protection| protection.key_envelope().map(str::to_owned)),
            snapshots: Vec::new(),
        };
        let mut connection = self.open_attested(Request::Detach(request), client_process_id)?;
        match self.receive(&mut connection)?.0 {
            Response::Detached => Ok(()),
            other => Err(refusal(other, "detach")),
        }
    }

    /// Offers, or withdraws, a session this client is still showing.
    ///
    /// Unlike [`Client::detach`] this changes nothing about who is reading the
    /// session's terminals: it publishes what a joining client needs and says
    /// whether the session is on offer. Only a client holding one of the
    /// session's panes may send it.
    pub fn share(
        &self,
        session_id: u64,
        summary: BackgroundSessionSummary,
        state: serde_json::Value,
        protection: Option<&SessionAuthentication>,
        offered: bool,
    ) -> Result<()> {
        self.share_with_secret(session_id, summary, state, protection, offered, None)
    }

    pub fn share_with_secret(
        &self,
        session_id: u64,
        summary: BackgroundSessionSummary,
        state: serde_json::Value,
        protection: Option<&SessionAuthentication>,
        offered: bool,
        secret: Option<&SessionSecret>,
    ) -> Result<()> {
        let mut connection = self.open_with_session_secret(
            Request::Share(crate::messages::ShareRequest {
                session_id,
                summary,
                state,
                verifier: protection.map(|protection| protection.verifier().to_owned()),
                key_envelope: protection
                    .and_then(|protection| protection.key_envelope().map(str::to_owned)),
                offered,
            }),
            secret,
        )?;
        self.expect_ok(&mut connection, "share")
    }

    pub fn list(&self) -> Result<Vec<BackgroundSessionSummary>> {
        self.list_with_secret(None)
    }

    pub fn list_with_secret(
        &self,
        secret: Option<&SessionSecret>,
    ) -> Result<Vec<BackgroundSessionSummary>> {
        let mut connection = self.open_with_session_secret(Request::List, secret)?;
        match self.receive(&mut connection)?.0 {
            Response::Sessions { sessions, .. } => Ok(sessions),
            other => Err(refusal(other, "list")),
        }
    }

    pub fn list_with_restorable(
        &self,
    ) -> Result<(Vec<BackgroundSessionSummary>, Vec<RestorableSessionRecord>)> {
        let mut connection = self.open(Request::List)?;
        match self.receive(&mut connection)?.0 {
            Response::Sessions {
                sessions,
                restorable,
            } => Ok((sessions, restorable)),
            other => Err(refusal(other, "list")),
        }
    }

    #[cfg(feature = "session-persistence")]
    pub fn resume(
        &self,
        record_id: u64,
        identity_paths: &[PathBuf],
    ) -> Result<crate::persistence::PersistedSession> {
        let identities = crate::persistence::IdentitySet::from_paths(identity_paths)?;
        let persisted = crate::persistence::load_session_from_directory(
            &self.directory,
            record_id,
            &identities,
        )?;
        // Only asked for when the record's protection is a typed secret. An
        // automatically protected one is opened by `resume_loaded` from the
        // envelope inside the record, using the identity that just decrypted it.
        let secret = persisted
            .verifier
            .as_ref()
            .filter(|_| persisted.key_envelope.is_none())
            .map(|_| secret_prompt::prompt_for_reconnect_secret())
            .transpose()?;
        self.resume_loaded(persisted, secret.as_ref(), &identities)
    }

    /// Resumes a record after the caller has handled any UI-specific secret
    /// prompt. The age identity is still loaded and the record is still
    /// decrypted here, so the daemon receives neither of them.
    #[cfg(feature = "session-persistence")]
    pub fn resume_with_secret(
        &self,
        record_id: u64,
        identity_paths: &[PathBuf],
        secret: Option<&SessionSecret>,
    ) -> Result<crate::persistence::PersistedSession> {
        self.resume_with_secret_and_identity_passphrases(record_id, identity_paths, &[], secret)
    }

    /// Resumes a record after the caller has handled both UI-specific prompts.
    /// Identity passphrases are positional with `identity_paths`; they stay in
    /// the client and are never included in the daemon request.
    #[cfg(feature = "session-persistence")]
    pub fn resume_with_secret_and_identity_passphrases(
        &self,
        record_id: u64,
        identity_paths: &[PathBuf],
        identity_passphrases: &[Option<SessionSecret>],
        secret: Option<&SessionSecret>,
    ) -> Result<crate::persistence::PersistedSession> {
        // Only what the caller collected: this is the entry point a window uses,
        // and `age`'s own fallback would read `/dev/tty` from the UI thread.
        let identities = crate::persistence::IdentitySet::from_supplied_passphrases(
            identity_paths,
            identity_passphrases,
        )?;
        let persisted = crate::persistence::load_session_from_directory(
            &self.directory,
            record_id,
            &identities,
        )?;
        self.resume_loaded(persisted, secret, &identities)
    }

    /// Sends a decrypted record to the daemon, settling its secret first.
    ///
    /// A record protected automatically carries its own way in, and the
    /// identities that opened the record are the identities that open that too —
    /// so the secret is recovered here rather than asked for by every caller. A
    /// caller that already has one (a typed secret, or one it recovered itself)
    /// keeps it: this fills a gap, it does not overrule.
    #[cfg(feature = "session-persistence")]
    pub(super) fn resume_loaded(
        &self,
        persisted: crate::persistence::PersistedSession,
        secret: Option<&SessionSecret>,
        identities: &crate::persistence::IdentitySet,
    ) -> Result<crate::persistence::PersistedSession> {
        let recovered = match (secret, persisted.key_envelope.as_deref()) {
            (None, Some(envelope)) => Some(crate::auto_protect::open(envelope, identities)?),
            _ => None,
        };
        let secret = secret.or(recovered.as_ref());
        let request = Request::Resume(Box::new(ResumeRequest {
            record_id: persisted.id,
            summary: persisted.summary.clone(),
            state: persisted.state.clone(),
            shared_state: persisted.shared_state.clone(),
            verifier: persisted.verifier.clone(),
            key_envelope: persisted.key_envelope.clone(),
            failed_authentications: persisted.failed_authentications,
            backoff_seconds: persisted.backoff_seconds,
            created_at: persisted.created_at,
            updated_at: persisted.updated_at,
            secret: secret.cloned(),
            snapshots: persisted
                .snapshots
                .iter()
                .map(|snapshot| ResumeSnapshot {
                    pane_id: snapshot.pane_id,
                    length: snapshot.bytes.len(),
                })
                .collect(),
        }));
        let mut connection = self.open_attested(request, std::process::id())?;
        for snapshot in &persisted.snapshots {
            connection.write_all(&snapshot.bytes)?;
        }
        match self.receive(&mut connection)?.0 {
            Response::Resumed { .. } => Ok(persisted),
            Response::AuthenticationRequired => {
                anyhow::bail!("the encrypted session is protected and needs its session secret")
            }
            Response::AuthenticationFailed => anyhow::bail!("session authentication failed"),
            other => Err(refusal(other, "resume")),
        }
    }

    pub fn kill(&self, session_id: u64) -> Result<()> {
        self.kill_with_secret(session_id, None)
    }

    pub fn kill_with_secret(&self, session_id: u64, secret: Option<&SessionSecret>) -> Result<()> {
        let mut connection = self.open_with_session_secret(Request::Kill { session_id }, secret)?;
        self.expect_ok(&mut connection, "kill")
    }

    /// Removes a session from the catalog without killing it.
    /// Scopes a backgrounded session to one process, or shares it with all.
    ///
    /// `protection` is what a joining process will have to present, and is
    /// required when sharing a session that has none.
    pub fn set_session_scope(
        &self,
        session_id: u64,
        shared: bool,
        protection: Option<&SessionAuthentication>,
    ) -> Result<()> {
        self.set_session_scope_with_secret(session_id, shared, protection, None)
    }

    pub fn set_session_scope_with_secret(
        &self,
        session_id: u64,
        shared: bool,
        protection: Option<&SessionAuthentication>,
        secret: Option<&SessionSecret>,
    ) -> Result<()> {
        let mut connection = self.open_with_session_secret(
            Request::SetSessionScope(crate::messages::SessionScopeRequest {
                session_id,
                shared,
                verifier: protection.map(|protection| protection.verifier().to_owned()),
                key_envelope: protection
                    .and_then(|protection| protection.key_envelope().map(str::to_owned)),
            }),
            secret,
        )?;
        self.expect_ok(&mut connection, "a session scope change")
    }

    pub fn forget(&self, session_id: u64) -> Result<()> {
        self.forget_with_secret(session_id, None)
    }

    pub fn forget_with_secret(
        &self,
        session_id: u64,
        secret: Option<&SessionSecret>,
    ) -> Result<()> {
        let mut connection =
            self.open_with_session_secret(Request::Forget { session_id }, secret)?;
        self.expect_ok(&mut connection, "forget")
    }

    /// Asks what the multiplexer currently knows about these panes.
    pub fn pane_states(&self, pane_ids: Vec<u64>) -> Result<Vec<PaneStateReport>> {
        let secret = self.session_secret();
        self.pane_states_with_secret(pane_ids, secret.as_ref())
    }

    pub fn pane_states_with_secret(
        &self,
        pane_ids: Vec<u64>,
        secret: Option<&SessionSecret>,
    ) -> Result<Vec<PaneStateReport>> {
        let mut connection =
            self.open_with_session_secret(Request::PaneStates { pane_ids }, secret)?;
        match self.receive(&mut connection)?.0 {
            Response::PaneStates { panes } => Ok(panes),
            other => Err(refusal(other, "pane states")),
        }
    }

    /// Hands a pane back because the window showing it has closed.
    pub fn close_pane(&self, session_id: u64, pane_id: u64) -> Result<()> {
        let mut connection = self.open(Request::ClosePane {
            session_id,
            pane_id,
        })?;
        self.expect_ok(&mut connection, "close pane")
    }

    /// Permanently removes a pane this client holds exclusively.
    pub fn discard_pane(&self, session_id: u64, pane_id: u64) -> Result<()> {
        let mut connection = self.open(Request::DiscardPane {
            session_id,
            pane_id,
        })?;
        self.expect_ok(&mut connection, "discard pane")
    }

    /// Takes a shared pane's terminal back, in answer to [`Event::Grant`].
    ///
    /// Sent on its own connection: the shared one is being retired, and the
    /// multiplexer closes its end of it once the last relayed frame has gone, which
    /// is how this client knows it has everything before it starts reading the
    /// terminal itself.
    pub fn take_exclusive(&self, session_id: u64, pane_id: u64) -> Result<AttachedPane> {
        let mut client = self.reconnect_client();
        client.client_id = ClientId::random()?;
        let mut connection = client.open(Request::TakeExclusive {
            session_id,
            pane_id,
        })?;
        let (response, mut descriptors) = self.receive(&mut connection)?;
        match response {
            Response::Attached {
                pane_id,
                child_pid,
                replay_length,
                handles,
                ..
            } => {
                let terminal = claim_terminal(&mut descriptors, handles)?;
                // Nothing to replay: everything the multiplexer read went to this
                // client over the relay, and replaying it would print the pane's
                // recent output a second time.
                anyhow::ensure!(
                    replay_length == 0,
                    "a granted pane must not carry a replay, but {replay_length} bytes came with it"
                );
                Ok(AttachedPane {
                    session_id,
                    pane_id,
                    child_pid,
                    attachment_client_id: client.client_id.clone(),
                    #[cfg(unix)]
                    descriptor: terminal,
                    #[cfg(windows)]
                    conout: terminal.0,
                    #[cfg(windows)]
                    conin: terminal.1,
                    replay: Vec::new(),
                })
            }
            other => Err(refusal(other, "taking a pane back")),
        }
    }

    /// Gives a taken pane back, after this client turned out not to be able to
    /// show it.
    ///
    /// The multiplexer stops reading a pane the moment it hands the descriptor
    /// over, because the client that took it is the one reading it from then
    /// on. A client that took one and could not adopt it has to say so, or the
    /// pane is read by nobody: its pty fills and the program on the other end
    /// of it stops reading input for every viewer, not just this one.
    pub fn release_exclusive(
        &self,
        session_id: u64,
        pane_id: u64,
        attachment_client_id: &ClientId,
    ) -> Result<()> {
        let mut client = self.reconnect_client();
        client.client_id = attachment_client_id.clone();
        let mut connection = client.open(Request::ReleaseExclusive {
            session_id,
            pane_id,
        })?;
        self.expect_ok(&mut connection, "releasing a pane")
    }
}
