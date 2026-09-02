use super::*;

use serde_json::json;

fn base_config() -> Config {
    Config::defaults(None, None)
}

/// A user configuration that adds a profile and a template of its own, so tests
/// can tell the inherited layer apart from the project's overrides.
fn base_config_with_user_template() -> Config {
    Config::parse(
        &serde_json::to_string(&json!({
            "profiles": [{ "name": "Toolbox", "program": "/bin/sh" }],
            "pane_split_templates": {
                "user-pair": { "layout": { "vertical": [{ "label": "left" }, { "label": "right" }] } }
            }
        }))
        .unwrap(),
        None,
        None,
    )
    .unwrap()
}

fn parse(source: &str, base: &Config) -> ProjectForm {
    ProjectForm::parse(source, Path::new("/projects/demo/.zetta/config.json"), base).unwrap()
}

fn saved(form: &ProjectForm) -> Value {
    serde_json::from_str(&form.to_json().unwrap()).unwrap()
}

#[test]
fn an_absent_field_stays_absent_so_it_keeps_inheriting() {
    let form = parse("{}", &base_config());

    assert_eq!(form.theme, None);
    assert_eq!(form.dark_theme, None);
    assert_eq!(form.default_profile, None);
    assert_eq!(form.default_tab_icon, ProjectTabIcon::Inherit);
    assert_eq!(form.inactive_pane_opacity, None);
    assert!(form.working_directory.text.is_empty());
    assert!(form.environment.is_empty());
    assert!(form.commands.is_empty());
    assert!(form.profiles.is_empty());
    assert_eq!(form.initial_split, None);
    // A file of `{}` must round-trip to `{}`: writing resolved defaults back
    // would silently pin the project to whatever the user configuration happens
    // to say today.
    assert_eq!(saved(&form), json!({}));
}

#[test]
fn an_empty_file_loads_as_an_empty_overlay() {
    // `project add` writes `{}`, but a project directory can also hold a file
    // that was truncated by hand.
    let form = parse("   \n", &base_config());

    assert_eq!(saved(&form), json!({}));
}

#[test]
fn every_curated_field_round_trips_through_the_form() {
    let source = json!({
        "theme": "One Dark",
        "dark_theme": "Dracula",
        "working_directory": "crates/app",
        "default_profile": "Toolbox",
        "default_tab_icon": "terminal",
        "inactive_pane_opacity": 0.8,
        "env": { "RUST_LOG": "debug", "PROJECT_ENV": "development" },
        "initial_split": "user-pair",
        "profiles": [{ "name": "Toolbox", "theme": "One Dark", "dark_theme": "Dracula", "hidden": true }]
    });
    let form = parse(
        &serde_json::to_string(&source).unwrap(),
        &base_config_with_user_template(),
    );

    assert_eq!(form.theme.as_deref(), Some("One Dark"));
    assert_eq!(form.dark_theme.as_deref(), Some("Dracula"));
    assert_eq!(form.working_directory.text, "crates/app");
    assert_eq!(form.default_profile.as_deref(), Some("Toolbox"));
    assert_eq!(
        form.default_tab_icon,
        ProjectTabIcon::Icon(IconName::Terminal)
    );
    assert_eq!(form.inactive_pane_opacity, Some(0.8));
    // Environment rows are ordered by name so the list does not reshuffle
    // between edits.
    assert_eq!(
        form.environment
            .iter()
            .map(|entry| entry.name.text.as_str())
            .collect::<Vec<_>>(),
        ["PROJECT_ENV", "RUST_LOG"]
    );
    assert_eq!(form.initial_split.as_deref(), Some("user-pair"));
    assert!(form.profiles[0].hidden);

    assert_eq!(saved(&form), source);
}

#[test]
fn a_profile_program_serializes_with_its_arguments_and_icon() {
    let mut form = parse("{}", &base_config());
    form.profiles.push(ProjectProfileForm {
        name: TextField::new(" Runner "),
        program: TextField::new("/usr/bin/env"),
        arguments: TextField::new("bash, -l"),
        theme: None,
        dark_theme: None,
        icon: Some(ProfileIcon::Bash),
        hidden: false,
    });

    assert_eq!(
        saved(&form),
        json!({
            "profiles": [{
                "name": "Runner",
                "program": "/usr/bin/env",
                "args": ["bash", "-l"],
                "icon": "bash"
            }]
        })
    );
}

#[test]
fn project_commands_round_trip_in_sorted_string_and_object_forms() {
    let source = json!({
        "commands": {
            "test:unit": "cargo test",
            "build": {
                "command": "echo $FOO && cargo build",
                "env": {"FOO": "bar"}
            }
        }
    });
    let form = parse(&serde_json::to_string(&source).unwrap(), &base_config());
    assert_eq!(
        form.commands
            .iter()
            .map(|command| command.name.text.as_str())
            .collect::<Vec<_>>(),
        ["build", "test:unit"]
    );
    assert!(form.commands[0].object);
    assert!(!form.commands[1].object);
    assert_eq!(saved(&form), source);

    let mut form = parse("{}", &base_config());
    form.commands.push(ProjectCommandForm {
        name: TextField::new("deploy"),
        command: TextField::new("cargo deploy"),
        environment: vec![ProjectEnvironmentForm {
            name: TextField::new("TARGET"),
            value: TextField::new("staging"),
        }],
        object: false,
    });
    assert_eq!(
        saved(&form),
        json!({
            "commands": {
                "deploy": {
                    "command": "cargo deploy",
                    "env": {"TARGET": "staging"}
                }
            }
        })
    );
}

#[test]
fn project_command_rows_reject_invalid_names_and_environments() {
    let mut form = parse("{}", &base_config());
    let command = |name: &str, environment: Vec<ProjectEnvironmentForm>| ProjectCommandForm {
        name: TextField::new(name),
        command: TextField::new("echo ok"),
        environment,
        object: true,
    };
    let env = |name: &str| ProjectEnvironmentForm {
        name: TextField::new(name),
        value: TextField::new("value"),
    };

    for name in ["-build", "has space", "--list"] {
        form.commands = vec![command(name, Vec::new())];
        assert!(form.validate().is_err(), "{name:?} should fail");
    }
    form.commands = vec![command("build", vec![env("ZETTA_TOKEN")])];
    assert!(form.validate().is_err());
    form.commands = vec![command("build", vec![env("1FOO")])];
    assert!(form.validate().is_err());
    form.commands = vec![command("build", vec![env("FOO"), env("foo")])];
    assert!(form.validate().is_err());
    form.commands = vec![command("build", Vec::new())];
    form.validate().unwrap();
}

#[test]
fn a_tab_icon_can_be_turned_off_or_returned_to_inherited() {
    let mut form = parse(r#"{"default_tab_icon": null}"#, &base_config());
    assert_eq!(form.default_tab_icon, ProjectTabIcon::None);
    // An explicit `null` is how a project turns off an icon the user
    // configuration sets, so it has to survive the round trip.
    assert_eq!(saved(&form), json!({ "default_tab_icon": null }));

    form.default_tab_icon = ProjectTabIcon::Inherit;
    assert_eq!(saved(&form), json!({}));
}

#[test]
fn the_working_directory_may_not_escape_the_project() {
    let mut form = parse("{}", &base_config());
    form.working_directory = TextField::new("../elsewhere");
    assert!(form.validate().is_err());

    form.working_directory = TextField::new("/etc");
    assert!(form.validate().is_err());

    form.working_directory = TextField::new("crates/app");
    form.validate().unwrap();
}

#[test]
fn environment_rows_reject_reserved_duplicate_and_malformed_names() {
    let mut form = parse("{}", &base_config());
    let row = |name: &str, value: &str| ProjectEnvironmentForm {
        name: TextField::new(name),
        value: TextField::new(value),
    };

    form.environment = vec![row("ZETTA_SESSION", "spoofed")];
    assert!(form.validate().is_err());

    form.environment = vec![row("NAME=VALUE", "x")];
    assert!(form.validate().is_err());

    form.environment = vec![row("", "x")];
    assert!(form.validate().is_err());

    form.environment = vec![row("RUST_LOG", "debug"), row("rust_log", "trace")];
    assert!(form.validate().is_err());

    form.environment = vec![row("RUST_LOG", "debug")];
    form.validate().unwrap();
}

#[test]
fn a_profile_needs_a_program_unless_it_overrides_an_inherited_one() {
    let mut form = parse("{}", &base_config_with_user_template());
    form.profiles.push(ProjectProfileForm {
        name: TextField::new("Runner"),
        program: TextField::default(),
        arguments: TextField::new("-l"),
        theme: None,
        dark_theme: None,
        icon: None,
        hidden: false,
    });
    assert!(form.validate().is_err());

    // A row without a program is matched against the application profiles by
    // name, so a name none of them answers to would only fail when the file was
    // loaded back.
    form.profiles[0].arguments = TextField::default();
    assert!(form.validate().is_err());

    form.profiles[0].program = TextField::new("/usr/bin/env");
    form.validate().unwrap();

    // Overriding only the theme of an inherited profile is the common case, and
    // needs no program.
    form.profiles[0] = ProjectProfileForm {
        name: TextField::new("Toolbox"),
        program: TextField::default(),
        arguments: TextField::default(),
        theme: Some("One Dark".to_owned()),
        dark_theme: None,
        icon: None,
        hidden: false,
    };
    form.validate().unwrap();
}

#[test]
fn the_user_configurations_templates_are_inherited_and_read_only() {
    let form = parse("{}", &base_config_with_user_template());
    let templates = &form.pane_templates;

    let user_template = templates
        .templates
        .iter()
        .find(|template| template.name.text == "user-pair")
        .expect("a user template is an inherited preset for a project");
    assert!(user_template.inherited());
    assert!(!user_template.editable());
    // The built-in presets stay listed first, ahead of what the user adds.
    assert_eq!(templates.templates[0].name.text, "three-right");
    // Nothing was overridden, so the project file gains no templates.
    assert_eq!(saved(&form), json!({}));
}

#[test]
fn overriding_an_inherited_template_writes_only_the_projects_copy() {
    let source = json!({
        "pane_split_templates": {
            "user-pair": { "layout": { "horizontal": [{ "label": "top" }, { "label": "bottom" }] } }
        }
    });
    let mut form = parse(
        &serde_json::to_string(&source).unwrap(),
        &base_config_with_user_template(),
    );
    let index = form
        .pane_templates
        .templates
        .iter()
        .position(|template| template.name.text == "user-pair")
        .unwrap();
    assert!(form.pane_templates.templates[index].editable());
    assert_eq!(saved(&form), source);

    // Discarding the override restores the inherited layout and drops the
    // project's entry entirely.
    form.pane_templates.select_template(index);
    form.pane_templates.delete_selected(false).unwrap();
    assert!(!form.pane_templates.templates[index].editable());
    assert_eq!(saved(&form), json!({}));
}

#[test]
fn the_initial_split_follows_a_template_renamed_in_the_same_session() {
    let mut form = parse(
        &serde_json::to_string(&json!({
            "initial_split": "development",
            "pane_split_templates": {
                "development": { "layout": { "vertical": [{ "label": "editor" }, { "label": "server" }] } }
            }
        }))
        .unwrap(),
        &base_config(),
    );
    let index = form
        .pane_templates
        .templates
        .iter()
        .position(|template| template.name.text == "development")
        .unwrap();

    form.pane_templates.templates[index].name = TextField::new("staging");

    assert_eq!(form.resolved_initial_split(), Some("staging"));
    assert_eq!(saved(&form)["initial_split"], json!("staging"));
}

#[test]
fn an_initial_split_naming_nothing_is_rejected() {
    let mut form = parse("{}", &base_config());
    form.initial_split = Some("missing".to_owned());

    assert!(form.validate().is_err());

    form.initial_split = Some("three-right".to_owned());
    form.validate().unwrap();
}

#[test]
fn unrecognized_fields_are_reported_rather_than_dropped() {
    let base = base_config();
    // Silently dropping the field would delete it from the file on the next
    // save.
    assert!(
        ProjectForm::parse(
            r#"{"terminal_font_size": 18}"#,
            Path::new("/projects/demo/.zetta/config.json"),
            &base,
        )
        .is_err()
    );
}

#[test]
fn saving_replaces_the_file_only_with_text_the_loader_accepts() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let work = root.join("work");
    fs::create_dir(&work).unwrap();
    let path = ProjectConfig::path_for(root);
    let base = base_config();

    save(root, &base, r#"{"working_directory": "work"}"#).unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "{\"working_directory\": \"work\"}\n"
    );

    // A directory that does not exist is only detectable against the
    // filesystem, so the loader is what has to reject it — and the previous
    // file has to survive.
    assert!(save(root, &base, r#"{"working_directory": "missing"}"#).is_err());
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "{\"working_directory\": \"work\"}\n"
    );
}
