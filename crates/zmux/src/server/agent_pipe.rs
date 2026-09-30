//! Each daemon-owned pane's stable SSH-agent name on a Windows host.
//!
//! On Unix a pane inherits a symlink the daemon owns and a Zosh relay repoints
//! (see `lifecycle::prepare_pane_agent_links`). A named pipe cannot be
//! symlinked, so on Windows the daemon serves the stable name itself: every
//! pane is started with its own private pipe as `SSH_AUTH_SOCK`, and each
//! connection to it is relayed, a request and its reply at a time, to the first
//! agent that answers of
//!
//! 1. the pipe the pane's Zosh relay published (`relay/agent_target.rs`),
//! 2. the agent the pane would have had without Zosh, and
//! 3. Windows OpenSSH's default agent pipe.
//!
//! Without this a remote pane on a Windows host ignored the agent forwarded to
//! it and asked the host's own agent — which, with 1Password installed, is a
//! Windows Hello prompt on a machine nobody is sitting at.
//!
//! The candidate order and the relay loop are portable so they are tested on
//! every host; only the pipe listener is Windows code.

use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

#[cfg(windows)]
mod listener;
#[cfg(windows)]
pub(super) use listener::{serve, stop};

/// The agent Windows OpenSSH asks when `SSH_AUTH_SOCK` is unset.
const WINDOWS_OPENSSH_AGENT: &str = r"\\.\pipe\openssh-ssh-agent";

/// The largest agent message relayed, matching Zosh's own limit.
const MAX_FRAME: usize = 256 * 1024;

/// The agents one connection is relayed to, most specific first.
///
/// `published` is the target file's contents, read fresh for every connection
/// so a relay that attaches or leaves takes effect on the next request.
/// Anything that is not a local pipe, and any daemon pane pipe — which would
/// relay a pane's agent back into itself — is skipped.
fn upstream_candidates(published: Option<&str>, fallback: Option<&Path>) -> Vec<PathBuf> {
    let published = published.map(str::trim).map(PathBuf::from);
    let mut candidates: Vec<PathBuf> = Vec::new();
    for candidate in published
        .into_iter()
        .chain(fallback.map(Path::to_path_buf))
        .chain(std::iter::once(PathBuf::from(WINDOWS_OPENSSH_AGENT)))
    {
        let text = candidate.to_string_lossy();
        let usable = text.starts_with(r"\\.\pipe\")
            && !text.starts_with(crate::paths::PANE_AGENT_PIPE_PREFIX)
            && !candidates.contains(&candidate);
        if usable {
            candidates.push(candidate);
        }
    }
    candidates
}

/// Relays agent requests from `client` to `agent` until the client hangs up.
///
/// The agent protocol is strictly one reply per request, so relaying whole
/// frames in turn needs no second thread — which matters on Windows, where a
/// synchronous pipe handle serializes a blocked read against any write.
fn relay_frames(
    client: &mut (impl Read + Write),
    agent: &mut (impl Read + Write),
) -> io::Result<()> {
    loop {
        let request = match read_frame(client) {
            Ok(frame) => frame,
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error),
        };
        agent.write_all(&request)?;
        agent.flush()?;
        let reply = read_frame(agent)?;
        client.write_all(&reply)?;
        client.flush()?;
    }
}

fn read_frame(stream: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length)?;
    let body_len = u32::from_be_bytes(length) as usize;
    if body_len == 0 || body_len > MAX_FRAME - 4 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "SSH-agent frame exceeds the frame limit",
        ));
    }
    let mut frame = vec![0_u8; body_len + 4];
    frame[..4].copy_from_slice(&length);
    stream.read_exact(&mut frame[4..])?;
    Ok(frame)
}

#[cfg(test)]
#[path = "../tests/server/agent_pipe.rs"]
mod tests;
