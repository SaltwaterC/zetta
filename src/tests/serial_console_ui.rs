use super::*;

fn prompt_with_device(port_name: &str) -> SerialConsolePrompt {
    SerialConsolePrompt {
        devices: vec![SerialDevice {
            port_name: port_name.to_owned(),
            description: None,
        }],
        loading: false,
        ..Default::default()
    }
}

#[test]
fn a_prompt_with_a_device_and_a_baud_rate_describes_a_connection() {
    let prompt = prompt_with_device("/dev/ttyUSB0");
    let settings = serial_settings_from_prompt(&prompt).unwrap();
    assert_eq!(settings.port_name, "/dev/ttyUSB0");
    assert_eq!(
        settings.baud_rate, 115_200,
        "the prompt opens on the default rate, which has to be usable as-is"
    );
    assert_eq!(settings.data_bits, prompt.data_bits);
    assert_eq!(settings.stop_bits, prompt.stop_bits);
    assert_eq!(settings.parity, prompt.parity);
    assert_eq!(settings.flow_control, prompt.flow_control);
}

/// The device list is empty until the scan finishes, and can stay empty when
/// nothing is plugged in; submitting then has no port to open.
#[test]
fn submitting_without_a_device_reports_it_rather_than_opening_nothing() {
    let prompt = SerialConsolePrompt::default();
    assert_eq!(
        serial_settings_from_prompt(&prompt).err(),
        Some("No serial device is selected".to_owned())
    );
}

/// The selection survives a rescan that shortens the list, so it can point past
/// the end.
#[test]
fn a_selection_past_the_end_of_the_device_list_is_reported() {
    let mut prompt = prompt_with_device("/dev/ttyUSB0");
    prompt.selected_device = 4;
    assert!(serial_settings_from_prompt(&prompt).is_err());
}

#[test]
fn a_baud_rate_that_is_not_a_positive_whole_number_is_rejected() {
    let expected = Some("Baud rate must be a positive whole number".to_owned());
    for text in ["", "0", "-9600", "9600.5", "9k6", "  "] {
        let mut prompt = prompt_with_device("/dev/ttyUSB0");
        prompt.baud_rate = TextField::new(text);
        assert_eq!(
            serial_settings_from_prompt(&prompt).err(),
            expected,
            "{text:?} should not be accepted as a baud rate"
        );
    }
}

/// The baud-rate field is the only one typed into, and it is a number: anything
/// that is not a digit would only be rejected later, by a message that cannot
/// say which keystroke caused it.
#[test]
fn the_baud_rate_field_takes_digits_and_nothing_else() {
    for digit in ["0", "1", "9"] {
        assert!(baud_rate_accepts(digit));
    }
    for key in ["a", "-", ".", " ", "escape", "enter", "f1", "", "12"] {
        assert!(
            !baud_rate_accepts(key),
            "{key:?} should not be typed into the baud rate"
        );
    }
}

/// Tab walks the rows in a cycle, so every field is reachable and shift-Tab
/// undoes a Tab rather than following its own order.
#[test]
fn tabbing_through_the_prompt_reaches_every_field_and_reverses() {
    let start = SerialField::Device;
    let mut field = start;
    let mut visited = vec![field];
    loop {
        field = field.adjacent(false);
        if field == start {
            break;
        }
        assert!(
            !visited.contains(&field),
            "{field:?} is reached twice before the walk closes"
        );
        visited.push(field);
        assert!(visited.len() < 16, "the Tab walk does not return to Device");
    }
    assert!(
        visited.contains(&SerialField::BaudRate),
        "the only typed field has to be reachable by keyboard"
    );
    for field in visited {
        assert_eq!(field.adjacent(false).adjacent(true), field);
    }
}
