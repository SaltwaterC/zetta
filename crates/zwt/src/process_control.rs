//! Tells the Zetta window `zwt` was run from which worktree its tab is in.
//!
//! The endpoint reading and request framing are `zmux::process_control`'s;
//! this module only names the command and what it carries.

use std::time::Duration;

use anyhow::Result;
use serde::Serialize;
use zmux::process_control::{self, ControlEndpoint, ControlResponse};

const CONTROL_CLIENT_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeNameRequest {
    pub attention_id: u64,
    pub name: Option<String>,
}

#[derive(Serialize)]
struct WorktreeNameFields<'a> {
    attention_id: u64,
    worktree_name: Option<&'a str>,
}

/// Ask the Zetta process identified by `process_id` to update its originating
/// tab's worktree title. This is deliberately a small, best-effort client:
/// `zwt` remains useful when Zetta is not running or its endpoint has gone
/// away.
pub fn request_process_worktree_name(
    process_id: u32,
    request: WorktreeNameRequest,
) -> Result<bool> {
    anyhow::ensure!(request.attention_id != 0, "attention ID must be positive");
    let endpoint = process_control::endpoint_for_process(process_id)?;
    send_worktree_name(&endpoint, &request)
}

fn send_worktree_name(endpoint: &ControlEndpoint, request: &WorktreeNameRequest) -> Result<bool> {
    let response: ControlResponse = process_control::request(
        endpoint,
        "set_worktree_name",
        &WorktreeNameFields {
            attention_id: request.attention_id,
            worktree_name: request.name.as_deref(),
        },
        CONTROL_CLIENT_TIMEOUT,
    )?;
    Ok(response.status == "ok")
}

#[cfg(test)]
#[path = "tests/process_control.rs"]
mod tests;
