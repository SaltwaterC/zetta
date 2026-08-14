use super::*;

fn base_config() -> Config {
    Config::defaults(None, None)
}

#[test]
fn project_config_overlays_curated_fields_and_defaults_to_project_root() {
    let temporary = tempfile::tempdir().unwrap();
    let work = temporary.path().join("work");
    fs::create_dir(&work).unwrap();

    let project = ProjectConfig::parse(
        r#"{
            "theme": "One Dark",
            "working_directory": "work",
            "env": { "RUST_LOG": "debug" },
            "inactive_pane_opacity": 0.6,
            "initial_split": "three-right"
        }"#,
        temporary.path(),
        &base_config(),
    )
    .unwrap();

    assert_eq!(project.root, fs::canonicalize(temporary.path()).unwrap());
    assert_eq!(project.effective.theme.as_deref(), Some("One Dark"));
    assert_eq!(
        project.effective.working_directory.as_deref(),
        Some(work.as_path())
    );
    assert!(project.effective.working_directory_configured);
    assert_eq!(project.environment["RUST_LOG"], "debug");
    assert_eq!(project.initial_split.as_deref(), Some("three-right"));

    let defaulted = ProjectConfig::parse("{}", temporary.path(), &base_config()).unwrap();
    assert_eq!(
        defaulted.effective.working_directory.as_deref(),
        Some(defaulted.root.as_path())
    );
}

#[test]
fn project_config_rejects_unsafe_or_global_only_fields() {
    let temporary = tempfile::tempdir().unwrap();
    assert!(
        ProjectConfig::parse(
            r#"{"working_directory":"../outside"}"#,
            temporary.path(),
            &base_config(),
        )
        .unwrap_err()
        .to_string()
        .contains("stay inside")
    );
    assert!(
        ProjectConfig::parse(
            r#"{"env":{"ZETTA_PROCESS_ID":"spoof"}}"#,
            temporary.path(),
            &base_config(),
        )
        .unwrap_err()
        .to_string()
        .contains("reserved")
    );
    assert!(
        ProjectConfig::parse(r#"{"compact_mode":true}"#, temporary.path(), &base_config(),)
            .unwrap_err()
            .to_string()
            .contains("unrecognized")
    );
    assert!(
        ProjectConfig::parse(
            r#"{"initial_split":"missing"}"#,
            temporary.path(),
            &base_config(),
        )
        .unwrap_err()
        .to_string()
        .contains("not an available")
    );
}

#[test]
fn registry_round_trips_and_uses_the_deepest_ancestor() {
    let temporary = tempfile::tempdir().unwrap();
    let outer = temporary.path().join("outer");
    let inner = outer.join("inner");
    let child = inner.join("src");
    fs::create_dir_all(&child).unwrap();
    let registry_path = temporary.path().join("config").join("projects.json");
    let mut registry = ProjectRegistry::load_from(registry_path.clone()).unwrap();

    assert!(registry.add(&outer).unwrap());
    assert!(!registry.add(&outer).unwrap());
    assert!(registry.add(&inner).unwrap());
    registry.save().unwrap();

    let mut loaded = ProjectRegistry::load_from(registry_path).unwrap();
    assert_eq!(
        loaded.matching_root(&child),
        Some(&fs::canonicalize(&inner).unwrap())
    );
    assert_eq!(
        loaded.remove(&child),
        Some(fs::canonicalize(&inner).unwrap())
    );
    assert_eq!(
        loaded.matching_root(&child),
        Some(&fs::canonicalize(&outer).unwrap())
    );
}

#[test]
fn discovery_stops_at_the_repository_root_and_config_creation_is_non_destructive() {
    let temporary = tempfile::tempdir().unwrap();
    let nested = temporary.path().join("src").join("nested");
    fs::create_dir_all(&nested).unwrap();
    fs::create_dir(temporary.path().join(".git")).unwrap();
    assert_eq!(discover_project_config(&nested).unwrap(), None);

    let path = ensure_project_config(temporary.path()).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "{}\n");
    fs::write(&path, "{\"theme\":\"One Dark\"}\n").unwrap();
    ensure_project_config(temporary.path()).unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "{\"theme\":\"One Dark\"}\n"
    );
    assert_eq!(
        discover_project_config(&nested).unwrap(),
        Some(fs::canonicalize(temporary.path()).unwrap())
    );
}

#[test]
fn wsl_paths_are_recognized_lexically() {
    assert!(is_wsl_unc_path(Path::new(r"\\wsl$\Ubuntu\home\me")));
    assert!(is_wsl_unc_path(Path::new(
        r"\\wsl.localhost\Ubuntu\home\me"
    )));
    assert!(!is_wsl_unc_path(Path::new("/home/me")));
}

#[test]
fn documented_project_configuration_example_stays_valid() {
    let temporary = tempfile::tempdir().unwrap();
    let project = ProjectConfig::parse(
        include_str!("../../project.config.example.json"),
        temporary.path(),
        &base_config(),
    )
    .unwrap();

    assert_eq!(project.initial_split.as_deref(), Some("development"));
    assert_eq!(project.environment["PROJECT_ENV"], "development");
}
