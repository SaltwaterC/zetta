use std::ffi::OsString;

use super::*;
use crate::cli_services::CliServiceCommand;

#[test]
fn serial_console_parser_uses_the_panel_defaults_and_accepts_all_settings() {
    let defaults = parse_serial_args([
        OsString::from("console"),
        OsString::from("--device"),
        OsString::from("/dev/ttyUSB0"),
    ])
    .unwrap();
    assert_eq!(
        defaults,
        CliServiceCommand::Serial(SerialCommand::Connect(SerialConnectionOptions {
            device: "/dev/ttyUSB0".to_owned(),
            baud_rate: 115_200,
            data_bits: 8,
            parity: SerialParity::None,
            stop_bits: 1,
            flow_control: SerialFlowControl::None,
        }))
    );

    let configured = parse_serial_args([
        OsString::from("console"),
        OsString::from("--device"),
        OsString::from("COM3"),
        OsString::from("--baud-rate"),
        OsString::from("9600"),
        OsString::from("--data-bits"),
        OsString::from("7"),
        OsString::from("--parity"),
        OsString::from("even"),
        OsString::from("--stop-bits"),
        OsString::from("2"),
        OsString::from("--flow-control"),
        OsString::from("hardware"),
    ])
    .unwrap();
    assert_eq!(
        configured,
        CliServiceCommand::Serial(SerialCommand::Connect(SerialConnectionOptions {
            device: "COM3".to_owned(),
            baud_rate: 9600,
            data_bits: 7,
            parity: SerialParity::Even,
            stop_bits: 2,
            flow_control: SerialFlowControl::Hardware,
        }))
    );

    let shorthand = parse_serial_args([
        OsString::from("console"),
        OsString::from("-d"),
        OsString::from("COM4"),
        OsString::from("-b"),
        OsString::from("57600"),
        OsString::from("-D"),
        OsString::from("7"),
        OsString::from("-p"),
        OsString::from("odd"),
        OsString::from("-s"),
        OsString::from("2"),
        OsString::from("-f"),
        OsString::from("software"),
    ])
    .unwrap();
    assert_eq!(
        shorthand,
        CliServiceCommand::Serial(SerialCommand::Connect(SerialConnectionOptions {
            device: "COM4".to_owned(),
            baud_rate: 57_600,
            data_bits: 7,
            parity: SerialParity::Odd,
            stop_bits: 2,
            flow_control: SerialFlowControl::Software,
        }))
    );
}

#[test]
fn serial_list_and_invalid_console_options_are_validated() {
    assert_eq!(
        parse_serial_args([OsString::from("list")]).unwrap(),
        CliServiceCommand::Serial(SerialCommand::List)
    );
    assert!(parse_serial_args([OsString::from("console")]).is_err());
    assert!(
        parse_serial_args([
            OsString::from("console"),
            OsString::from("--device"),
            OsString::from("ttyS0"),
            OsString::from("--data-bits"),
            OsString::from("9"),
        ])
        .is_err()
    );
}
