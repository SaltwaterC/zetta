use super::*;

#[test]
fn parses_and_sorts_string_and_object_commands() {
    let commands = parse_project_commands(&serde_json::json!({
        "test:unit": "cargo test",
        "build": {
            "command": "echo $FOO && cargo build",
            "env": {"FOO": "bar"}
        }
    }))
    .unwrap();

    assert_eq!(
        commands.keys().cloned().collect::<Vec<_>>(),
        vec!["build", "test:unit"]
    );
    assert_eq!(commands["build"].command, "echo $FOO && cargo build");
    assert_eq!(commands["build"].environment["FOO"], "bar");
    assert!(commands["test:unit"].environment.is_empty());
}

#[test]
fn rejects_invalid_names_and_command_objects() {
    for name in ["", "-build", "has space", "--list"] {
        assert!(validate_command_name(name).is_err(), "{name:?} should fail");
    }
    for name in ["1FOO", "has-dash", "has.dot"] {
        assert!(
            parse_project_commands(&serde_json::json!({
                "build": {"command": "echo ok", "env": {name: "x"}}
            }))
            .is_err(),
            "{name:?} should not be interpolated as a shell variable"
        );
    }
    assert!(parse_project_commands(&serde_json::json!({"build": {}})).is_err());
    assert!(
        parse_project_commands(&serde_json::json!({
            "build": {"command": "echo ok", "unknown": true}
        }))
        .is_err()
    );
    assert!(
        parse_project_commands(&serde_json::json!({
            "build": {"command": "echo ok", "env": {"ZETTA_PROCESS_ID": "x"}}
        }))
        .is_err()
    );
    assert!(
        parse_project_commands(&serde_json::json!({
            "build": {"command": "echo ok", "env": {"FOO": "x", "foo": "y"}}
        }))
        .is_err()
    );
}

#[test]
fn command_environment_overrides_project_environment() {
    let project = HashMap::from([
        ("foo".to_owned(), "project".to_owned()),
        ("FOO".to_owned(), "project uppercase".to_owned()),
        ("BAR".to_owned(), "project".to_owned()),
    ]);
    let command = BTreeMap::from([
        ("FOO".to_owned(), "command".to_owned()),
        ("BAZ".to_owned(), "command".to_owned()),
    ]);
    assert_eq!(
        merge_command_environment(&project, &command),
        BTreeMap::from([
            ("BAR".to_owned(), "project".to_owned()),
            ("BAZ".to_owned(), "command".to_owned()),
            ("FOO".to_owned(), "command".to_owned()),
        ])
    );
}

#[test]
fn command_arguments_must_follow_delimiter() {
    assert_eq!(
        parse_project_command_args(&[
            OsString::from("build"),
            OsString::from("--"),
            OsString::from("--release"),
            OsString::from("two words"),
        ])
        .unwrap(),
        ProjectCommandInvocation::Run {
            name: "build".to_owned(),
            arguments: vec!["--release".to_owned(), "two words".to_owned()],
        }
    );
    assert!(
        parse_project_command_args(&[OsString::from("build"), OsString::from("--release")])
            .is_err()
    );
    assert!(
        parse_project_command_args(&[
            OsString::from("build"),
            OsString::from("--release"),
            OsString::from("--"),
            OsString::from("artifact"),
        ])
        .is_err()
    );
    assert_eq!(
        parse_project_command_args(&[OsString::from("--list")]).unwrap(),
        ProjectCommandInvocation::List
    );
}
