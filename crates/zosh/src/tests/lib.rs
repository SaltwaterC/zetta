use super::*;

#[test]
fn endpoint_arguments_require_host_and_nonzero_port() {
    assert_eq!(
        client::parse_args(["host".into(), "60001".into()]).unwrap(),
        ClientArgs {
            host: "host".to_owned(),
            port: 60001,
            help: false,
            version: false,
            colors: false,
        }
    );
    assert!(client::parse_args(["host".into()]).is_err());
    assert!(client::parse_args(["host".into(), "0".into()]).is_err());
}

#[test]
fn endpoint_help_and_version_do_not_require_mosh_key() {
    assert!(client::parse_args(["--help".into()]).unwrap().help);
    assert!(client::parse_args(["--version".into()]).unwrap().version);
    assert!(client::parse_args(["-c".into()]).unwrap().colors);
}

#[test]
fn legacy_endpoint_shape_is_only_two_nonzero_positional_arguments() {
    assert!(endpoint_shape(&["192.0.2.1".into(), "60001".into()]));
    assert!(!endpoint_shape(&["host".into(), "0".into()]));
    assert!(!endpoint_shape(&[
        "host".into(),
        "60001".into(),
        "cmd".into()
    ]));
    assert!(!endpoint_shape(&["--help".into(), "60001".into()]));
}
