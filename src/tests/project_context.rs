use super::*;

#[test]
fn registered_detection_loads_once_then_reuses_the_cached_project() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("project");
    let child = root.join("src");
    fs::create_dir_all(root.join(".zetta")).unwrap();
    fs::create_dir_all(&child).unwrap();
    fs::write(ProjectConfig::path_for(&root), "{}\n").unwrap();
    let registry_path = temporary.path().join("registry.json");
    let mut registry = ProjectRegistry::load_from(registry_path).unwrap();
    registry.add(&root).unwrap();
    let base = Config::defaults(None, None);

    let first = detect_project_for_directory(&child, &registry, &base, &[], true);
    let registered = first.registered_root.unwrap();
    assert!(first.config.unwrap().is_ok());
    assert!(first.offer_root.is_none());

    let cached = detect_project_for_directory(
        &child,
        &registry,
        &base,
        std::slice::from_ref(&registered),
        true,
    );
    assert_eq!(cached.registered_root, Some(registered));
    assert!(cached.config.is_none());
}

#[test]
fn native_discovery_offers_an_unregistered_repository_but_wsl_mode_does_not_scan() {
    let temporary = tempfile::tempdir().unwrap();
    let child = temporary.path().join("src");
    fs::create_dir_all(temporary.path().join(".git")).unwrap();
    fs::create_dir_all(temporary.path().join(".zetta")).unwrap();
    fs::create_dir(&child).unwrap();
    fs::write(ProjectConfig::path_for(temporary.path()), "{}\n").unwrap();
    let registry = ProjectRegistry::load_from(temporary.path().join("registry.json")).unwrap();
    let base = Config::defaults(None, None);

    let native = detect_project_for_directory(&child, &registry, &base, &[], true);
    assert_eq!(
        native.offer_root,
        Some(fs::canonicalize(temporary.path()).unwrap())
    );
    let lexical_only = detect_project_for_directory(&child, &registry, &base, &[], false);
    assert!(lexical_only.offer_root.is_none());
}

#[test]
fn invalidating_detection_never_reuses_an_in_flight_generation() {
    let registry = ProjectRegistry::load_from(PathBuf::from("registry.json")).unwrap();
    let mut projects = ProjectState::new(registry);
    let directory = PathBuf::from("project");

    let first = projects.begin_detection(7, directory.clone()).unwrap();
    projects.invalidate_detections();
    let second = projects.begin_detection(7, directory).unwrap();

    assert_ne!(first, second);
}

#[test]
fn wsl_project_directories_are_lexical_but_cannot_escape_the_distribution() {
    let profile = Profile {
        name: "WSL: Ubuntu".to_owned(),
        command: Shell::WithArguments {
            program: "wsl.exe".to_owned(),
            args: vec!["--distribution".to_owned(), "Ubuntu".to_owned()],
            title_override: None,
        },
        theme: None,
        icon: ProfileIcon::default(),
    };

    assert_eq!(
        wsl_reported_directory(&profile, "/home/me/project"),
        Some(PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me\project"))
    );
    assert_eq!(wsl_reported_directory(&profile, "/home/me/../../etc"), None);
}
