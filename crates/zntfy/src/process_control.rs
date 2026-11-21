//! Minimal client for the two Zetta-only notification integrations.
//!
//! Notification delivery is useful in every terminal. When Zetta's inherited
//! target variables are present, this client additionally asks the owning
//! process whether the tab is silent and focuses that tab after a body click.
//! The endpoint reading and request framing are `zmux::process_control`'s.

use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use zmux::process_control::{self, ControlResponse};

const CONTROL_CLIENT_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Serialize)]
struct AttentionFields {
    #[serde(skip_serializing_if = "Option::is_none")]
    attention_id: Option<u64>,
}

#[derive(Deserialize)]
struct SilentModeResponse {
    status: String,
    #[serde(default)]
    silent_mode: bool,
}

pub(super) fn request_process_silent_mode(
    process_id: u32,
    attention_id: Option<u64>,
) -> Result<bool> {
    anyhow::ensure!(attention_id != Some(0), "attention ID must be positive");
    let endpoint = process_control::endpoint_for_process(process_id)?;
    let response: SilentModeResponse = process_control::request(
        &endpoint,
        "get_silent_mode",
        &AttentionFields { attention_id },
        CONTROL_CLIENT_TIMEOUT,
    )?;
    anyhow::ensure!(
        response.status == "ok",
        "Zetta rejected the silent-mode query"
    );
    Ok(response.silent_mode)
}

pub(super) fn request_process_focus_tab(process_id: u32, attention_id: u64) -> Result<bool> {
    anyhow::ensure!(attention_id != 0, "attention ID must be positive");
    let endpoint = process_control::endpoint_for_process(process_id)?;
    let response: ControlResponse = process_control::request(
        &endpoint,
        "focus_tab",
        &AttentionFields {
            attention_id: Some(attention_id),
        },
        CONTROL_CLIENT_TIMEOUT,
    )?;
    Ok(response.status == "ok")
}
