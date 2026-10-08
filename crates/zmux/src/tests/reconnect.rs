use std::fs;

use super::*;
use crate::protocol;

fn catalog(process_id: u32, runner_id: u64, session_ids: &[u64]) -> BackgroundSessionCatalog {
    BackgroundSessionCatalog {
        version: protocol::CATALOG_VERSION,
        process_id,
        runner_id,
        sessions: session_ids
            .iter()
            .map(|id| protocol::BackgroundSessionSummary {
                id: *id,
                title: format!("Session {id}"),
                authentication_required: *id == 2,
                active_pane: 1,
                layout: protocol::BackgroundPaneLayout::Pane { pane_id: 1 },
                panes: Vec::new(),
                held: false,
                scoped_to: None,
                key_envelope: None,
            })
            .collect(),
    }
}

#[test]
fn finds_full_and_unique_bare_session_ids() {
    let catalogs = [catalog(123, 7, &[1, 2]), catalog(456, 8, &[3])];

    let full = find_session(&catalogs, "123:7:2").unwrap();
    assert_eq!(
        (full.process_id, full.runner_id, full.session_id),
        (123, 7, 2)
    );
    assert!(full.authentication_required);

    let bare = find_session(&catalogs, "3").unwrap();
    assert_eq!(
        (bare.process_id, bare.runner_id, bare.session_id),
        (456, 8, 3)
    );
}

#[test]
fn rejects_ambiguous_bare_session_ids() {
    let catalogs = [catalog(123, 7, &[1]), catalog(456, 8, &[1])];
    let error = find_session(&catalogs, "1").unwrap_err().to_string();
    assert!(error.contains("ambiguous"), "{error}");
}

#[test]
fn rejects_zero_components_in_full_session_ids() {
    let error = find_session(&[catalog(123, 7, &[1])], "0:7:1")
        .unwrap_err()
        .to_string();
    assert!(error.contains("positive whole numbers"), "{error}");
}

#[test]
fn reconnect_origin_requires_positive_process_and_attention_ids() {
    assert_eq!(
        parse_reconnect_origin("123", "456"),
        Some(ReconnectOrigin {
            process_id: 123,
            attention_id: 456,
        })
    );
    for (process_id, attention_id) in [("0", "456"), ("123", "0"), ("not-a-pid", "456")] {
        assert_eq!(parse_reconnect_origin(process_id, attention_id), None);
    }
}

/// A remote host listing, as its own session, an envelope sealed on another
/// host: the user's identity opens it, but the key inside is never handed back
/// for sending.
#[cfg(feature = "session-persistence")]
#[test]
fn a_remote_session_offering_another_hosts_sealed_key_is_sent_nothing() {
    use age::secrecy::ExposeSecret as _;

    let directory = tempfile::tempdir().unwrap();
    let identity = age::x25519::Identity::generate();
    let identity_path = directory.path().join("identity.txt");
    fs::write(
        &identity_path,
        format!("{}\n", identity.to_string().expose_secret()),
    )
    .unwrap();
    let recipients =
        crate::persistence::RecipientSet::parse(&[identity.to_public().to_string()]).unwrap();
    // Sealed on the victim host, published there, and copied by the host the
    // user is now attaching to.
    let elsewhere = crate::auto_protect::seal(&recipients).unwrap();
    let mut summary = catalog(1, 1, &[2]).sessions.remove(0);
    summary.key_envelope = elsewhere.authentication.key_envelope().map(str::to_owned);

    let result = remote_session_secret_with(
        &summary,
        std::slice::from_ref(&identity_path),
        |envelope, identities| {
            crate::auto_protect::open_for_remote_with(
                envelope,
                identities,
                "malicious-host",
                || Ok(vec!["SHA256:the-malicious-hosts-own-key".to_owned()]),
            )
        },
    );

    let error = result.expect_err("the key must not be released");
    assert!(error.to_string().contains("was not sent"), "{error:#}");
}

#[cfg(unix)]
mod control_endpoints {
    use std::{os::unix::fs::PermissionsExt as _, path::Path};

    use super::*;

    fn private_session_directory() -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("sessions");
        crate::private_fs::create_private_dir(&directory).unwrap();
        (root, directory)
    }

    /// Publishes an endpoint for this test process, which is running, naming
    /// `socket_path`.
    fn publish(path: &Path, socket_path: &Path) {
        let endpoint = serde_json::json!({
            "version": CONTROL_VERSION,
            "process_id": std::process::id(),
            "socket_path": socket_path,
            "token": "token",
        });
        crate::private_fs::write_private_file(path, endpoint.to_string().as_bytes()).unwrap();
    }

    fn own_endpoint_path(directory: &Path) -> PathBuf {
        directory.join(format!("control-{}.json", std::process::id()))
    }

    #[test]
    fn a_running_windows_own_endpoint_is_found() {
        let (_root, directory) = private_session_directory();
        let path = own_endpoint_path(&directory);
        publish(&path, &path.with_extension("sock"));

        let endpoints = running_control_endpoints(&directory).unwrap();
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].socket_path, path.with_extension("sock"));
    }

    #[test]
    fn an_endpoint_naming_a_socket_other_than_its_own_is_not_used() {
        let (root, directory) = private_session_directory();
        publish(
            &own_endpoint_path(&directory),
            &root.path().join("elsewhere.sock"),
        );

        assert!(running_control_endpoints(&directory).unwrap().is_empty());
        let Err(error) = read_control_endpoint(&own_endpoint_path(&directory), std::process::id())
        else {
            panic!("an endpoint naming another socket must be refused");
        };
        assert!(format!("{error:#}").contains("not its own"), "{error:#}");
    }

    #[test]
    fn a_symlinked_endpoint_file_is_not_trusted() {
        let (root, directory) = private_session_directory();
        let planted = root.path().join("planted.json");
        let path = own_endpoint_path(&directory);
        publish(&planted, &path.with_extension("sock"));
        std::os::unix::fs::symlink(&planted, &path).unwrap();

        assert!(running_control_endpoints(&directory).unwrap().is_empty());
    }

    #[test]
    fn an_endpoint_published_under_another_processs_name_is_not_used() {
        let (_root, directory) = private_session_directory();
        let path = directory.join(format!("control-{}.json", std::process::id() + 1));
        publish(&path, &path.with_extension("sock"));

        assert!(running_control_endpoints(&directory).unwrap().is_empty());
    }

    #[test]
    fn endpoints_are_not_read_from_a_directory_others_can_write() {
        let (_root, directory) = private_session_directory();
        let path = own_endpoint_path(&directory);
        publish(&path, &path.with_extension("sock"));
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o777)).unwrap();

        assert!(running_control_endpoints(&directory).is_err());
    }
}
