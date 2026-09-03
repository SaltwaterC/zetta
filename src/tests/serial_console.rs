use super::*;

#[test]
fn defaults_to_115200_8n1_without_flow_control() {
    let prompt = SerialConsolePrompt::default();
    assert_eq!(prompt.baud_rate.text, "115200");
    assert_eq!(prompt.framing_label(), "8N1");
    assert_eq!(prompt.flow_control, serialport::FlowControl::None);
}

#[test]
fn serial_fields_wrap_in_both_directions() {
    assert_eq!(SerialField::Device.adjacent(true), SerialField::FlowControl);
    assert_eq!(
        SerialField::FlowControl.adjacent(false),
        SerialField::Device
    );
}

#[test]
fn serial_value_cycle_wraps() {
    let mut prompt = SerialConsolePrompt {
        field: SerialField::DataBits,
        ..Default::default()
    };
    prompt.cycle_current_value(false);
    assert_eq!(prompt.data_bits, serialport::DataBits::Five);
    prompt.cycle_current_value(true);
    assert_eq!(prompt.data_bits, serialport::DataBits::Eight);
}

#[test]
fn baud_rate_arrows_cycle_common_values_and_keep_custom_entry_available() {
    let mut prompt = SerialConsolePrompt {
        field: SerialField::BaudRate,
        ..Default::default()
    };
    prompt.cycle_current_value(false);
    assert_eq!(prompt.baud_rate.text, "230400");
    prompt.baud_rate = TextField::new("100000");
    prompt.cycle_current_value(true);
    assert_eq!(prompt.baud_rate.text, "57600");
}

#[test]
fn baud_rate_selection_defaults_to_inactive() {
    let prompt = SerialConsolePrompt::default();
    assert!(!prompt.baud_rate.select_all);
}

#[cfg(target_os = "linux")]
#[test]
fn legacy_linux_serial_names_are_identified_for_validation() {
    assert!(linux_legacy_serial_port("/dev/ttyS0"));
    assert!(linux_legacy_serial_port("/dev/ttyS31"));
    assert!(!linux_legacy_serial_port("/dev/ttyUSB0"));
    assert!(!linux_legacy_serial_port("/dev/ttyACM0"));
    assert!(!linux_legacy_serial_port("/dev/rfcomm0"));
}

#[cfg(target_os = "linux")]
#[test]
fn nonexistent_and_non_device_linux_entries_are_filtered() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing");
    let regular_file = directory.path().join("ttyS0");
    fs::write(&regular_file, b"not a tty").unwrap();
    let ports = vec![missing, regular_file]
        .into_iter()
        .map(|path| serialport::SerialPortInfo {
            port_name: path.to_string_lossy().into_owned(),
            port_type: serialport::SerialPortType::Unknown,
        })
        .collect();

    assert!(detected_serial_devices(ports).is_empty());
}

#[cfg(target_os = "linux")]
#[test]
fn linux_legacy_probe_rejects_files_without_tty_attributes() {
    let file = tempfile::NamedTempFile::new().unwrap();
    assert!(!linux_tty_responds(file.path()));
}

#[cfg(target_os = "linux")]
#[test]
fn generic_serial8250_slots_are_identified_as_possible_phantom_uarts() {
    assert!(linux_generic_serial8250_device_path(Path::new(
        "/sys/devices/platform/serial8250/serial8250:0/serial8250:0.15"
    )));
    assert!(!linux_generic_serial8250_device_path(Path::new(
        "/sys/devices/pci0000:00/0000:00:16.3/0000:00:16.3:0/tty/ttyS4"
    )));
    assert!(!linux_generic_serial8250_device_path(Path::new(
        "/sys/devices/pnp0/00:04/tty/ttyS0"
    )));
}
