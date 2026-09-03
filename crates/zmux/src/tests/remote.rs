use super::*;

#[test]
fn remote_targets_keep_open_ssh_destination_syntax_intact() {
    let target = RemoteTarget::new("alias.example").with_port(Some(2222));

    assert_eq!(target.destination(), "alias.example");
    assert_eq!(target.port(), Some(2222));
    assert!(target.validate().is_ok());
}

#[test]
fn remote_targets_reject_option_injection_and_empty_destinations() {
    assert!(RemoteTarget::new("").validate().is_err());
    assert!(RemoteTarget::new("-oProxyCommand=bad").validate().is_err());
    assert!(
        RemoteTarget::new("host")
            .with_port(Some(0))
            .validate()
            .is_err()
    );
}

#[test]
fn endpoint_queries_preserve_the_user_ssh_configuration() {
    let target = RemoteTarget::new("dev@example.test").with_port(Some(2222));

    assert_eq!(
        endpoint_arguments(&target),
        [
            "-T",
            "-p",
            "2222",
            "dev@example.test",
            "zmux",
            "endpoint",
            "--json",
        ]
    );
}

#[test]
fn remote_target_does_not_override_open_ssh_identity_selection() {
    let target = RemoteTarget::new("alias");

    assert!(
        !endpoint_arguments(&target)
            .iter()
            .any(|argument| argument == "-i")
    );
    assert!(
        !forward_arguments(&target, "/tmp/local.sock:/run/zmux.sock")
            .iter()
            .any(|argument| argument == "-i")
    );
}

#[test]
fn forwards_are_stream_local_and_do_not_request_a_shell() {
    let target = RemoteTarget::new("alias").with_port(Some(2200));

    assert_eq!(
        forward_arguments(&target, "/tmp/local.sock:/run/user/1000/zmux.sock"),
        [
            "-T",
            "-N",
            "-o",
            "ExitOnForwardFailure=yes",
            "-p",
            "2200",
            "-L",
            "/tmp/local.sock:/run/user/1000/zmux.sock",
            "alias",
        ]
    );
}

#[test]
fn client_ids_are_random_and_serializable() {
    let first = crate::messages::ClientId::random().unwrap();
    let second = crate::messages::ClientId::random().unwrap();
    assert_ne!(first, second);
    assert_eq!(first.as_str().len(), 32);
    let wire = serde_json::to_string(&first).unwrap();
    assert_eq!(
        serde_json::from_str::<crate::messages::ClientId>(&wire).unwrap(),
        first
    );
}
