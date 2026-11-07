//! Compatibility markers shared by the standalone CLI and Zetta's version output.

/// Named versions of the contracts compiled into this multiplexer.
pub fn compatibility_version_text() -> String {
    let versions = [
        ("CONTROL_VERSION", crate::protocol::CONTROL_VERSION),
        ("CATALOG_VERSION", crate::protocol::CATALOG_VERSION),
        ("ZMUX_PROTOCOL_VERSION", crate::messages::PROTOCOL_VERSION),
        ("ZMUX_ENDPOINT_VERSION", crate::transport::ENDPOINT_VERSION),
        (
            "ZMUX_SHARED_SESSION_STATE_VERSION",
            crate::messages::SHARED_SESSION_STATE_VERSION,
        ),
        ("ZMUX_HANDOVER_VERSION", crate::upgrade::HANDOVER_VERSION),
        #[cfg(feature = "session-persistence")]
        (
            "ZMUX_MANIFEST_VERSION",
            crate::persistence::MANIFEST_VERSION,
        ),
        #[cfg(feature = "session-persistence")]
        (
            "ZMUX_PERSISTENCE_BINDING_VERSION",
            crate::persistence::BINDING_VERSION,
        ),
        #[cfg(windows)]
        (
            "ZMUX_HOST_PROTOCOL_VERSION",
            crate::pty_host::HOST_PROTOCOL_VERSION,
        ),
        #[cfg(windows)]
        (
            "ZMUX_MINIMUM_HOST_PROTOCOL_VERSION",
            crate::pty_host::MINIMUM_HOST_PROTOCOL_VERSION,
        ),
        #[cfg(windows)]
        (
            "ZMUX_CONSOLE_SERVER_HOST_PROTOCOL_VERSION",
            crate::pty_host::CONSOLE_SERVER_HOST_PROTOCOL_VERSION,
        ),
        #[cfg(windows)]
        (
            "ZMUX_HOST_INSTALL_REVISION",
            crate::pty_host::HOST_INSTALL_REVISION,
        ),
    ];
    let text = versions
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("\n");
    #[cfg(feature = "session-persistence")]
    let text = format!(
        "{text}\nZMUX_SEALED_KEY_VERSION={}",
        crate::auto_protect::SEALED_KEY_VERSION
    );
    text
}

#[cfg(test)]
#[path = "tests/versions.rs"]
mod tests;
