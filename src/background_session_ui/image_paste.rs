//! Image-paste support for remote shared sessions.
//!
//! The terminal crate owns ordering and the native local shortcut. This module
//! owns the application-specific conversion from GPUI clipboard images to the
//! PNG payload understood by `zmux`.

use anyhow::Result;
use gpui::Image;
use terminal::{ImagePasteHandler, ImagePasteResult};

use crate::{image_paste::normalize_image, mux::MuxRuntime};

pub(super) struct RemoteImagePasteHandler {
    runtime: MuxRuntime,
    session_id: u64,
    pane_id: u64,
}

impl RemoteImagePasteHandler {
    pub(super) fn new(runtime: &MuxRuntime, session_id: u64, pane_id: u64) -> Self {
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
