//! Windows' half of pane startup and peer checks: a pane's agent pipe, and
//! the attestation a client gives where the kernel does not report which
//! process connected.

use super::*;

/// The agent a Windows pane would have used had the daemon not given it a
/// pipe of its own: the requester's, else the daemon's, else none, which leaves
/// Windows OpenSSH's default.
pub(super) fn pane_agent_fallback(env: &HashMap<String, String>) -> Option<PathBuf> {
    env.get("SSH_AUTH_SOCK")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("SSH_AUTH_SOCK").map(PathBuf::from))
        .filter(|path| !path.as_os_str().is_empty())
}

/// Stops serving a discarded pane's agent pipe; see the Unix counterpart for
/// why this is not a `Drop`.
pub(in crate::server) fn remove_pane_agent_links(pane_id: u64) {
    super::agent_pipe::stop(pane_id);
}

/// Whether this request's answer could depend on who the peer is, when the
/// kernel did not say and the daemon cannot tell without asking.
///
/// Narrow on purpose, because asking costs a round trip: only requests that can
/// act on a session somebody else owns, and only when the session they name is
/// actually protected. Nothing else has an authorization decision to make — a
/// session with no secret can be attached by any same-user process for itself,
/// so confirming which process is asking would protect nothing.
/// `Detach` and `Resume` are absent on purpose: they stream raw bytes after
/// their message, so there is nowhere to interject a challenge. Those ask to be
/// identified first, with [`Request::Attest`].
///
/// A local `Attach`, `TakeExclusive` and `Configure` are the exceptions to
/// "only protected sessions". Each is where a window's identity is recorded for
/// later — whose backgrounded session it is, which viewer may take a shared
/// pane back, which recipients its protected sessions are sealed to — and those
/// checks accept only a vouched-for peer; unattested, a window would lose its
/// own panes, or its sessions' disk records. A stream-only client is remote and
/// cannot answer.
pub(in crate::server) fn attestation_needed(
    daemon: &Arc<Daemon>,
    request: &Request,
    stream_only: bool,
) -> bool {
    if !stream_only
        && matches!(
            request,
            Request::Attach(_) | Request::TakeExclusive { .. } | Request::Configure { .. }
        )
    {
        return true;
    }
    // A session being protected is sealed to what the protecting window
    // configured, which only a vouched-for window has; see `sealing`.
    if !stream_only
        && match request {
            Request::CreateShared(request) => request.verifier.is_some(),
            Request::Share(request) => request.verifier.is_some(),
            Request::SetSessionScope(request) => request.verifier.is_some(),
            _ => false,
        }
    {
        return true;
    }
    #[cfg(feature = "session-persistence")]
    if let Request::Spawn(SpawnRequest {
        session_id: Some(session_id),
        ..
    }) = request
        && daemon
            .restored
            .lock()
            .recover_poison()
            .iter()
            .any(|restored| {
                restored.request.record_id == *session_id && restored.request.verifier.is_some()
            })
    {
        return true;
    }
    let session_id = match request {
        // Adding a pane to a live session; a new session has no secret yet.
        Request::Spawn(SpawnRequest {
            session_id: Some(session_id),
            ..
        }) => Some(*session_id),
        Request::ReleaseExclusive { session_id, .. } => Some(*session_id),
        Request::SpawnShared(request) => Some(request.session_id),
        Request::SpawnSharedBatch(request) => Some(request.session_id),
        Request::ApplyShared(request) => Some(request.session_id),
        Request::SharedSnapshot { session_id } | Request::LeaveShared { session_id } => {
            Some(*session_id)
        }
        Request::Share(request) => Some(request.session_id),
        Request::Snapshot(request) => Some(request.session_id),
        Request::StoreImage(request) if !stream_only => Some(request.session_id),
        Request::Resize { session_id, .. }
        | Request::SetConsolePalette { session_id, .. }
        | Request::ClosePane { session_id, .. }
        | Request::DiscardPane { session_id, .. }
        | Request::Kill { session_id }
        | Request::Forget { session_id } => Some(*session_id),
        Request::SetSessionScope(request) => Some(request.session_id),
        // Names panes rather than a session, and whose sessions those are is
        // exactly what it must not disclose, so it is decided against whatever
        // protected sessions exist.
        Request::PaneStates { .. } => None,
        _ => return false,
    };
    let sessions = daemon.sessions();
    match session_id {
        Some(session_id) => sessions
            .iter()
            .any(|session| session.id == session_id && session.authentication.is_some()),
        None => sessions
            .iter()
            .any(|session| session.authentication.is_some()),
    }
}

/// Asks the peer to prove it is the process its envelope named.
///
/// Answered means the identity is as good as a kernel-reported one for this
/// connection; unanswered, or answered wrongly, means the request goes ahead
/// with no identity and every protected-session check refuses it.
pub(in crate::server) fn attest_peer(
    connection: &mut Connection,
    client_process_id: u32,
) -> Result<Option<u32>> {
    /// Bounded, unlike most of the daemon's reads: until the answer arrives or
    /// the attempt is abandoned, the challenge is holding a handle inside
    /// another process. A client that names somebody else and then says nothing
    /// must not be able to leave it there.
    const ANSWER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

    let challenge = crate::transport::PeerChallenge::issue(client_process_id)?;
    connection.send(&Response::AttestationRequired {
        handle: challenge.handle(),
    })?;
    connection.set_read_timeout(Some(ANSWER_TIMEOUT))?;
    let answer = connection
        .receive::<Request>()
        .context("reading a peer attestation answer");
    connection.set_read_timeout(None)?;
    let Request::Attested { nonce } = answer?.0 else {
        anyhow::bail!("expected an attestation answer");
    };
    Ok(challenge.matches(&nonce).then_some(client_process_id))
}
