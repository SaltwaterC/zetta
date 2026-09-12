//! Image-paste support for remote shared sessions.
//!
//! The terminal crate owns ordering and the native local shortcut. This module
//! owns the application-specific conversion from GPUI clipboard images to the
//! PNG payload understood by `zmux`, and the choice of which handler a pane's
//! terminal is built with.

use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use gpui::Image;
use task::Shell;
use terminal::{ImagePasteHandler, ImagePasteResult};

use crate::{image_paste::normalize_image, mux::MuxRuntime, ssh_image_paste::SshImagePasteHandler};

/// What a local pane's image upload would need, for the panes that may turn out
/// not to be remote. Kept together because the three values travel as one and
/// are read only when the runtime is local.
///
/// Generic over the environment because a pane's carries
/// `collections::FxBuildHasher`, which this crate cannot name.
pub(crate) struct LocalPasteTarget<I> {
    pub(crate) shell: Shell,
    pub(crate) environment: I,
    pub(crate) working_directory: Option<PathBuf>,
}

/// The image-paste handler a pane's terminal is built with, chosen by where the
/// pane's process actually runs.
///
/// This is one function because getting it wrong is invisible: a pane in a
/// remote session that is handed the local handler asks it about a foreground
/// process, a byte-stream pane has none to report, and the paste silently
/// degrades to the native chord — which reaches a program on another host as an
/// empty clipboard. Every pane that can belong to a session goes through here
/// so the remote case cannot be reached by omission.
pub(crate) fn handler_for_pane<I>(
    runtime: &MuxRuntime,
    session_id: u64,
    pane_id: u64,
    local: LocalPasteTarget<I>,
) -> Arc<dyn ImagePasteHandler>
where
    I: IntoIterator<Item = (String, String)>,
{
    if runtime.is_remote() {
        return Arc::new(RemoteImagePasteHandler::new(runtime, session_id, pane_id));
    }
    Arc::new(SshImagePasteHandler::new(
        local.shell,
        local.environment,
        local.working_directory,
    ))
}

pub(crate) struct RemoteImagePasteHandler {
    runtime: MuxRuntime,
    session_id: u64,
    pane_id: u64,
}

impl RemoteImagePasteHandler {
    pub(crate) fn new(runtime: &MuxRuntime, session_id: u64, pane_id: u64) -> Self {
        Self {
            runtime: runtime.clone(),
            session_id,
            pane_id,
        }
    }
}

impl ImagePasteHandler for RemoteImagePasteHandler {
    fn paste_image(&self, image: &Image, _: Option<&[String]>) -> Result<ImagePasteResult> {
        let bytes = normalize_image(image)?;
        self.runtime
            .client()
            .store_image(self.session_id, self.pane_id, bytes)
            .map(ImagePasteResult::ResolvedPath)
    }
}
