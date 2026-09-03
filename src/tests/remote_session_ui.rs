use super::*;

#[test]
fn remote_picker_starts_on_the_target_field() {
    let picker = RemoteSessionPicker::default();

    assert_eq!(picker.field, RemoteSessionField::Target);
    assert!(picker.target.text.is_empty());
    assert!(picker.port.text.is_empty());
    assert!(picker.sessions.is_empty());
    assert!(!picker.loading);
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
