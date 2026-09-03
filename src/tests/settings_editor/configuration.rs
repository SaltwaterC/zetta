use super::*;
use crate::settings_editor::tests::settings_test_path;

#[cfg(any(target_os = "macos", target_os = "linux"))]
use crate::config::Profile;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use task::Shell;

#[test]
fn configuration_form_round_trip_uses_typed_values_and_profiles() {
    let root = std::env::temp_dir().join(format!(
        "zetta-configuration-form-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(
        &root,
        r#"{
            "default_profile": "System",
            "terminal_font_size": 13,
            "dark_theme": "Solarized Dark",
            "profiles": [{
                "name": "Login shell",
                "program": "/bin/sh",
                "args": ["-l"],
                "theme": "One Dark",
                "dark_theme": "Solarized Dark",
                "icon": "fish"
            }]
        }"#,
    )
    .unwrap();
    let config = Config::load(Some(&root), None).unwrap();
    let mut form = ConfigurationForm::load(&root, &config).unwrap();
    form.terminal_font_size.text = "16".to_owned();
    form.default_tab_icon = Some(IconName::Folder);
    form.max_scroll_history_lines.text = "123456789".to_owned();
    form.inactive_pane_opacity = 0.65;
    form.compact_mode = true;
    form.hide_pane_size = false;
    form.hide_title_bar_labels = true;
    form.hide_title_bar_buttons = true;
    #[cfg(target_os = "macos")]
    {
        form.hide_title_bar_menus = false;
    }
    form.pane_controls_position = PaneControlsPosition::Left;
    form.pane_controls_hidden_by_default = true;
    form.working_directory_scope = WorkingDirectoryScope::Pane;
    form.new_tab_profile = NewTabProfile::Inherit;
    #[cfg(feature = "http-server")]
    {
        form.http_server_port.text = "8080".to_owned();
    }
    #[cfg(feature = "tftp-server")]
    {
        form.tftp_server_port.text = "1069".to_owned();
    }
    form.profiles
        .iter_mut()
        .find(|profile| !profile.detected)
        .unwrap()
        .arguments
        .text = "-l, -i".to_owned();

    let text = form.to_json().unwrap();
    let output: Value = serde_json::from_str(&text).unwrap();
    Config::parse(&text, Some(&root), None).unwrap();
    fs::remove_file(root).unwrap();

    assert_eq!(output["terminal_font_size"], 16.);
    assert_eq!(output["dark_theme"], "Solarized Dark");
    assert_eq!(output["default_tab_icon"], "folder");
    assert_eq!(output["max_scroll_history_lines"], 123_456_789);
    assert_eq!(output["inactive_pane_opacity"], 0.65);
    assert_eq!(output["compact_mode"], true);
    assert_eq!(output["hide_pane_size"], false);
    assert_eq!(output["hide_title_bar_labels"], true);
    assert_eq!(output["hide_title_bar_buttons"], true);
    #[cfg(target_os = "macos")]
    assert_eq!(output["hide_title_bar_menus"], false);
    assert_eq!(output["pane_controls_position"], "left");
    assert_eq!(output["pane_controls_hidden_by_default"], true);
    assert_eq!(output["working_directory_scope"], "pane");
    assert_eq!(output["new_tab_profile"], "inherit");
    #[cfg(feature = "http-server")]
    assert_eq!(output["http_server_port"], 8080);
    #[cfg(feature = "tftp-server")]
    assert_eq!(output["tftp_server_port"], 1069);
    assert_eq!(output["profiles"][0]["args"], json!(["-l", "-i"]));
    let login_profile = output["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|profile| profile["name"] == "Login shell")
        .unwrap();
    assert_eq!(login_profile["icon"], "fish");
    assert_eq!(login_profile["dark_theme"], "Solarized Dark");
}

#[test]
fn configuration_form_round_trip_preserves_custom_pane_template_trees() {
    let root = std::env::temp_dir().join(format!(
        "zetta-pane-template-form-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let layout = json!({
        "vertical": [
            {
                "label": "server",
                "profile": "System",
                "env": { "ROLE": "server" },
                "overlay": { "text": "SERVER", "size": "xl", "opacity": 85, "color": "cyan" }
            },
            { "command": { "program": "ssh", "args": ["host"] } }
        ]
    });
    let template = json!({
        "env": { "PROJECT": "zetta", "ROLE": "default" },
        "layout": layout
    });
    fs::write(
        &root,
        serde_json::to_string(&json!({
            "pane_split_templates": { "custom": template.clone() }
        }))
        .unwrap(),
    )
    .unwrap();
    let config = Config::load(Some(&root), None).unwrap();
    let form = ConfigurationForm::load(&root, &config).unwrap();
    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    fs::remove_file(root).unwrap();

    assert_eq!(output["pane_split_templates"]["custom"], template);
}

#[test]
fn configuration_form_omits_automatic_profile_icons() {
    let root = std::env::temp_dir().join(format!(
        "zetta-automatic-profile-icon-form-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(
        &root,
        r#"{"profiles":[{"name":"Automatic fish","program":"/opt/bin/fish"}]}"#,
    )
    .unwrap();
    let config = Config::load(Some(&root), None).unwrap();
    let form = ConfigurationForm::load(&root, &config).unwrap();
    let automatic = form
        .profiles
        .iter()
        .find(|profile| profile.name.text == "Automatic fish")
        .unwrap();
    assert_eq!(automatic.icon, None);
    assert_eq!(automatic.automatic_icon, ProfileIcon::Fish);
    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    let serialized = output["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|profile| profile["name"] == "Automatic fish")
        .unwrap();
    assert!(serialized.get("icon").is_none());
    fs::remove_file(root).unwrap();
}

#[test]
fn configuration_form_round_trip_preserves_hidden_detected_profiles() {
    let root = std::env::temp_dir().join(format!(
        "zetta-hidden-profile-form-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(&root, r#"{"profiles":[{"name":"System","hidden":true}]}"#).unwrap();
    let config = Config::load(Some(&root), None).unwrap();
    let form = ConfigurationForm::load(&root, &config).unwrap();
    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    fs::remove_file(root).unwrap();

    assert_eq!(
        output["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|profile| profile["name"] == "System")
            .and_then(|profile| profile.get("hidden")),
        Some(&json!(true))
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn non_macos_configuration_preserves_macos_title_bar_setting() {
    let root = std::env::temp_dir().join(format!(
        "zetta-macos-title-bar-setting-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(&root, r#"{"hide_title_bar_menus":true}"#).unwrap();
    let config = Config::load(Some(&root), None).unwrap();
    let form = ConfigurationForm::load(&root, &config).unwrap();
    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    fs::remove_file(root).unwrap();

    assert_eq!(output["hide_title_bar_menus"], true);
}

#[test]
fn max_scrollback_is_displayed_symbolically_but_serialized_numerically() {
    let config = Config::defaults(None, None);
    let missing = std::env::temp_dir().join(format!(
        "zetta-max-scrollback-form-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let form = ConfigurationForm::load(&missing, &config).unwrap();
    assert_eq!(form.max_scroll_history_lines.text, "Max");
    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    // "Max" is the built-in default, so it's omitted from the saved file rather
    // than being pinned as an explicit override.
    assert!(
        !output
            .as_object()
            .unwrap()
            .contains_key("max_scroll_history_lines")
    );
}

#[test]
fn detected_profile_theme_overrides_are_the_only_detected_profiles_serialized() {
    let root = std::env::temp_dir().join(format!(
        "zetta-detected-profile-form-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(
        &root,
        r#"{"profiles":[{"name":"System","theme":"One Dark"}]}"#,
    )
    .unwrap();
    let config = Config::load(Some(&root), None).unwrap();
    let mut form = ConfigurationForm::load(&root, &config).unwrap();
    let system_index = form
        .profiles
        .iter()
        .position(|profile| profile.name.text == "System")
        .unwrap();
    assert!(form.profiles[system_index].detected);
    assert_eq!(
        form.profiles[system_index].theme.as_deref(),
        Some("One Dark")
    );

    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    assert_eq!(
        output["profiles"],
        json!([{"name": "System", "theme": "One Dark"}])
    );

    form.profiles[system_index].theme = None;
    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    fs::remove_file(root).unwrap();
    assert_eq!(output["profiles"], json!([]));
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn configuration_form_round_trip_serializes_the_resolved_homebrew_profile_name() {
    let root = std::env::temp_dir().join(format!(
        "zetta-homebrew-profile-form-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(
        &root,
        r#"{
            "default_profile": "fish (homebrew)",
            "profiles": [{"name":"fish (homebrew)","theme":"One Dark"}]
        }"#,
    )
    .unwrap();

    let mut config = Config::defaults(Some(&root), None);
    config.profiles = vec![
        Profile {
            name: "System".to_owned(),
            command: Shell::System,
            theme: None,
            dark_theme: None,
            icon: ProfileIcon::Zetta,
        },
        Profile {
            name: "Fish (Homebrew)".to_owned(),
            command: Shell::Program("/opt/homebrew/bin/fish".to_owned()),
            theme: None,
            dark_theme: None,
            icon: ProfileIcon::Fish,
        },
    ];
    config.default_profile = 1;

    let form = ConfigurationForm::load(&root, &config).unwrap();
    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    fs::remove_file(root).unwrap();

    assert_eq!(output["default_profile"], "Fish (Homebrew)");
    assert_eq!(
        output["profiles"],
        json!([{"name":"Fish (Homebrew)","theme":"One Dark"}])
    );
}

#[test]
fn configuration_defaults_round_trip_produces_minimal_output() {
    let config = Config::defaults(None, None);
    let missing = std::env::temp_dir().join(format!(
        "zetta-default-config-form-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let form = ConfigurationForm::load(&missing, &config).unwrap();
    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    let object = output.as_object().unwrap();
    for key in object.keys() {
        // `terminal_font_size` has no fixed default (it falls back to a
        // theme-dependent size at runtime), so it's intentionally always
        // written rather than filtered — see the design notes in to_json.
        assert!(
            matches!(key.as_str(), "profiles" | "terminal_font_size"),
            "unexpected default-valued key {key:?}"
        );
    }
    if let Some(profiles) = object.get("profiles") {
        assert_eq!(profiles, &json!([]));
    }
}

/// The automatic-protection toggle appears only once there is something to seal
/// a session key to and an effective identity to open it with. The conventional
/// SSH identity counts when the identity field is blank.
#[cfg(feature = "session-persistence")]
#[test]
fn the_automatic_protection_toggle_is_offered_only_with_a_recipient_and_an_identity() {
    let root = settings_test_path("zetta-auto-protect-offered");
    let config = Config::defaults(Some(&root), None);
    let mut form = ConfigurationForm::load(&root, &config).unwrap();
    assert!(!form.session_auto_protect_is_offered());

    form.session_persistence_recipients = TextField::new("age1example".to_owned());
    assert_eq!(
        form.session_auto_protect_is_offered(),
        crate::config::default_session_identity_path().is_some()
    );

    form.session_persistence_identity = TextField::new("   ".to_owned());
    assert_eq!(
        form.session_auto_protect_is_offered(),
        crate::config::default_session_identity_path().is_some()
    );

    form.session_persistence_identity = TextField::new("~/keys/zetta.txt".to_owned());
    assert!(form.session_auto_protect_is_offered());

    form.session_persistence_recipients = TextField::new(String::new());
    assert!(!form.session_auto_protect_is_offered());
}

#[cfg(feature = "session-persistence")]
#[test]
fn automatic_protection_round_trips_through_the_configuration_form() {
    let root = settings_test_path("zetta-auto-protect-round-trip");
    fs::write(
        &root,
        serde_json::to_string(&json!({
            "sessions": {
                "persistence": {
                    "recipients": ["age1example"],
                    "identity": "~/keys/zetta.txt",
                    "auto_protect": true,
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let config = Config::load(Some(&root), None).unwrap();
    let mut form = ConfigurationForm::load(&root, &config).unwrap();
    assert!(form.session_persistence_auto_protect);

    form.session_persistence_auto_protect = false;
    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    fs::remove_file(root).unwrap();

    assert_eq!(
        output["sessions"]["persistence"]["auto_protect"],
        json!(false)
    );
    // Written back beside what it depends on, so a reload cannot end up with the
    // flag set and nothing to seal to.
    assert_eq!(
        output["sessions"]["persistence"]["recipients"],
        json!(["age1example"])
    );
}
