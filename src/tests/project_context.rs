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

    let first = detect_project_for_directory(&child, &registry, &base, &[]);
    let registered = first.registered_root.unwrap();
    assert!(first.config.unwrap().is_ok());
    assert!(first.offer_root.is_none());

    let cached =
        detect_project_for_directory(&child, &registry, &base, std::slice::from_ref(&registered));
    assert_eq!(cached.registered_root, Some(registered));
    assert!(cached.config.is_none());
}

#[test]
fn discovery_offers_an_unregistered_repository_and_an_unreachable_path_stays_lexical() {
    let temporary = tempfile::tempdir().unwrap();
    let child = temporary.path().join("src");
    fs::create_dir_all(temporary.path().join(".git")).unwrap();
    fs::create_dir_all(temporary.path().join(".zetta")).unwrap();
    fs::create_dir(&child).unwrap();
    fs::write(ProjectConfig::path_for(temporary.path()), "{}\n").unwrap();
    let registry = ProjectRegistry::load_from(temporary.path().join("registry.json")).unwrap();
    let base = Config::defaults(None, None);

    let native = detect_project_for_directory(&child, &registry, &base, &[]);
    assert_eq!(
        native.offer_root,
        Some(fs::canonicalize(temporary.path()).unwrap())
    );

    // A directory that cannot be canonicalized (a deleted directory, or a WSL
    // UNC path while the share is unreachable) falls back to lexical matching:
    // it is never offered as a project, but a registered root is still
    // recognized there.
    let missing = temporary.path().join("missing");
    let result = detect_project_for_directory(&missing, &registry, &base, &[]);
    assert!(result.registered_root.is_none());
    assert!(result.offer_root.is_none());
}

#[cfg(windows)]
#[test]
fn registered_wsl_roots_match_lexically_while_the_share_is_unreachable() {
    let temporary = tempfile::tempdir().unwrap();
    let root = PathBuf::from(r"\\wsl.localhost\ZettaRegressionTest\home\me\project");
    let child = root.join("src");
    let registry_path = temporary.path().join("registry.json");
    fs::write(
        &registry_path,
        serde_json::to_string(&serde_json::json!({
            "version": 1,
            "projects": [root]
        }))
        .unwrap(),
    )
    .unwrap();
    let registry = ProjectRegistry::load_from(registry_path).unwrap();
    let base = Config::defaults(None, None);

    let result =
        detect_project_for_directory(&child, &registry, &base, std::slice::from_ref(&root));
    assert_eq!(result.registered_root, Some(root));
    assert!(result.config.is_none());
    assert!(result.offer_root.is_none());
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

fn icon_test_project(icon: Option<IconName>) -> ProjectConfig {
    let mut effective = Config::defaults(None, None);
    effective.default_tab_icon = icon;
    ProjectConfig {
        root: PathBuf::from("/project"),
        effective,
        environment: HashMap::new(),
        initial_split: None,
    }
}

#[test]
fn leaving_a_project_restores_the_tab_icon_it_had_before_entering() {
    let project = icon_test_project(Some(IconName::Folder));
    let mut inherited = HashMap::new();
    let mut icon = Some(IconName::Terminal);

    apply_project_tab_icon(
        1,
        &mut icon,
        TabIconOverride::None,
        Some(&project),
        &mut inherited,
    );
    assert_eq!(icon, Some(IconName::Folder));
    assert_eq!(inherited.get(&1), Some(&Some(IconName::Terminal)));

    apply_project_tab_icon(1, &mut icon, TabIconOverride::None, None, &mut inherited);
    assert_eq!(icon, Some(IconName::Terminal));
    assert!(inherited.is_empty());
}

#[test]
fn repeated_project_application_and_transitions_keep_the_original_fallback() {
    let first = icon_test_project(Some(IconName::Folder));
    let second = icon_test_project(Some(IconName::Star));
    let mut inherited = HashMap::new();
    let mut icon = Some(IconName::Terminal);

    apply_project_tab_icon(
        1,
        &mut icon,
        TabIconOverride::None,
        Some(&first),
        &mut inherited,
    );
    apply_project_tab_icon(
        1,
        &mut icon,
        TabIconOverride::None,
        Some(&first),
        &mut inherited,
    );
    apply_project_tab_icon(
        1,
        &mut icon,
        TabIconOverride::None,
        Some(&second),
        &mut inherited,
    );
    assert_eq!(icon, Some(IconName::Star));
    assert_eq!(inherited.get(&1), Some(&Some(IconName::Terminal)));

    apply_project_tab_icon(1, &mut icon, TabIconOverride::None, None, &mut inherited);
    assert_eq!(icon, Some(IconName::Terminal));
    assert!(inherited.is_empty());
}

#[test]
fn a_tab_opened_directly_into_a_project_must_not_be_seeded_with_the_projects_own_icon() {
    // A tab created while a project is already active must start from the
    // non-project default (see `Zetta::open_tab_with_profile_context`), not
    // from `project.effective.default_tab_icon`. Seeding it with the
    // project's own icon here, as a regression would, snapshots that same
    // icon as the "original" — so leaving the project never changes it.
    let project = icon_test_project(Some(IconName::Folder));
    let mut inherited = HashMap::new();
    let mut icon = project.effective.default_tab_icon;

    apply_project_tab_icon(
        1,
        &mut icon,
        TabIconOverride::None,
        Some(&project),
        &mut inherited,
    );
    apply_project_tab_icon(1, &mut icon, TabIconOverride::None, None, &mut inherited);

    // Demonstrates the bug this test guards against: the icon is stuck on
    // the project's icon instead of resetting to a real default.
    assert_eq!(icon, Some(IconName::Folder));
}

#[test]
fn an_explicit_tab_icon_wins_over_project_changes_and_reapplication() {
    let first = icon_test_project(Some(IconName::Folder));
    let second = icon_test_project(Some(IconName::Star));
    let mut inherited = HashMap::new();
    let mut icon = Some(IconName::Terminal);

    apply_project_tab_icon(
        1,
        &mut icon,
        TabIconOverride::Icon(IconName::Terminal),
        Some(&first),
        &mut inherited,
    );
    apply_project_tab_icon(
        1,
        &mut icon,
        TabIconOverride::Icon(IconName::Terminal),
        Some(&first),
        &mut inherited,
    );
    apply_project_tab_icon(
        1,
        &mut icon,
        TabIconOverride::Icon(IconName::Terminal),
        Some(&second),
        &mut inherited,
    );
    apply_project_tab_icon(
        1,
        &mut icon,
        TabIconOverride::Icon(IconName::Terminal),
        None,
        &mut inherited,
    );

    assert_eq!(icon, Some(IconName::Terminal));
    assert!(inherited.is_empty());
}

#[test]
fn an_explicit_hidden_tab_icon_wins_over_project_changes_and_leaving() {
    let project = icon_test_project(Some(IconName::Folder));
    let mut inherited = HashMap::new();
    let mut icon = None;

    apply_project_tab_icon(
        1,
        &mut icon,
        TabIconOverride::Hidden,
        Some(&project),
        &mut inherited,
    );
    apply_project_tab_icon(1, &mut icon, TabIconOverride::Hidden, None, &mut inherited);

    assert_eq!(icon, None);
    assert!(inherited.is_empty());
}

#[gpui::test]
fn active_project_theme_overrides_a_profile_theme_it_never_mentioned(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        theme::init(theme::LoadThemes::All(Box::new(ZettaAssets)), cx);
        let registry = ThemeRegistry::global(cx);
        theme_settings::load_bundled_themes(&registry);

        // A profile whose theme came from the global configuration, not from
        // this project's own `profiles` overlay (see `merge_profiles`):
        // opening the project must not let that inherited theme keep
        // outranking the project's own `theme`.
        let profile = Profile {
            name: "bash".to_owned(),
            command: Shell::Program("bash".to_owned()),
            theme: Some("Solarized Light".to_owned()),
            icon: ProfileIcon::default(),
        };

        let mut effective = Config::defaults(None, None);
        effective.theme = Some("Solarized Dark".to_owned());
        let project = ProjectConfig {
            root: PathBuf::from("/project"),
            effective,
            environment: HashMap::new(),
            initial_split: None,
        };

        let theme = resolve_project_profile_theme(&profile, Some(&project), cx)
            .unwrap()
            .unwrap();
        assert_eq!(theme.name.as_ref(), "Solarized Dark");

        // With no project active, the profile's own theme still applies.
        let theme = resolve_project_profile_theme(&profile, None, cx)
            .unwrap()
            .unwrap();
        assert_eq!(theme.name.as_ref(), "Solarized Light");

        // A project that sets no theme of its own falls back to the profile.
        let project_without_theme = ProjectConfig {
            root: PathBuf::from("/project"),
            effective: Config::defaults(None, None),
            environment: HashMap::new(),
            initial_split: None,
        };
        let theme = resolve_project_profile_theme(&profile, Some(&project_without_theme), cx)
            .unwrap()
            .unwrap();
        assert_eq!(theme.name.as_ref(), "Solarized Light");
    });
}
