use super::*;

fn picker_with_suggestions(target: &str) -> RemoteSessionPicker {
    RemoteSessionPicker {
        target: TextField::new(target),
        suggestions: vec![
            "production".to_owned(),
            "prod-west".to_owned(),
            "prod-staging".to_owned(),
            "staging".to_owned(),
        ],
        ..Default::default()
    }
}

#[test]
fn remote_picker_starts_on_the_target_field() {
    let picker = RemoteSessionPicker::default();

    assert_eq!(picker.field, RemoteSessionField::Target);
    assert!(picker.target.text.is_empty());
    assert!(picker.port.text.is_empty());
    assert!(picker.sessions.is_empty());
    assert!(!picker.loading);
    assert!(picker.suggestion_navigation.is_none());
}

#[test]
fn down_selects_the_first_host_without_leaving_the_target_field() {
    let mut picker = picker_with_suggestions("");

    assert!(picker.navigate_suggestions(false));
    assert_eq!(picker.field, RemoteSessionField::Target);
    assert_eq!(picker.target.text, "production");
    assert_eq!(picker.suggestion_navigation.as_ref().unwrap().selected, 0);
    assert!(!picker.visible_suggestions().is_empty());
}

#[test]
fn suggestion_navigation_wraps_through_matching_hosts() {
    let mut picker = picker_with_suggestions("prod");

    picker.navigate_suggestions(false);
    assert_eq!(picker.target.text, "production");
    picker.navigate_suggestions(false);
    assert_eq!(picker.target.text, "prod-west");
    picker.navigate_suggestions(true);
    assert_eq!(picker.target.text, "production");
    picker.navigate_suggestions(true);
    assert_eq!(picker.target.text, "prod-staging");
}

#[test]
fn suggestion_navigation_preserves_the_original_filter() {
    let mut picker = picker_with_suggestions("prod");

    picker.navigate_suggestions(false);

    assert_eq!(picker.target.text, "production");
    assert_eq!(
        picker.suggestion_navigation.as_ref().unwrap().filter,
        "prod"
    );
    assert_eq!(
        picker.visible_suggestions(),
        vec!["production", "prod-west", "prod-staging"]
    );
}

#[test]
fn editing_the_target_resets_suggestion_navigation() {
    let mut picker = picker_with_suggestions("prod");
    picker.navigate_suggestions(false);

    picker.target = TextField::new("productionx");
    picker.reset_suggestion_navigation();

    assert!(picker.suggestion_navigation.is_none());
    assert!(picker.visible_suggestions().is_empty());
}

#[test]
fn target_navigation_keeps_the_current_value_for_enter_to_load() {
    let mut picker = picker_with_suggestions("prod");
    picker.navigate_suggestions(false);

    let target = Zetta::remote_target_from_picker(&picker).unwrap();

    assert_eq!(picker.field, RemoteSessionField::Target);
    assert_eq!(target.destination(), "production");
}

#[test]
fn remote_picker_parses_optional_ports_and_rejects_invalid_values() {
    let mut picker = RemoteSessionPicker {
        target: TextField::new("dev.example"),
        ..Default::default()
    };

    let target = Zetta::remote_target_from_picker(&picker).unwrap();
    assert_eq!(target.destination(), "dev.example");
    assert_eq!(target.port(), None);

    picker.port = TextField::new("2200");
    assert_eq!(
        Zetta::remote_target_from_picker(&picker).unwrap().port(),
        Some(2200)
    );

    picker.port = TextField::new("0");
    assert!(Zetta::remote_target_from_picker(&picker).is_err());
    picker.port = TextField::new("not-a-port");
    assert!(Zetta::remote_target_from_picker(&picker).is_err());
}
