use super::*;
use std::fs;

fn setup(source: &str) -> (tempfile::TempDir, ProjectRegistry, Config) {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join(".zetta")).unwrap();
    fs::write(ProjectConfig::path_for(directory.path()), source).unwrap();
    let mut registry = ProjectRegistry::load_from(directory.path().join("registry.json")).unwrap();
    registry.add(directory.path()).unwrap();
    (directory, registry, Config::defaults(None, None))
}

fn approve(source: &str, root: &Path, base: &Config, registry: &mut ProjectRegistry) {
    let pending = ProjectConfig::parse_registered(source, root, base, registry).unwrap();
    registry
        .approve(root, &pending.pending_approval.unwrap().fingerprint)
        .unwrap();
}

#[test]
fn project_trust_blocks_filesystem_commands_and_preserves_other_settings() {
    let source = include_str!("../../project.config.example.json");
    let (directory, registry, base) = setup(source);
    let project = ProjectConfig::load_in_registry(directory.path(), &base, &registry).unwrap();
    assert!(project.pending_approval.is_some());
    assert!(project.commands.is_empty());
    let original = ProjectConfig::parse(source, directory.path(), &base).unwrap();
    assert_eq!(project.environment, original.environment);
    assert_eq!(project.initial_split, original.initial_split);
    assert_eq!(
        project.effective.profiles.len(),
        original.effective.profiles.len()
    );
    assert_eq!(
        project.effective.default_profile,
        original.effective.default_profile
    );
    assert_eq!(
        project.effective.pane_split_templates.len(),
        original.effective.pane_split_templates.len()
    );
    assert!(
        project
            .effective
            .pane_split_templates
            .contains_key("development")
    );
    assert_eq!(
        project.effective.working_directory,
        original.effective.working_directory
    );
    assert_eq!(project.theme, original.theme);
    assert_eq!(project.dark_theme, original.dark_theme);
    assert_eq!(
        project.effective.default_tab_icon,
        original.effective.default_tab_icon
    );
}

#[test]
fn project_trust_restores_the_approved_snapshot_and_blocks_changed_commands() {
    let source =
        r#"{"commands":{"build":"cargo build"},"env":{"BUILD":"1"},"initial_split":"three-right"}"#;
    let (directory, mut registry, base) = setup(source);
    approve(source, directory.path(), &base, &mut registry);
    registry.save().unwrap();
    let registry = ProjectRegistry::load_from(registry.path().to_owned()).unwrap();
    let approved = ProjectConfig::load_in_registry(directory.path(), &base, &registry).unwrap();
    assert!(approved.pending_approval.is_none());
    assert_eq!(approved.commands["build"].command, "cargo build");
    assert_eq!(approved.environment["BUILD"], "1");
    assert_eq!(approved.initial_split.as_deref(), Some("three-right"));
    let changed = source.replace("cargo build", "other command");
    fs::write(ProjectConfig::path_for(directory.path()), changed).unwrap();
    let blocked = ProjectConfig::load_in_registry(directory.path(), &base, &registry).unwrap();
    assert!(blocked.pending_approval.is_some());
    assert!(blocked.commands.is_empty());
    assert_eq!(blocked.environment["BUILD"], "1");
    assert_eq!(blocked.initial_split.as_deref(), Some("three-right"));
}

#[test]
fn project_trust_ignores_formatting_key_order_and_cosmetic_changes() {
    let source = r#"{"profiles":[{"name":"Runner","program":"sh","theme":"One Dark"}],"env":{"B":"2","A":"1"},"commands":{"build":{"command":"cargo build","env":{"B":"2","A":"1"}}}}"#;
    let (directory, mut registry, base) = setup(source);
    approve(source, directory.path(), &base, &mut registry);
    let reformatted = r#"{
        "theme":"Dracula",
        "commands":{"build":{"env":{"A":"1","B":"2"},"command":"cargo build"}},
        "env":{"A":"1","B":"2"},
        "profiles":[{"theme":"Dracula","program":"sh","name":"Runner"}]
    }"#;
    let project =
        ProjectConfig::parse_registered(reformatted, directory.path(), &base, &registry).unwrap();
    assert!(project.pending_approval.is_none());
    assert_eq!(project.theme.as_deref(), Some("Dracula"));
    assert_eq!(
        project.effective.profiles.last().unwrap().theme.as_deref(),
        Some("Dracula")
    );
}

#[test]
fn project_trust_only_commands_changes_require_renewed_approval() {
    let (directory, mut registry, base) = setup("{}");
    fs::create_dir(directory.path().join("work")).unwrap();
    let source = serde_json::json!({
        "working_directory":".",
        "default_profile":base.profiles[0].name,
        "profiles":[{"name":"Runner","program":"sh","args":["-c","echo original"]}],
        "env":{"SETTING":"original"},
        "commands":{"build":"echo original"},
        "initial_split":"three-right",
        "pane_split_templates":{"custom":{"split":"vertical","panes":[{},{}]}}
    });
    // Use the documented template syntax rather than maintaining another copy.
    let example: Value =
        serde_json::from_str(include_str!("../../project.config.example.json")).unwrap();
    let mut source = source;
    source["pane_split_templates"] = example["pane_split_templates"].clone();
    let encoded = source.to_string();
    approve(&encoded, directory.path(), &base, &mut registry);
    let variants = [
        ("working_directory", Value::String("work".into())),
        ("default_profile", Value::String("Runner".into())),
        (
            "profiles",
            serde_json::json!([{"name":"Runner","program":"sh","args":["-c","echo changed"]}]),
        ),
        ("env", serde_json::json!({"SETTING":"changed"})),
        ("commands", serde_json::json!({"build":"echo changed"})),
        ("initial_split", Value::String("three-left".into())),
        ("pane_split_templates", serde_json::json!({})),
    ];
    for (field, value) in variants {
        let mut changed = source.clone();
        changed[field] = value;
        let project = ProjectConfig::parse_registered(
            &changed.to_string(),
            directory.path(),
            &base,
            &registry,
        )
        .unwrap();
        assert_eq!(
            project.pending_approval.is_some(),
            field == "commands",
            "only commands must invalidate approval; changed {field}"
        );
    }
}

#[test]
fn project_trust_legacy_registration_is_not_silently_approved() {
    let source = r#"{"commands":{"build":"cargo build"}}"#;
    let (directory, registry, base) = setup(source);
    fs::write(
        registry.path(),
        serde_json::json!({"version":1,"projects":[directory.path()]}).to_string(),
    )
    .unwrap();
    let legacy = ProjectRegistry::load_from(registry.path().to_owned()).unwrap();
    let project = ProjectConfig::load_in_registry(directory.path(), &base, &legacy).unwrap();
    assert!(project.pending_approval.is_some());
    assert!(project.commands.is_empty());
}

#[test]
fn project_trust_acceptance_does_not_approve_a_replacement_file() {
    let source = r#"{"commands":{"build":"cargo build"}}"#;
    let (directory, mut registry, base) = setup(source);
    let reviewed = ProjectConfig::load_in_registry(directory.path(), &base, &registry).unwrap();
    fs::write(
        ProjectConfig::path_for(directory.path()),
        r#"{"commands":{"build":"replacement"}}"#,
    )
    .unwrap();
    registry
        .approve(
            directory.path(),
            &reviewed.pending_approval.unwrap().fingerprint,
        )
        .unwrap();
    let replacement = ProjectConfig::load_in_registry(directory.path(), &base, &registry).unwrap();
    assert!(replacement.pending_approval.is_some());
    assert!(replacement.commands.is_empty());
}

#[test]
fn project_trust_is_scoped_to_the_configuration_root_and_revoked_on_removal() {
    let source = r#"{"commands":{"build":"cargo build"}}"#;
    let (directory, mut registry, base) = setup(source);
    approve(source, directory.path(), &base, &mut registry);
    let child = directory.path().join("child");
    fs::create_dir_all(child.join(".zetta")).unwrap();
    let other = ProjectConfig::parse_registered(source, &child, &base, &registry).unwrap();
    assert!(other.pending_approval.is_some());
    registry.remove(directory.path()).unwrap();
    registry.add(directory.path()).unwrap();
    let removed =
        ProjectConfig::parse_registered(source, directory.path(), &base, &registry).unwrap();
    assert!(removed.pending_approval.is_some());
}

#[test]
fn project_trust_empty_or_cosmetic_configuration_needs_no_approval() {
    let (directory, registry, base) = setup("{}");
    let project = ProjectConfig::parse_registered(
        r#"{"theme":"Dracula","inactive_pane_opacity":0.7}"#,
        directory.path(),
        &base,
        &registry,
    )
    .unwrap();
    assert!(project.pending_approval.is_none());
    assert_eq!(project.theme.as_deref(), Some("Dracula"));
}

#[test]
fn project_trust_profile_appearance_overrides_need_no_execution_approval() {
    let (directory, registry, base) = setup("{}");
    let source = serde_json::json!({"profiles":[{"name":base.profiles[0].name,"theme":"Dracula","icon":"auto"}]});
    let project =
        ProjectConfig::parse_registered(&source.to_string(), directory.path(), &base, &registry)
            .unwrap();
    assert!(project.pending_approval.is_none());
    assert_eq!(
        project.effective.profiles[0].theme.as_deref(),
        Some("Dracula")
    );
    assert_eq!(
        project.effective.profiles[0].command,
        base.profiles[0].command
    );
}

#[test]
fn project_trust_editor_saves_approve_the_submitted_commands() {
    let original = r#"{"commands":{"build":"cargo build"}}"#;
    let (directory, mut registry, base) = setup(original);
    approve(original, directory.path(), &base, &mut registry);
    registry.save().unwrap();
    let (_, saved_registry, project) = save_from_editor(
        directory.path(),
        &base,
        r#"{"commands":{"build":"changed"}}"#,
        registry.path().to_path_buf(),
        &command_approval(original).unwrap().fingerprint,
    )
    .unwrap();
    assert!(project.pending_approval.is_none());
    assert_eq!(project.commands["build"].command, "changed");
    let persisted = ProjectRegistry::load_from(saved_registry.path().to_path_buf()).unwrap();
    let reopened = ProjectConfig::load_in_registry(directory.path(), &base, &persisted).unwrap();
    assert!(reopened.pending_approval.is_none());
    assert_eq!(reopened.commands["build"].command, "changed");
    fs::write(
        ProjectConfig::path_for(directory.path()),
        r#"{"commands":{"build":"external change"}}"#,
    )
    .unwrap();
    let external = ProjectConfig::load_in_registry(directory.path(), &base, &persisted).unwrap();
    assert!(external.pending_approval.is_some());
    assert!(external.commands.is_empty());
}

#[test]
fn project_trust_command_environment_changes_require_approval() {
    let source = r#"{"commands":{"build":{"command":"cargo build","env":{"MODE":"original"}}}}"#;
    let (directory, mut registry, base) = setup(source);
    approve(source, directory.path(), &base, &mut registry);
    let changed = source.replace("original", "changed");
    let project =
        ProjectConfig::parse_registered(&changed, directory.path(), &base, &registry).unwrap();
    assert!(project.pending_approval.is_some());
    assert!(project.commands.is_empty());
}

#[test]
fn project_trust_editor_approval_never_uses_replacement_commands_from_disk() {
    let original = r#"{"commands":{"build":"cargo build"}}"#;
    let (directory, mut registry, base) = setup(original);
    let submitted = r#"{"commands":{"build":"UI edit"}}"#;
    let approval = command_approval(submitted).unwrap();
    crate::project_form::save(directory.path(), &base, submitted).unwrap();
    // A writer replacing the editor's save before its reload must not obtain
    // the approval intended for the command snapshot submitted by the user.
    fs::write(
        ProjectConfig::path_for(directory.path()),
        r#"{"commands":{"build":"external replacement"}}"#,
    )
    .unwrap();
    registry
        .approve(directory.path(), &approval.fingerprint)
        .unwrap();
    let project = ProjectConfig::load_in_registry(directory.path(), &base, &registry).unwrap();
    assert!(project.pending_approval.is_some());
    assert!(project.commands.is_empty());
}

#[test]
fn project_trust_editor_saving_other_fields_does_not_approve_imported_commands() {
    let source = r#"{"commands":{"build":"imported command"}}"#;
    let (directory, registry, base) = setup(source);
    registry.save().unwrap();
    let (_, _, project) = save_from_editor(
        directory.path(),
        &base,
        r#"{"theme":"Dracula","commands":{"build":"imported command"}}"#,
        registry.path().to_path_buf(),
        &command_approval(source).unwrap().fingerprint,
    )
    .unwrap();
    assert!(project.pending_approval.is_some());
    assert!(project.commands.is_empty());
    assert_eq!(project.theme.as_deref(), Some("Dracula"));
}
