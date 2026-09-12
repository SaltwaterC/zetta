use super::*;

use crate::config::{RemoteSessionConfig, RemoteSessionProtocol};

/// The protocol is written the same way in configuration, on the command line
/// and in the picker, and an unknown one says what the two are.
#[test]
fn a_protocol_name_is_read_the_same_way_everywhere() {
    assert_eq!(
        RemotePaneTransport::parse("ssh", Some(500)).unwrap(),
        RemotePaneTransport::Ssh,
        "an SSH session has no link to hold open, so the interval is not its business"
    );
    assert_eq!(
        RemotePaneTransport::parse(" ZOSH ", Some(250)).unwrap(),
        RemotePaneTransport::Zosh {
            keep_alive_ms: Some(250)
        }
    );
    assert_eq!(
        RemotePaneTransport::parse("zosh", None).unwrap(),
        RemotePaneTransport::Zosh {
            keep_alive_ms: None
        }
    );

    let error = RemotePaneTransport::parse("mosh", None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("ssh"), "{error}");
    assert!(error.contains("zosh"), "{error}");
}

/// What the picker starts on comes from configuration, and the keep-alive
/// interval travels with the protocol it belongs to.
#[test]
fn the_configured_protocol_is_what_a_session_starts_on() {
    assert_eq!(
        RemotePaneTransport::from_config(&RemoteSessionConfig::default()),
        RemotePaneTransport::Ssh
    );
    assert_eq!(
        RemotePaneTransport::from_config(&RemoteSessionConfig {
            protocol: RemoteSessionProtocol::Zosh,
            keep_alive_ms: Some(250),
        }),
        RemotePaneTransport::Zosh {
            keep_alive_ms: Some(250)
        }
    );
    assert_eq!(
        RemotePaneTransport::from_config(&RemoteSessionConfig {
            protocol: RemoteSessionProtocol::Ssh,
            // Configured for when Zosh is chosen later; it does not make an
            // SSH session hold anything open.
            keep_alive_ms: Some(250),
        })
        .keep_alive_ms(),
        None
    );
}
