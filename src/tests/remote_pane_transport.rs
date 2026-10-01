use super::*;

use crate::config::{RemoteSessionConfig, RemoteSessionProtocol};

/// The protocol is written the same way in configuration, on the command line
/// and in the picker, and an unknown one says what the two are.
#[test]
fn a_protocol_name_is_read_the_same_way_everywhere() {
    assert_eq!(
        RemotePaneTransport::parse("ssh", Some(500), false).unwrap(),
        RemotePaneTransport::Ssh,
        "an SSH session has no link to hold open, so the interval is not its business"
    );
    assert_eq!(
        RemotePaneTransport::parse(" ZOSH ", Some(250), false).unwrap(),
        RemotePaneTransport::Zosh {
            keep_alive_ms: Some(250),
            forward_agent: false,
        }
    );
    assert_eq!(
        RemotePaneTransport::parse("zosh", None, false).unwrap(),
        RemotePaneTransport::Zosh {
            keep_alive_ms: None,
            forward_agent: false,
        }
    );

    let error = RemotePaneTransport::parse("mosh", None, false)
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
            forward_agent: false,
        }),
        RemotePaneTransport::Zosh {
            keep_alive_ms: Some(250),
            forward_agent: false,
        }
    );
    assert_eq!(
        RemotePaneTransport::from_config(&RemoteSessionConfig {
            protocol: RemoteSessionProtocol::Ssh,
            // Configured for when Zosh is chosen later; it does not make an
            // SSH session hold anything open.
            keep_alive_ms: Some(250),
            forward_agent: false,
        })
        .keep_alive_ms(),
        None
    );
}

/// A fallback is marked on the pane it happened to, so the reason has to stay
/// with that pane: a neighbour that came up on Mosh, or a pane the bootstrap
/// never tried, must not be marked with it.
#[test]
fn a_fallback_reason_belongs_to_the_pane_it_names() {
    let streams = RemotePaneStreams {
        streams: HashMap::new(),
        fallbacks: vec![
            (4, "pane 4 never answered".to_owned()),
            (7, "pane 7 could not start".to_owned()),
        ],
    };
    assert_eq!(streams.fallback(4), Some("pane 4 never answered"));
    assert_eq!(streams.fallback(7), Some("pane 7 could not start"));
    assert_eq!(streams.fallback(3), None);
    assert_eq!(
        streams.fallbacks().collect::<Vec<_>>(),
        ["pane 4 never answered", "pane 7 could not start"],
        "the notice still reports every pane, in bootstrap order"
    );
    assert_eq!(RemotePaneStreams::default().fallback(4), None);
}
