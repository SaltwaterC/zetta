//! Who may hold, take or end a session, and which process a request
//! really comes from.

use super::*;

/// The identity a control request acts as: what the transport vouched for, or
/// the envelope's own claim where nothing could.
///
/// Only for checks whose failure is inconvenient rather than a privilege
/// boundary — "are you the window holding this pane" for a session with no
/// secret, which any same-user process could attach for itself anyway. A
/// protected session's controls go through [`session_control_authorized`] or
/// [`protected_holder_authorized`], neither of which accepts a claim.
pub(in crate::server) fn control_process_id(
    client_process_id: u32,
    peer_process_id: Option<u32>,
) -> u32 {
    peer_process_id.unwrap_or(client_process_id)
}

/// The envelope's process where the transport confirmed it is the one asking,
/// and `None` where it could not, or found somebody else.
///
/// Unlike [`control_process_id`], never the claim on its own: this is the
/// identity for checks that grant something — answering a handover, attaching
/// a backgrounded session, taking a pane back — which a same-user process
/// naming another must not pass.
pub(in crate::server) fn verified_process_id(
    client_process_id: u32,
    peer_process_id: Option<u32>,
) -> Option<u32> {
    peer_process_id.filter(|peer| *peer == client_process_id)
}

/// Whether a protected session may be acted on by this peer as one of the
/// clients currently showing it.
///
/// Distinct from [`session_control_authorized`] in refusing the owner: resizing
/// a pane or repainting its palette is something only a window displaying it
/// can sensibly ask for.
pub(in crate::server) fn protected_holder_authorized(
    session: &Session,
    peer_process_id: Option<u32>,
) -> bool {
    peer_process_id.is_some_and(|process_id| session_is_held_by(session, process_id))
}

/// Whether a peer that is neither the owner nor a holder may still *offer* a
/// protected session.
///
/// The way out of a deadlock that would otherwise be permanent. Attaching a
/// scoped session refuses and says to share it — see `attach`, which spells out
/// that a window which has exited cannot share it itself — and sharing refused
/// in turn because the owner it named no longer exists. A live session, and
/// whatever is still running inside it, was unreachable for good.
///
/// Safe because offering changes no secret. It makes the session *listed* and
/// attachable; whoever attaches still has to present the secret already set, and
/// a protected session's catalog entry is stripped either way. Replacing the
/// verifier is the half that hands a session over, and that stays with the owner.
///
/// Restricted to an owner that is gone, so it can never overrule a window that
/// is running and has deliberately kept its session to itself.
pub(in crate::server) fn stranded_session_may_be_offered(
    session: &Session,
    shared: bool,
    replaces_verifier: bool,
) -> bool {
    shared && !replaces_verifier && !session.owner.is_some_and(ProcessIdentity::is_running)
}

/// Whether `peer_process_id` may control a protected session.
///
/// The owner or a holder may by identity alone. Anyone else — chiefly a remote
/// stream-only client, which cannot prove a local PID through the SSH socket —
/// needs `proof` of the session secret, established before the caller took the
/// sessions lock by [`prove_session_secret`] with the same Argon2 verifier and
/// refusal window attach uses.
pub(in crate::server) fn session_control_authorized(
    session: &Session,
    peer_process_id: Option<u32>,
    proof: &SecretProof,
) -> bool {
    let Some(authentication) = session.authentication.as_ref() else {
        return true;
    };
    session_identity_authorized(session, peer_process_id) || proof.authorizes(authentication)
}

/// Whether a vouched-for peer owns or holds `session`, which is what lets it
/// control a protected session without presenting the secret.
pub(in crate::server) fn session_identity_authorized(
    session: &Session,
    peer_process_id: Option<u32>,
) -> bool {
    peer_process_id.is_some_and(|process_id| {
        session.owner.is_some_and(|owner| owner.is(process_id))
            || session_is_held_by(session, process_id)
    })
}

/// How many windows are being relayed this pane. Zero for a pane one window holds
/// outright, which is the state unsharing is trying to reach.
pub(in crate::server) fn shared_viewer_count(attachment: &Attachment) -> usize {
    match attachment {
        Attachment::Shared(clients) => clients.len(),
        Attachment::None
        | Attachment::Exclusive(_)
        | Attachment::Revoking { .. }
        | Attachment::Granting { .. } => 0,
    }
}

/// Whether this client is one of the clients showing any of the session's panes.
pub(in crate::server) fn session_is_held_by(session: &Session, client_process_id: u32) -> bool {
    session
        .panes
        .iter()
        .any(|pane| pane_is_held_by(pane, client_process_id))
}

pub(in crate::server) fn pane_is_held_by(pane: &Pane, client_process_id: u32) -> bool {
    match &pane.attachment {
        Attachment::Exclusive(holder)
        | Attachment::Revoking { holder }
        | Attachment::Granting { holder } => *holder == client_process_id,
        Attachment::Shared(clients) => clients
            .iter()
            .any(|client| client.process_id == client_process_id),
        Attachment::None => false,
    }
}
