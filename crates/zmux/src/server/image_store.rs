//! Private staging for images pasted into shared panes.
//!
//! Image bytes never travel on the pane's long-lived relay. A client first
//! proves that it is an active viewer, then sends a bounded PNG payload over a
//! fresh request connection. The returned path is valid in the environment
//! where the session's child process runs.

use std::{collections::HashSet, fs};

use anyhow::{Context as _, Result};

use crate::{
    catalog::{create_private_dir, write_private_file},
    messages::{ClientId, MAX_IMAGE_BYTES, Response},
};

use super::*;

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

/// Stores an image for an active shared viewer.
pub(super) fn store_image(
    daemon: &Arc<Daemon>,
    session_id: u64,
    pane_id: u64,
    length: usize,
    client_process_id: u32,
    peer_process_id: Option<u32>,
    client_id: ClientId,
    stream_only: bool,
    session_secret: Option<&str>,
    connection: &mut Connection,
) -> Result<()> {
    anyhow::ensure!(
        (1..=MAX_IMAGE_BYTES).contains(&length),
        "image payload must be between 1 byte and {MAX_IMAGE_BYTES} bytes"
    );

    {
        let mut sessions = daemon
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
            return connection.send(&Response::Error {
                message: format!("session {session_id} does not exist"),
            });
        };
        let Some(pane) = session.panes.iter().find(|pane| pane.id == pane_id) else {
            return connection.send(&Response::Error {
                message: format!("session {session_id} has no pane {pane_id}"),
            });
        };
        let active_viewer = match &pane.attachment {
            Attachment::Shared(clients) => clients
                .iter()
                .find(|client| {
                    client.client_id == client_id
                        && (stream_only
                            || client.process_id
                                == control_process_id(client_process_id, peer_process_id))
                })
                .is_some(),
            Attachment::None
            | Attachment::Exclusive(_)
            | Attachment::Revoking { .. }
            | Attachment::Granting { .. } => false,
        };
        anyhow::ensure!(
            active_viewer,
            "client is not an active shared viewer of pane {pane_id}"
        );
        anyhow::ensure!(
            session_control_authorized(session, peer_process_id, session_secret),
            "session {session_id} is protected and the image-paste client is not authorized"
        );
    }

    let bytes = connection.read_exact(length)?;
    anyhow::ensure!(
        bytes.starts_with(PNG_SIGNATURE),
        "image payload is not a PNG"
    );

    let session_directory = daemon.image_directory.join(session_id.to_string());
    create_private_dir(&session_directory)?;
    let name = format!("{}.png", crate::transport::random_hex(16)?);
    let final_path = session_directory.join(name);
    let temporary_path =
        session_directory.join(format!(".{}.tmp", crate::transport::random_hex(16)?));
    write_private_file(&temporary_path, &bytes)
        .with_context(|| format!("writing image staging file {}", temporary_path.display()))?;
    if let Err(error) = fs::rename(&temporary_path, &final_path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(error)
            .with_context(|| format!("committing image staging file {}", final_path.display()));
    }
    let path = final_path
        .to_str()
        .context("image staging path is not valid UTF-8")?
        .to_owned();
    connection.send(&Response::ImageStored { path })
}

/// Creates the staging root and removes directories that do not belong to a
/// session adopted by this daemon. This runs after upgrade handover adoption,
/// so live sessions from the old daemon remain intact.
pub(super) fn sweep_image_staging(daemon: &Arc<Daemon>) -> Result<()> {
    create_private_dir(&daemon.image_directory)?;
    let live_sessions = daemon
        .sessions
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .map(|session| session.id)
        .collect::<HashSet<_>>();
    for entry in fs::read_dir(&daemon.image_directory)? {
        let entry = entry?;
        let path = entry.path();
        let Some(session_id) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse().ok())
        else {
            continue;
        };
        if path.is_dir() && !live_sessions.contains(&session_id) {
            fs::remove_dir_all(&path)
                .with_context(|| format!("removing stale image directory {}", path.display()))?;
        }
    }
    Ok(())
}

/// Removes all staged images for a session. Cleanup is deliberately best
/// effort at lifecycle call sites; a later daemon startup sweep repairs any
/// directory left behind by a failed removal.
pub(super) fn remove_image_session(daemon: &Daemon, session_id: u64) {
    let path = daemon.image_directory.join(session_id.to_string());
    match fs::remove_dir_all(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            log::warn!("could not remove staged images for session {session_id}: {error}")
        }
    }
}

#[cfg(test)]
#[path = "../tests/server/image_store.rs"]
mod tests;
