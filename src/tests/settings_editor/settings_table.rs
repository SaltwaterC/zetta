use super::*;
use crate::settings_editor::tests::settings_test_path;

/// A form loaded from a file holding `contents`.
fn form_from(prefix: &str, contents: &str) -> ConfigurationForm {
    let path = settings_test_path(prefix);
    fs::write(&path, contents).unwrap();
    let config = Config::load(Some(&path), None).unwrap();
    let form = ConfigurationForm::load(&path, &config).unwrap();
    fs::remove_file(&path).ok();
    form
}

fn saved(form: &ConfigurationForm) -> Map<String, Value> {
    match serde_json::from_str(&form.to_json().unwrap()).unwrap() {
        Value::Object(root) => root,
        other => panic!("saved a non-object: {other}"),
    }
}

fn number(setting: ConfigSetting) -> NumberSpec {
    match setting.spec().kind {
        SettingKind::Number(spec) => spec,
        _ => panic!("{setting:?} is not a number"),
    }
}

#[test]
fn every_setting_has_a_key_of_its_own() {
    let mut keys: Vec<_> = ALL_SETTINGS
        .iter()
        .map(|setting| setting.spec().key)
        .collect();
    let count = keys.len();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), count, "two settings write the same key");
}

/// A default that its own setting would refuse could never be saved back.
#[test]
fn every_default_is_a_value_its_setting_accepts() {
    for &setting in ALL_SETTINGS {
        match setting.spec().kind {
            SettingKind::Choice(spec) => assert!(
                spec.options.iter().any(|(value, _)| *value == spec.default),
                "{setting:?} defaults to an option it does not offer"
            ),
            SettingKind::Number(spec) => {
                if let Some(default) = spec.default {
                    assert_eq!(
                        spec.read(&spec.format(default)),
                        Ok(Some(default)),
                        "{setting:?}"
                    );
                }
                if let Start::Value(start) = spec.start {
                    assert!((spec.min..=spec.max).contains(&start), "{setting:?}");
                }
            }
            _ => {}
        }
    }
}

/// An untouched form saves none of the scalar settings: each is at the value
/// leaving it out already means, so the file stays the user's own.
#[test]
fn a_form_left_at_its_defaults_writes_none_of_them() {
    let form = form_from("zetta-table-defaults", "{}");
    let root = saved(&form);
    for &setting in ALL_SETTINGS {
        assert_eq!(
            get_path(&root, setting.spec().key),
            None,
            "{setting:?} was written"
        );
    }
    // Including the objects nested settings live in.
    assert!(!root.contains_key("sessions"), "{root:?}");
}

#[test]
fn values_off_their_defaults_are_written_where_the_file_keeps_them_and_load_back() {
    let mut form = form_from("zetta-table-written", "{}");
    ConfigSetting::CompactMode.set_switch_shown(&mut form, true);
    ConfigSetting::ShowPaneSize.set_switch_shown(&mut form, true);
    ConfigSetting::MouseClipboard.set_switch_shown(&mut form, false);
    ConfigSetting::SessionRetention.set_choice(&mut form, 0);
    ConfigSetting::RemoteProtocol.set_choice(&mut form, 1);
    form.max_scroll_history_lines.text = "5000".to_owned();
    form.remote_session_keep_alive.text = "250".to_owned();
    form.working_directory.text = "/srv".to_owned();

    let root = saved(&form);
    let at = |key: &[&str]| get_path(&root, key).cloned();
    assert_eq!(at(&["compact_mode"]), Some(json!(true)));
    // A "Show" switch over a `hide_` key writes the opposite of what it shows.
    assert_eq!(at(&["hide_pane_size"]), Some(json!(false)));
    assert_eq!(at(&["mouse_clipboard"]), Some(json!(false)));
    assert_eq!(at(&["sessions", "retention"]), Some(json!("none")));
    assert_eq!(at(&["sessions", "remote", "protocol"]), Some(json!("zosh")));
    assert_eq!(
        at(&["sessions", "remote", "keep_alive_ms"]),
        Some(json!(250))
    );
    assert_eq!(at(&["max_scroll_history_lines"]), Some(json!(5000)));
    assert_eq!(at(&["working_directory"]), Some(json!("/srv")));

    let reloaded = form_from(
        "zetta-table-reloaded",
        &serde_json::to_string(&Value::Object(root)).unwrap(),
    );
    for &setting in ALL_SETTINGS {
        assert_eq!(
            setting.encode(&reloaded),
            setting.encode(&form),
            "{setting:?} did not survive a save and a load"
        );
    }
}

/// Putting the last key of `sessions` back to its default removes the object,
/// but not while it still holds a key the table does not write: the form
/// writes into the file it read, and keeps what it does not own.
#[test]
fn a_nested_object_is_pruned_only_once_nothing_is_left_in_it() {
    let mut form = form_from("zetta-table-prune", r#"{"sessions":{"retention":"none"}}"#);
    ConfigSetting::SessionRetention.set_choice(&mut form, 1);
    assert!(!saved(&form).contains_key("sessions"));

    let Value::Object(mut root) = json!({"sessions": {"remote": {"protocol": "zosh"}, "kept": 1}})
    else {
        unreachable!()
    };
    remove_path(&mut root, &["sessions", "remote", "protocol"]);
    assert_eq!(Value::Object(root), json!({"sessions": {"kept": 1}}));
}

#[test]
fn a_number_out_of_range_is_named_with_its_range_and_unit() {
    let mut form = form_from("zetta-table-invalid", "{}");
    form.session_ring_bytes.text = "12".to_owned();
    let message = form.check(ConfigSetting::SessionRingBytes).unwrap();
    assert!(
        message.starts_with("Retained screen size must be a whole number of bytes from 4096"),
        "{message}"
    );
    // And a save fails on the field, so it can take the keyboard there.
    let error = form.to_json().unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<InvalidField>()
            .map(|invalid| invalid.field),
        Some(ConfigTextField::Setting(ConfigSetting::SessionRingBytes))
    );

    form.session_ring_bytes.text = "8192.5".to_owned();
    assert!(
        form.check(ConfigSetting::SessionRingBytes).is_some(),
        "not a whole number"
    );
    form.session_ring_bytes.text = String::new();
    assert!(
        form.check(ConfigSetting::SessionRingBytes).is_some(),
        "empty is not a size"
    );
}

/// What an empty field means differs by setting, and each says so.
#[test]
fn an_empty_number_field_means_what_its_setting_says() {
    let mut form = form_from("zetta-table-empty", r#"{"terminal_font_size":15}"#);
    form.terminal_font_size.text = String::new();
    form.remote_session_keep_alive.text = String::new();
    assert_eq!(form.check(ConfigSetting::FontSize), None);
    assert_eq!(form.check(ConfigSetting::RemoteKeepAlive), None);
    let root = saved(&form);
    // Unset: the theme's size applies again.
    assert!(!root.contains_key("terminal_font_size"));
    // Null, which is also what leaving the key out means, so it is left out.
    assert!(!root.contains_key("sessions"), "{root:?}");
}

#[test]
fn the_scrollback_sentinel_reads_as_the_maximum_in_any_case() {
    let spec = number(ConfigSetting::ScrollHistory);
    let maximum = terminal::MAX_SCROLL_HISTORY_LINES as f64;
    for spelling in ["Max", "max", " MAX "] {
        assert_eq!(spec.read(spelling), Ok(Some(maximum)), "{spelling:?}");
    }
    assert_eq!(spec.format(maximum), "Max");
    assert!(spec.message().ends_with(", or Max"), "{}", spec.message());

    // The maximum is the default, so it is not written.
    let mut form = form_from("zetta-table-max", r#"{"max_scroll_history_lines":10}"#);
    form.max_scroll_history_lines.text = "max".to_owned();
    assert!(!saved(&form).contains_key("max_scroll_history_lines"));
}

#[test]
fn stepping_an_empty_field_starts_where_its_setting_says() {
    let mut form = form_from("zetta-table-start", "{}");
    form.terminal_font_size.text = String::new();
    ConfigSetting::FontSize.step_number(&mut form, 1, 13.);
    assert_eq!(form.terminal_font_size.text, "14");

    // Mosh's heartbeat is empty; stepping up starts at the suggested interval.
    form.remote_session_keep_alive.text = String::new();
    ConfigSetting::RemoteKeepAlive.step_number(&mut form, 1, 13.);
    assert_eq!(
        form.remote_session_keep_alive.text,
        (crate::config::REMOTE_KEEP_ALIVE_DEFAULT_MS + 10).to_string()
    );
}

#[test]
fn stepping_a_number_typed_out_of_range_lands_on_the_end_it_is_beyond() {
    let mut form = form_from("zetta-table-clamp", "{}");
    form.terminal_font_size.text = "200".to_owned();
    ConfigSetting::FontSize.step_number(&mut form, -1, 13.);
    assert_eq!(
        form.terminal_font_size.text,
        crate::config::MAX_TERMINAL_FONT_SIZE.to_string()
    );

    form.max_scroll_history_lines.text = "Max".to_owned();
    ConfigSetting::ScrollHistory.step_number(&mut form, 1, 13.);
    assert_eq!(form.max_scroll_history_lines.text, "Max");
}

#[test]
fn scroll_history_steps_cover_the_full_range_without_jumping_to_max() {
    let maximum = i32::MAX as u64;
    assert_eq!(adjusted_scroll_history(100_000, 1, maximum), 200_000);
    assert_eq!(adjusted_scroll_history(100_000, -1, maximum), 99_000);
    assert_eq!(
        adjusted_scroll_history(maximum, -1, maximum),
        maximum - 100_000_000
    );
    assert_eq!(adjusted_scroll_history(maximum - 1, 1, maximum), maximum);
}

#[test]
fn a_choice_is_set_by_its_position_and_shown_by_its_label() {
    let mut form = form_from("zetta-table-choice", "{}");
    assert_eq!(
        ConfigSetting::WorkingDirectoryScope.choice_label(&form),
        Some("Tab")
    );
    ConfigSetting::WorkingDirectoryScope.set_choice(&mut form, 1);
    assert_eq!(
        ConfigSetting::WorkingDirectoryScope.choice_label(&form),
        Some("Pane")
    );
    // A position past the options changes nothing.
    ConfigSetting::WorkingDirectoryScope.set_choice(&mut form, 9);
    assert_eq!(
        ConfigSetting::WorkingDirectoryScope.choice_label(&form),
        Some("Pane")
    );
}

#[cfg(feature = "session-persistence")]
#[test]
fn a_comma_list_is_written_as_a_list_and_read_back_as_one_field() {
    let spec = TextSpec {
        text: text_access!(working_directory),
        shape: TextShape::CommaList,
    };
    assert_eq!(spec.encode(" a, ,b ,"), json!(["a", "b"]));
    assert_eq!(spec.decode(Some(&json!(["a", "b"]))), "a, b");
    assert_eq!(spec.decode(None), "");
}

#[test]
fn a_working_directory_of_home_spelled_with_a_slash_is_still_the_default() {
    let mut form = form_from("zetta-table-home", r#"{"working_directory":"/srv"}"#);
    form.working_directory.text = "~/".to_owned();
    assert!(!saved(&form).contains_key("working_directory"));
}
