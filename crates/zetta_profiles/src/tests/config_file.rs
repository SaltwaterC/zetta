use super::*;
#[test]
fn backend_overlay_preserves_discovered_command_and_explicit_default() {
    let mut profiles = vec![ProfileDefinition {
        name: "CMD".into(),
        command: ProfileCommand::with_args("cmd.exe", vec!["/d".into()]),
    }];
    merge(
        &mut profiles,
        Configured {
            backend: ConsoleBackend::Conpty,
            profiles: vec![ConfiguredProfile {
                name: "cmd".into(),
                command: None,
                backend: Some(ConsoleBackend::ConsoleServer),
            }],
        },
    );
    assert_eq!(profiles[0].command.program.as_deref(), Some("cmd.exe"));
    assert_eq!(profiles[0].command.args, ["/d"]);
    assert_eq!(
        profiles[0].command.console_backend,
        ConsoleBackend::ConsoleServer
    );
    merge(
        &mut profiles,
        Configured {
            backend: ConsoleBackend::Conpty,
            profiles: vec![ConfiguredProfile {
                name: "cmd".into(),
                command: None,
                backend: Some(ConsoleBackend::Conpty),
            }],
        },
    );
    assert_eq!(profiles[0].command.console_backend, ConsoleBackend::Conpty);
}
#[test]
fn backend_values_are_typed_and_default_is_omitted_from_wire() {
    for value in ["null", "\"unknown\"", "true"] {
        assert!(
            serde_json::from_str::<ProfileEntry>(&format!(
                r#"{{"name":"CMD","windows_console_backend":{value}}}"#
            ))
            .is_err()
        );
    }
    let entry: ProfileEntry = serde_json::from_str(r#"{"name":"CMD"}"#).unwrap();
    assert_eq!(entry.backend, None);
    let mut command = ProfileCommand::program("cmd.exe");
    assert!(
        serde_json::to_value(&command)
            .unwrap()
            .get("windows_console_backend")
            .is_none()
    );
    command.console_backend = ConsoleBackend::ConsoleServer;
    let encoded = serde_json::to_value(&command).unwrap();
    assert_eq!(encoded["windows_console_backend"], "console-server");
    assert_eq!(
        serde_json::from_value::<ProfileCommand>(encoded).unwrap(),
        command
    );
}
#[test]
fn top_level_backend_is_every_unspecified_profiles_default() {
    let discovered = |name: &str| ProfileDefinition {
        name: name.into(),
        command: ProfileCommand::program(format!("{name}.exe")),
    };
    let mut profiles = vec![discovered("CMD"), discovered("PowerShell")];
    merge(
        &mut profiles,
        Configured {
            backend: ConsoleBackend::ConsoleServer,
            profiles: vec![
                ConfiguredProfile {
                    name: "powershell".into(),
                    command: None,
                    backend: Some(ConsoleBackend::Conpty),
                },
                ConfiguredProfile {
                    name: "Python".into(),
                    command: Some(ProfileCommand::program("python.exe")),
                    backend: None,
                },
            ],
        },
    );
    let backends: Vec<_> = profiles
        .iter()
        .map(|profile| (profile.name.as_str(), profile.command.console_backend))
        .collect();
    assert_eq!(
        backends,
        [
            ("CMD", ConsoleBackend::ConsoleServer),
            ("PowerShell", ConsoleBackend::Conpty),
            ("Python", ConsoleBackend::ConsoleServer),
        ]
    );
}
#[test]
fn top_level_backend_is_read_from_the_file_and_typed() {
    let directory =
        std::env::temp_dir().join(format!("zetta-profiles-backend-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("config.json");
    std::fs::write(&path, r#"{"windows_console_backend":"console-server"}"#).unwrap();
    let configured = configured_profiles(&path).unwrap();
    assert_eq!(configured.backend, ConsoleBackend::ConsoleServer);
    std::fs::write(&path, "{}").unwrap();
    assert_eq!(
        configured_profiles(&path).unwrap().backend,
        ConsoleBackend::Conpty
    );
    for value in ["null", "\"unknown\""] {
        std::fs::write(&path, format!(r#"{{"windows_console_backend":{value}}}"#)).unwrap();
        assert!(configured_profiles(&path).is_err(), "{value} was accepted");
    }
    std::fs::remove_dir_all(&directory).unwrap();
}
#[test]
fn backend_spellings_round_trip() {
    for backend in [ConsoleBackend::Conpty, ConsoleBackend::ConsoleServer] {
        assert_eq!(ConsoleBackend::parse(backend.as_str()), Some(backend));
        assert_eq!(
            serde_json::to_value(backend).unwrap(),
            backend.as_str(),
            "the file and the settings table must spell it the same way"
        );
    }
    assert_eq!(ConsoleBackend::parse("ConPTY"), None);
}
