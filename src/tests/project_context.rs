use super::*;
use crate::project::PROJECT_CONFIG_DIRECTORY;
use std::{path::Path, process::Command};

fn git(directory: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .current_dir(directory)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn base_config() -> Config {
    Config::defaults(None, None)
}

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
fn registered_main_projects_resolve_managed_worktree_aliases_and_local_configs() {
    let temporary = tempfile::tempdir().unwrap();
    let main = temporary.path().join("project");
    let linked = temporary.path().join("worktrees").join("feature");
    fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    git(&main, &["config", "user.email", "test@example.invalid"]);
    git(&main, &["config", "user.name", "Zetta Test"]);
    fs::write(main.join("file"), "base\n").unwrap();
    git(&main, &["add", "file"]);
    git(
        &main,
        &["-c", "commit.gpgsign=false", "commit", "-qm", "initial"],
    );
    fs::create_dir_all(main.join(PROJECT_CONFIG_DIRECTORY)).unwrap();
    fs::write(ProjectConfig::path_for(&main), "{\"theme\":\"One Dark\"}\n").unwrap();
    fs::create_dir_all(linked.parent().unwrap()).unwrap();
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wt/feature",
            linked.to_str().unwrap(),
        ],
    );
    let child = linked.join("src");
    fs::create_dir(&child).unwrap();

    let registry_path = temporary.path().join("registry.json");
    let mut registry = ProjectRegistry::load_from(registry_path).unwrap();
    registry.add(&main).unwrap();
    // A stale duplicate registration must not win over the main project.
    registry.add(&linked).unwrap();
    let main = fs::canonicalize(main).unwrap();

    let resolution = resolve_registered_project(&child, &registry);
    assert_eq!(resolution.root, Some(main.clone()));
    assert_eq!(resolution.config_root, Some(main.clone()));
    assert_eq!(
        resolution
            .managed_worktree
            .as_ref()
            .map(|worktree| &worktree.name),
        Some(&"feature".to_owned())
    );

    let detection = detect_project_for_directory(&child, &registry, &base_config(), &[]);
    assert_eq!(detection.registered_root, Some(main.clone()));
    assert!(detection.config.unwrap().is_ok());
    assert!(detection.offer_root.is_none());

    fs::create_dir_all(linked.join(PROJECT_CONFIG_DIRECTORY)).unwrap();
    fs::write(
        ProjectConfig::path_for(&linked),
        r#"{"commands":{"worktree":"echo local"}}"#,
    )
    .unwrap();
    let linked = fs::canonicalize(linked).unwrap();
    let resolution = resolve_registered_project(&child, &registry);
    assert_eq!(resolution.root, Some(main.clone()));
    assert_eq!(resolution.config_root, Some(linked.clone()));
    assert!(resolution.managed_worktree.is_some());

    let detection = detect_project_for_directory(&child, &registry, &base_config(), &[]);
    assert_eq!(detection.registered_root, Some(main));
    let project = detection.config.unwrap().unwrap();
    assert_eq!(project.root, linked);
    assert_eq!(project.commands["worktree"].command, "echo local");
    assert!(detection.offer_root.is_none());
}

#[test]
fn an_unregistered_managed_worktree_keeps_the_normal_import_offer() {
    let temporary = tempfile::tempdir().unwrap();
    let main = temporary.path().join("project");
    let linked = temporary.path().join("linked");
    fs::create_dir(&main).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    git(&main, &["config", "user.email", "test@example.invalid"]);
    git(&main, &["config", "user.name", "Zetta Test"]);
    fs::write(main.join("file"), "base\n").unwrap();
    git(&main, &["add", "file"]);
    git(
        &main,
        &["-c", "commit.gpgsign=false", "commit", "-qm", "initial"],
    );
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wt/unregistered",
            linked.to_str().unwrap(),
        ],
    );
    fs::create_dir_all(linked.join(PROJECT_CONFIG_DIRECTORY)).unwrap();
    fs::write(ProjectConfig::path_for(&linked), "{}\n").unwrap();
    let child = linked.join("src");
    fs::create_dir(&child).unwrap();

    let registry = ProjectRegistry::load_from(temporary.path().join("registry.json")).unwrap();
    let detection = detect_project_for_directory(&child, &registry, &base_config(), &[]);
    assert!(detection.registered_root.is_none());
    assert_eq!(
        detection.offer_root,
        Some(fs::canonicalize(&linked).unwrap())
    );
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

#[test]
fn discovery_does_not_offer_a_repository_without_a_project_config() {
    let temporary = tempfile::tempdir().unwrap();
    let child = temporary.path().join("src");
    fs::create_dir_all(temporary.path().join(".git")).unwrap();
    fs::create_dir(&child).unwrap();
    let registry = ProjectRegistry::load_from(temporary.path().join("registry.json")).unwrap();
    let base = Config::defaults(None, None);

    let result = detect_project_for_directory(&child, &registry, &base, &[]);

    assert!(result.registered_root.is_none());
    assert!(result.config.is_none());
    assert!(result.offer_root.is_none());
}

#[test]
fn detection_leaves_a_registered_project_when_the_shell_moves_outside_it() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("project");
    let outside = temporary.path().join("outside");
    fs::create_dir_all(root.join(".zetta")).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(ProjectConfig::path_for(&root), "{}\n").unwrap();
    let mut registry = ProjectRegistry::load_from(temporary.path().join("registry.json")).unwrap();
    registry.add(&root).unwrap();
    let base = Config::defaults(None, None);

    let result = detect_project_for_directory(&outside, &registry, &base, &[]);

    assert!(result.registered_root.is_none());
    assert!(result.config.is_none());
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
        dark_theme: None,
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
        theme: None,
        dark_theme: None,
        environment: HashMap::new(),
        commands: std::collections::BTreeMap::new(),
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
            dark_theme: Some("One Dark".to_owned()),
            icon: ProfileIcon::default(),
        };

        let mut effective = Config::defaults(None, None);
        effective.theme = Some("Solarized Dark".to_owned());
        effective.dark_theme = Some("Gruvbox Dark".to_owned());
        let project = ProjectConfig {
            root: PathBuf::from("/project"),
            effective,
            theme: Some("Solarized Dark".to_owned()),
            dark_theme: Some("Gruvbox Dark".to_owned()),
            environment: HashMap::new(),
            commands: std::collections::BTreeMap::new(),
            initial_split: None,
        };

        let theme = resolve_project_profile_theme(&profile, Some(&project), cx)
            .unwrap()
            .unwrap();
        assert_eq!(theme.name.as_ref(), "Solarized Dark");

        let explicit = resolve_terminal_theme(Some("One Dark"), None, &profile, Some(&project), cx)
            .unwrap()
            .unwrap();
        assert_eq!(explicit.name.as_ref(), "One Dark");

        let tab_override =
            resolve_terminal_theme(None, Some("Gruvbox Dark"), &profile, Some(&project), cx)
                .unwrap()
                .unwrap();
        assert_eq!(tab_override.name.as_ref(), "Gruvbox Dark");

        let pane_override = resolve_terminal_theme(
            Some("One Light"),
            Some("Gruvbox Dark"),
            &profile,
            Some(&project),
            cx,
        )
        .unwrap()
        .unwrap();
        assert_eq!(pane_override.name.as_ref(), "One Light");

        // With no project active, the profile's own theme still applies.
        let theme = resolve_project_profile_theme(&profile, None, cx)
            .unwrap()
            .unwrap();
        assert_eq!(theme.name.as_ref(), "Solarized Light");

        *SystemAppearance::global_mut(cx) = SystemAppearance(theme::Appearance::Dark);
        let theme = resolve_project_profile_theme(&profile, Some(&project), cx)
            .unwrap()
            .unwrap();
        assert_eq!(theme.name.as_ref(), "Gruvbox Dark");
        let explicit = resolve_terminal_theme(Some("One Dark"), None, &profile, Some(&project), cx)
            .unwrap()
            .unwrap();
        assert_eq!(explicit.name.as_ref(), "One Dark");

        // A project that sets no theme of its own falls back to the profile.
        let project_without_theme = ProjectConfig {
            root: PathBuf::from("/project"),
            effective: Config::defaults(None, None),
            theme: None,
            dark_theme: None,
            environment: HashMap::new(),
            commands: std::collections::BTreeMap::new(),
            initial_split: None,
        };
        let theme = resolve_project_profile_theme(&profile, Some(&project_without_theme), cx)
            .unwrap()
            .unwrap();
        assert_eq!(theme.name.as_ref(), "One Dark");

        *SystemAppearance::global_mut(cx) = SystemAppearance(theme::Appearance::Light);
        let theme = resolve_project_profile_theme(&profile, Some(&project_without_theme), cx)
            .unwrap()
            .unwrap();
        assert_eq!(theme.name.as_ref(), "Solarized Light");
    });
}
