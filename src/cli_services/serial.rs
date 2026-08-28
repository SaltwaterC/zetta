use std::ffi::OsString;
use std::io::{self, Read as _, Write as _};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use anyhow::{Context as _, Result};

use crate::startup::format_help_table;

use super::CliServiceCommand;
use super::raw_terminal::RawTerminal;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SerialCommand {
    List,
    Connect(SerialConnectionOptions),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SerialConnectionOptions {
    pub(crate) device: String,
    pub(crate) baud_rate: u32,
    pub(crate) data_bits: u8,
    pub(crate) parity: SerialParity,
    pub(crate) stop_bits: u8,
    pub(crate) flow_control: SerialFlowControl,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SerialParity {
    None,
    Odd,
    Even,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SerialFlowControl {
    None,
    Software,
    Hardware,
}

pub(crate) fn serial_help() -> String {
    format!(
        "Zetta serial console\n\nUsage:\n  zetta serial list\n  zetta serial console [OPTIONS]\n\nCommands:\n{}\n\nConsole options:\n{}\n\nThe serial console uses the terminal's raw input mode. Ctrl-C is sent to the device; press Ctrl-] to disconnect locally.",
        format_help_table([
            ("list", "List currently available serial devices"),
            ("console", "Connect the current terminal to a serial device",),
        ]),
        format_help_table([
            ("-d, --device PATH", "Serial device to open (required)"),
            ("-b, --baud-rate RATE", "Baud rate (default: 115200)"),
            ("-D, --data-bits BITS", "5, 6, 7, or 8 (default: 8)"),
            ("-p, --parity MODE", "none, odd, or even (default: none)"),
            ("-s, --stop-bits BITS", "1 or 2 (default: 1)"),
            (
                "-f, --flow-control MODE",
                "none, software, or hardware (default: none)",
            ),
            ("-h, --help", "Print help"),
        ])
    )
}

pub(crate) fn parse_serial_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<CliServiceCommand> {
    let args = args.into_iter().collect::<Vec<_>>();
    if args
        .iter()
        .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
    {
        anyhow::bail!("{}", serial_help());
    }
    let Some(operation) = args.first() else {
        anyhow::bail!("usage: zetta serial <list|console>; run `zetta serial --help` for details");
    };
    match operation.to_string_lossy().as_ref() {
        "list" => {
            anyhow::ensure!(args.len() == 1, "usage: zetta serial list");
            Ok(CliServiceCommand::Serial(SerialCommand::List))
        }
        "console" => parse_serial_console_args(&args[1..]),
        unknown => anyhow::bail!("unknown serial command {unknown:?}; expected list or console"),
    }
}

fn parse_serial_console_args(args: &[OsString]) -> Result<CliServiceCommand> {
    let mut device = None;
    let mut baud_rate = 115_200;
    let mut data_bits = 8;
    let mut parity = SerialParity::None;
    let mut stop_bits = 1;
    let mut flow_control = SerialFlowControl::None;
    let mut arguments = args.iter();
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            "--device" | "-d" => {
                anyhow::ensure!(device.is_none(), "--device may only be specified once");
                device = Some(
                    arguments
                        .next()
                        .context("--device requires a serial device path")?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            "--baud-rate" | "-b" => {
                baud_rate = parse_nonzero_u32(
                    arguments.next().context("--baud-rate requires a rate")?,
                    "--baud-rate",
                )?;
            }
            "--data-bits" | "-D" => {
                data_bits = parse_serial_data_bits(
                    arguments
                        .next()
                        .context("--data-bits requires 5, 6, 7, or 8")?,
                )?;
            }
            "--parity" | "-p" => {
                parity = parse_serial_parity(
                    arguments
                        .next()
                        .context("--parity requires none, odd, or even")?,
                )?;
            }
            "--stop-bits" | "-s" => {
                stop_bits = parse_serial_stop_bits(
                    arguments.next().context("--stop-bits requires 1 or 2")?,
                )?;
            }
            "--flow-control" | "-f" => {
                flow_control = parse_serial_flow_control(
                    arguments
                        .next()
                        .context("--flow-control requires none, software, or hardware")?,
                )?;
            }
            option if option.starts_with('-') => anyhow::bail!("unknown serial option {option:?}"),
            value => anyhow::bail!(
                "unexpected serial argument {value:?}; specify the device with --device"
            ),
        }
    }
    let device = device.context("--device is required; run `zetta serial --help` for details")?;
    anyhow::ensure!(!device.is_empty(), "--device must not be empty");
    Ok(CliServiceCommand::Serial(SerialCommand::Connect(
        SerialConnectionOptions {
            device,
            baud_rate,
            data_bits,
            parity,
            stop_bits,
            flow_control,
        },
    )))
}

fn parse_nonzero_u32(argument: &OsString, option: &str) -> Result<u32> {
    let value = argument
        .to_string_lossy()
        .parse::<u32>()
        .with_context(|| format!("{option} must be a positive whole number"))?;
    anyhow::ensure!(value > 0, "{option} must be greater than zero");
    Ok(value)
}

fn parse_serial_data_bits(argument: &OsString) -> Result<u8> {
    match argument.to_string_lossy().as_ref() {
        "5" => Ok(5),
        "6" => Ok(6),
        "7" => Ok(7),
        "8" => Ok(8),
        value => anyhow::bail!("--data-bits must be 5, 6, 7, or 8, got {value:?}"),
    }
}

fn parse_serial_parity(argument: &OsString) -> Result<SerialParity> {
    match argument.to_string_lossy().to_ascii_lowercase().as_str() {
        "none" => Ok(SerialParity::None),
        "odd" => Ok(SerialParity::Odd),
        "even" => Ok(SerialParity::Even),
        value => anyhow::bail!("--parity must be none, odd, or even, got {value:?}"),
    }
}

fn parse_serial_stop_bits(argument: &OsString) -> Result<u8> {
    match argument.to_string_lossy().as_ref() {
        "1" => Ok(1),
        "2" => Ok(2),
        value => anyhow::bail!("--stop-bits must be 1 or 2, got {value:?}"),
    }
}

fn parse_serial_flow_control(argument: &OsString) -> Result<SerialFlowControl> {
    match argument.to_string_lossy().to_ascii_lowercase().as_str() {
        "none" => Ok(SerialFlowControl::None),
        "software" => Ok(SerialFlowControl::Software),
        "hardware" => Ok(SerialFlowControl::Hardware),
        value => anyhow::bail!("--flow-control must be none, software, or hardware, got {value:?}"),
    }
}

impl SerialCommand {
    pub(super) fn run(&self) -> Result<()> {
        match self {
            Self::List => {
                let devices = serialport::available_ports()
                    .map(crate::detected_serial_devices)
                    .context("enumerating serial devices")?;
                for device in devices {
                    println!("{}", device.port_name);
                }
                Ok(())
            }
            Self::Connect(options) => run_serial_console(options),
        }
    }
}

fn run_serial_console(options: &SerialConnectionOptions) -> Result<()> {
    let settings = crate::SerialConnectionSettings {
        port_name: options.device.clone(),
        baud_rate: options.baud_rate,
        data_bits: match options.data_bits {
            5 => serialport::DataBits::Five,
            6 => serialport::DataBits::Six,
            7 => serialport::DataBits::Seven,
            8 => serialport::DataBits::Eight,
            _ => unreachable!("serial data bits are validated while parsing"),
        },
        parity: match options.parity {
            SerialParity::None => serialport::Parity::None,
            SerialParity::Odd => serialport::Parity::Odd,
            SerialParity::Even => serialport::Parity::Even,
        },
        stop_bits: match options.stop_bits {
            1 => serialport::StopBits::One,
            2 => serialport::StopBits::Two,
            _ => unreachable!("serial stop bits are validated while parsing"),
        },
        flow_control: match options.flow_control {
            SerialFlowControl::None => serialport::FlowControl::None,
            SerialFlowControl::Software => serialport::FlowControl::Software,
            SerialFlowControl::Hardware => serialport::FlowControl::Hardware,
        },
    };
    let connection = crate::open_serial_connection(&settings)?;
    let _raw_terminal = RawTerminal::enable()?;
    eprintln!(
        "Connected to {} at {} baud. Press Ctrl-] to disconnect; Ctrl-C is sent to the device.",
        settings.port_name, settings.baud_rate
    );

    let active = Arc::new(AtomicBool::new(true));
    let writer = connection.writer;
    let input_active = active.clone();
    std::thread::Builder::new()
        .name("serial-console-input".to_owned())
        .spawn(move || copy_stdin_to_serial(writer, input_active))
        .context("starting serial console input")?;

    let mut reader = connection.reader;
    let mut stdout = io::stdout().lock();
    let mut buffer = [0; 4096];
    while active.load(Ordering::Acquire) {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => {
                if let Err(error) = stdout.write_all(&buffer[..count]) {
                    if error.kind() == io::ErrorKind::BrokenPipe {
                        return Ok(());
                    }
                    return Err(error).context("writing serial output");
                }
                stdout.flush().context("flushing serial output")?;
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) => {}
            Err(error) => return Err(error).context("reading serial output"),
        }
    }
    Ok(())
}

fn copy_stdin_to_serial(mut writer: Box<dyn serialport::SerialPort>, active: Arc<AtomicBool>) {
    let mut stdin = io::stdin().lock();
    let mut buffer = [0; 1024];
    loop {
        let count = match stdin.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(count) => count,
        };
        if let Some(disconnect) = buffer[..count].iter().position(|byte| *byte == 0x1d) {
            if disconnect > 0 && writer.write_all(&buffer[..disconnect]).is_err() {
                break;
            }
            break;
        }
        if writer.write_all(&buffer[..count]).is_err() || writer.flush().is_err() {
            break;
        }
    }
    active.store(false, Ordering::Release);
}

#[cfg(test)]
#[path = "../tests/cli_services/serial.rs"]
mod tests;
