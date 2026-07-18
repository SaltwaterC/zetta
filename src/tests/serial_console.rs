use super::*;

#[test]
fn defaults_to_115200_8n1_without_flow_control() {
    let prompt = SerialConsolePrompt::default();
    assert_eq!(prompt.baud_rate, "115200");
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
    assert_eq!(prompt.baud_rate, "230400");
    prompt.baud_rate = "100000".to_owned();
    prompt.cycle_current_value(true);
    assert_eq!(prompt.baud_rate, "57600");
}
