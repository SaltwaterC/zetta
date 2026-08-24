use super::*;
use crate::config::profile_is_hidden;
use serde_json::json;

#[test]
fn profile_command_parser_accepts_repeatable_arguments_and_reset() {
    let parsed = parse_profile_args(
        &[
            "add".into(),
            "Dev Shell".into(),
            "--program".into(),
            "bash".into(),
            "--arg".into(),
            "-l".into(),
            "--arg".into(),
            "one two".into(),
            "--dark-theme".into(),
            "One Dark".into(),
            "--icon".into(),
            "fish".into(),
            "--config".into(),
            "custom.json".into(),
        ],
        None,
    )
    .unwrap();
    assert_eq!(parsed.config_path, Some(PathBuf::from("custom.json")));
    assert_eq!(
        parsed.command,
        ProfileCommand::Add {
            name: "Dev Shell".to_owned(),
            program: "bash".to_owned(),
            args: vec!["-l".to_owned(), "one two".to_owned()],
            theme: None,
            dark_theme: Some("One Dark".to_owned()),
            icon: Some(ProfileIcon::Fish),
        }
    );

    let parsed = parse_profile_args(
        &["theme".into(), "Dev Shell".into(), "--reset".into()],
        None,
    )
    .unwrap();
    assert_eq!(
        parsed.command,
        ProfileCommand::Theme {
            profile: "Dev Shell".to_owned(),
            theme: None,
        }
    );
}

#[test]
fn profile_help_documents_icon_operations_and_values() {
    let help = profile_operation_help(None);
    assert!(help.contains("profile icon PROFILE ICON"));
    assert!(help.contains("profile dark-theme PROFILE THEME"));
    assert!(help.contains("--icon ICON"));
    let add_help = profile_operation_help(Some("add"));
    assert!(add_help.contains("zetta, bash, zsh, fish, or auto"));
    assert!(add_help.contains("-d, --dark-theme THEME"));
    let icon_help = profile_operation_help(Some("icon"));
    assert!(icon_help.contains("--reset"));
    assert!(icon_help.contains("auto, zetta, bash, zsh, or fish"));
    let dark_theme_help = profile_operation_help(Some("dark-theme"));
    assert!(dark_theme_help.contains("--reset"));
}

#[test]
fn adding_a_profile_preserves_unrelated_configuration_fields() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    fs::write(&path, r#"{"theme":"One Dark","compact_mode":true}"#).unwrap();

    let result = run(
        ProfileCommand::Add {
            name: "Dev Shell".to_owned(),
            program: "bash".to_owned(),
            args: vec!["-l".to_owned(), "one two".to_owned()],
            theme: None,
            dark_theme: None,
            icon: None,
        },
        Some(&path),
    )
    .unwrap();
    assert!(result.changed);

    let root: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(root["compact_mode"], true);
    assert_eq!(root["profiles"][0]["name"], "Dev Shell");
    assert_eq!(root["profiles"][0]["args"], json!(["-l", "one two"]));
    Config::load(Some(&path), None).unwrap();
}

#[test]
fn profile_visibility_is_case_insensitive_and_idempotent() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    fs::write(
        &path,
        r#"{"profiles":[{"name":"Dev Shell","program":"bash"}]}"#,
    )
    .unwrap();

    let first = run(
        ProfileCommand::Disable {
            profile: "dEv ShElL".to_owned(),
        },
        Some(&path),
    )
    .unwrap();
    assert!(first.changed);
    let second = run(
        ProfileCommand::Disable {
            profile: "DEV SHELL".to_owned(),
        },
        Some(&path),
    )
    .unwrap();
    assert!(!second.changed);

    let config = Config::load(Some(&path), None).unwrap();
    assert!(profile_is_hidden(
        config
            .profiles
            .iter()
            .find(|profile| profile.name == "Dev Shell")
            .unwrap(),
        &config.hidden_profiles
    ));

    let enabled = run(
        ProfileCommand::Enable {
            profile: "DEV SHELL".to_owned(),
        },
        Some(&path),
    )
    .unwrap();
    assert!(enabled.changed);
    let enabled_again = run(
        ProfileCommand::Enable {
            profile: "dev shell".to_owned(),
        },
        Some(&path),
    )
    .unwrap();
    assert!(!enabled_again.changed);

    fs::write(
        &path,
        r#"{"profiles":[{"name":"Dev Shell","program":"bash","hidden":false}]}"#,
    )
    .unwrap();
    let explicitly_visible = run(
        ProfileCommand::Enable {
            profile: "dev shell".to_owned(),
        },
        Some(&path),
    )
    .unwrap();
    assert!(!explicitly_visible.changed);
}

#[test]
fn theme_mutation_sets_and_resets_a_profile_override() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    fs::write(
        &path,
        r#"{"profiles":[{"name":"Dev Shell","program":"bash","theme":"One Light"}]}"#,
    )
    .unwrap();

    let unknown = run(
        ProfileCommand::Theme {
            profile: "Dev Shell".to_owned(),
            theme: Some("Not a bundled theme".to_owned()),
        },
        Some(&path),
    )
    .unwrap_err();
    assert!(unknown.to_string().contains("unknown theme"));

    run(
        ProfileCommand::Theme {
            profile: "dev shell".to_owned(),
            theme: Some("One Dark".to_owned()),
        },
        Some(&path),
    )
    .unwrap();
    let configured: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(configured["profiles"][0]["theme"], "One Dark");

    let parsed = parse_profile_args(
        &["dark-theme".into(), "Dev Shell".into(), "One Dark".into()],
        None,
    )
    .unwrap();
    assert_eq!(
        parsed.command,
        ProfileCommand::DarkTheme {
            profile: "Dev Shell".to_owned(),
            theme: Some("One Dark".to_owned()),
        }
    );
    run(parsed.command, Some(&path)).unwrap();
    let configured: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(configured["profiles"][0]["dark_theme"], "One Dark");

    run(
        ProfileCommand::DarkTheme {
            profile: "Dev Shell".to_owned(),
            theme: None,
        },
        Some(&path),
    )
    .unwrap();

    run(
        ProfileCommand::Theme {
            profile: "DEV SHELL".to_owned(),
            theme: None,
        },
        Some(&path),
    )
    .unwrap();
    let reset: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert!(reset["profiles"][0].get("theme").is_none());
    assert!(reset["profiles"][0].get("dark_theme").is_none());
}

#[test]
fn icon_mutation_sets_and_resets_a_profile_override() {
    let parsed =
        parse_profile_args(&["icon".into(), "Dev Shell".into(), "zsh".into()], None).unwrap();
    assert_eq!(
        parsed.command,
        ProfileCommand::Icon {
            profile: "Dev Shell".to_owned(),
            icon: Some(ProfileIcon::Zsh),
        }
    );

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    fs::write(
        &path,
        r#"{"profiles":[{"name":"Dev Shell","program":"bash"}]}"#,
    )
    .unwrap();
    run(
        ProfileCommand::Icon {
            profile: "Dev Shell".to_owned(),
            icon: Some(ProfileIcon::Fish),
        },
        Some(&path),
    )
    .unwrap();
    let configured: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(configured["profiles"][0]["icon"], "fish");

    run(
        ProfileCommand::Icon {
            profile: "Dev Shell".to_owned(),
            icon: None,
        },
        Some(&path),
    )
    .unwrap();
    let reset: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert!(reset["profiles"][0].get("icon").is_none());
}

#[test]
fn adding_a_duplicate_profile_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    fs::write(
        &path,
        r#"{"profiles":[{"name":"Dev Shell","program":"bash"}]}"#,
    )
    .unwrap();

    let error = run(
        ProfileCommand::Add {
            name: "dev shell".to_owned(),
            program: "zsh".to_owned(),
            args: Vec::new(),
            theme: None,
            dark_theme: None,
            icon: None,
        },
        Some(&path),
    )
    .unwrap_err();
    assert!(error.to_string().contains("already exists"));
}

#[test]
fn missing_configuration_is_created_for_a_profile_addition() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("nested/config.json");

    let result = run(
        ProfileCommand::Add {
            name: "Dev Shell".to_owned(),
            program: "bash".to_owned(),
            args: Vec::new(),
            theme: None,
            dark_theme: None,
            icon: None,
        },
        Some(&path),
    )
    .unwrap();
    assert!(result.changed);
    assert!(path.is_file());
    let config = Config::load(Some(&path), None).unwrap();
    assert!(
        config
            .profiles
            .iter()
            .any(|profile| profile.name == "Dev Shell")
    );
}

#[test]
fn removal_rejects_detected_and_active_default_profiles() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    fs::write(
        &path,
        r#"{"default_profile":"Dev Shell","profiles":[{"name":"Dev Shell","program":"bash"},{"name":"System","hidden":true}]}"#,
    )
    .unwrap();

    let default_error = run(
        ProfileCommand::Remove {
            profile: "dev shell".to_owned(),
        },
        Some(&path),
    )
    .unwrap_err();
    assert!(default_error.to_string().contains("active default"));

    let detected_error = run(
        ProfileCommand::Remove {
            profile: "system".to_owned(),
        },
        Some(&path),
    )
    .unwrap_err();
    assert!(detected_error.to_string().contains("detected profile"));
}

#[test]
fn malformed_configuration_is_not_overwritten() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    let original = "{ malformed";
    fs::write(&path, original).unwrap();

    assert!(
        run(
            ProfileCommand::Add {
                name: "Dev".to_owned(),
                program: "bash".to_owned(),
                args: Vec::new(),
                theme: None,
                dark_theme: None,
                icon: None,
            },
            Some(&path),
        )
        .is_err()
    );
    assert_eq!(fs::read_to_string(path).unwrap(), original);
}

#[test]
fn profile_theme_names_are_sorted_and_include_bundled_themes() {
    let themes = profile_theme_names().unwrap();
    assert!(themes.windows(2).all(|window| window[0] < window[1]));
    assert!(themes.contains(&"One Dark".to_owned()));
}
