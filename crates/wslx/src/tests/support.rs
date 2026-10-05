//! Helpers shared by the sidecar tests.

#[cfg(unix)]
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// A fresh directory under the system temporary directory, removed on drop.
/// Only the Unix tests need a directory: a socket, or a home for the script.
#[cfg(unix)]
pub struct ScratchDir(PathBuf);

#[cfg(unix)]
impl ScratchDir {
    pub fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let path =
            std::env::temp_dir().join(format!("wslx-test-{name}-{}-{nonce:x}", std::process::id()));
        fs::create_dir_all(&path).expect("creating a scratch directory");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

#[cfg(unix)]
impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// One agent message with `body` as its contents.
pub fn agent_frame(body: &[u8]) -> Vec<u8> {
    let mut frame = u32::try_from(body.len()).unwrap().to_be_bytes().to_vec();
    frame.extend_from_slice(body);
    frame
}
