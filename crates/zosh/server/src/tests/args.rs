use super::*;

#[test]
fn port_range() {
    assert_eq!(parse_port_range("60000").unwrap(), (60000, 60000));
    assert_eq!(parse_port_range("60000:61000").unwrap(), (60000, 61000));
    assert_eq!(parse_port_range("0").unwrap(), (0, 0));
    assert!(parse_port_range("61000:60000").is_err());
}

#[test]
fn ssh_connection_server_ip() {
    assert_eq!(
        parse_ssh_server_ip("192.0.2.10 50123 198.51.100.20 22").unwrap(),
        "198.51.100.20".parse::<IpAddr>().unwrap()
    );
    assert_eq!(
        parse_ssh_server_ip("2001:db8::1 50123 2001:db8::2 22").unwrap(),
        "2001:db8::2".parse::<IpAddr>().unwrap()
    );
}

#[test]
fn command_after_separator() {
    let args = vec![
        OsString::from("new"),
        OsString::from("-p"),
        OsString::from("60001"),
        OsString::from("--"),
        OsString::from("sh"),
        OsString::from("-lc"),
        OsString::from("echo hi"),
    ];
    let ParseOutcome::Run(cfg) = parse(args).unwrap() else {
        panic!("expected config")
    };
    assert_eq!(cfg.port_low, 60001);
    assert_eq!(cfg.command.len(), 3);
}

#[test]
fn agent_forwarding_is_opt_in_and_duplicate_flags_are_rejected() {
    let ParseOutcome::Run(cfg) = parse(vec![
        OsString::from("new"),
        OsString::from("--forward-agent"),
    ])
    .unwrap() else {
        panic!("expected config")
    };
    assert!(cfg.forward_agent);
    assert!(!Config::default().forward_agent);

    let ParseOutcome::Run(cfg) = parse(vec![
        OsString::from("new"),
        OsString::from("--no-forward-agent"),
    ])
    .unwrap() else {
        panic!("expected config")
    };
    assert!(!cfg.forward_agent);
    assert!(
        parse(vec![
            OsString::from("new"),
            OsString::from("--forward-agent"),
            OsString::from("--no-forward-agent"),
        ])
        .is_err()
    );
}
