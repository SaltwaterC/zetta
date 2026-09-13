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
            keep_alive: None,
            scrollback_kib: None,
        }
    );
    assert!(client::parse_args(["host".into()]).is_err());
    assert!(client::parse_args(["host".into(), "0".into()]).is_err());
}

/// The same shape as `-k`: the short form takes no value, so `-s host` still
/// names a target, and `0` is the one value outside the range that means
/// something.
#[test]
fn the_endpoint_client_takes_a_scrollback_size_or_turns_it_off() {
    let scrollback = |arguments: [std::ffi::OsString; 3]| {
        client::parse_args(arguments).map(|args| args.scrollback_kib)
    };
    assert_eq!(
        scrollback(["-s".into(), "host".into(), "60001".into()]).unwrap(),
        Some(client::SCROLLBACK_DEFAULT_KIB)
    );
    assert_eq!(
        scrollback(["--scrollback=512".into(), "host".into(), "60001".into()]).unwrap(),
        Some(512)
    );
    assert_eq!(
        scrollback(["--no-scrollback".into(), "host".into(), "60001".into()]).unwrap(),
        Some(0)
    );
    // `-c` prints a colour count and takes no endpoint, so it cannot be
    // combined with one; the size still parses on its own.
    assert!(scrollback(["host".into(), "60001".into(), "-c".into()]).is_err());
    assert!(scrollback(["--scrollback=1".into(), "host".into(), "60001".into()]).is_err());
    assert!(scrollback(["--scrollback=99999".into(), "host".into(), "60001".into()]).is_err());
}

#[test]
fn the_endpoint_client_takes_a_keep_alive_with_or_without_an_interval() {
    let keep_alive = |arguments: [std::ffi::OsString; 3]| {
        client::parse_args(arguments).map(|args| args.keep_alive)
    };
    assert_eq!(
        keep_alive(["-k".into(), "host".into(), "60001".into()]).unwrap(),
        Some(mosh_rs::sender::KEEP_ALIVE_DEFAULT_MS)
    );
    assert_eq!(
        keep_alive(["--keep-alive".into(), "host".into(), "60001".into()]).unwrap(),
        Some(mosh_rs::sender::KEEP_ALIVE_DEFAULT_MS)
    );
    assert_eq!(
        keep_alive(["--keep-alive=250".into(), "host".into(), "60001".into()]).unwrap(),
        Some(250)
    );
    assert_eq!(
        keep_alive(["-k=250".into(), "host".into(), "60001".into()]).unwrap(),
        Some(250)
    );
    // Out of range, unparseable, and repeated are all refused rather
    // than silently taking one of the two values.
    assert!(keep_alive(["--keep-alive=5".into(), "host".into(), "60001".into()]).is_err());
    assert!(keep_alive(["--keep-alive=99999".into(), "host".into(), "60001".into()]).is_err());
    assert!(keep_alive(["--keep-alive=soon".into(), "host".into(), "60001".into()]).is_err());
    assert!(
        client::parse_args([
            "-k".into(),
            "--keep-alive=250".into(),
            "host".into(),
            "60001".into()
        ])
        .is_err()
    );
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
