#[cfg(any(feature = "http-server", feature = "tftp-server"))]
use std::path::PathBuf;
#[cfg(feature = "serial-console")]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
#[cfg(any(
    feature = "serial-console",
    feature = "http-server",
    feature = "tftp-server"
))]
use std::{
    ffi::OsString,
    io::{self, Read as _, Write as _},
};

#[cfg(any(
    feature = "serial-console",
    feature = "http-server",
    feature = "tftp-server"
))]
use anyhow::Context as _;
#[cfg(any(
    feature = "serial-console",
    feature = "http-server",
    feature = "tftp-server"
))]
use anyhow::Result;

#[cfg(any(
    feature = "serial-console",
    feature = "http-server",
    feature = "tftp-server"
))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CliServiceCommand {
    #[cfg(feature = "serial-console")]
    Serial(SerialCommand),
    #[cfg(feature = "http-server")]
    Http(HttpServerCommand),
    #[cfg(feature = "tftp-server")]
    Tftp(TftpServerCommand),
}

#[cfg(any(
    feature = "serial-console",
    feature = "http-server",
    feature = "tftp-server"
))]
impl CliServiceCommand {
    pub(crate) fn run(&self) -> Result<()> {
        match self {
            #[cfg(feature = "serial-console")]
            Self::Serial(command) => command.run(),
            #[cfg(feature = "http-server")]
            Self::Http(command) => command.run(),
            #[cfg(feature = "tftp-server")]
            Self::Tftp(command) => command.run(),
        }
    }
}

#[cfg(feature = "serial-console")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SerialCommand {
    List,
    Connect(SerialConnectionOptions),
}

#[cfg(feature = "serial-console")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SerialConnectionOptions {
    pub(crate) device: String,
    pub(crate) baud_rate: u32,
    pub(crate) data_bits: u8,
    pub(crate) parity: SerialParity,
    pub(crate) stop_bits: u8,
    pub(crate) flow_control: SerialFlowControl,
}

#[cfg(feature = "serial-console")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SerialParity {
    None,
    Odd,
    Even,
}

#[cfg(feature = "serial-console")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SerialFlowControl {
    None,
    Software,
    Hardware,
}

#[cfg(feature = "serial-console")]
pub(crate) fn serial_help() -> &'static str {
    "Zetta serial console\n\nUsage:\n  zetta serial list\n  zetta serial console [OPTIONS]\n\nCommands:\n  list                              List currently available serial devices\n  console                           Connect the current terminal to a serial device\n\nConsole options:\n  -d, --device PATH                 Serial device to open (required)\n  -b, --baud-rate RATE              Baud rate (default: 115200)\n  -D, --data-bits BITS              5, 6, 7, or 8 (default: 8)\n  -p, --parity MODE                 none, odd, or even (default: none)\n  -s, --stop-bits BITS              1 or 2 (default: 1)\n  -f, --flow-control MODE           none, software, or hardware (default: none)\n  -h, --help                        Print help\n\nThe serial console uses the terminal's raw input mode. Ctrl-C is sent to the device; press Ctrl-] to disconnect locally."
}

#[cfg(feature = "serial-console")]
pub(crate) fn parse_serial_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<CliServiceCommand> {
    let args = args.into_iter().collect::<Vec<_>>();
    if args
        .iter()
        .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
    {
        anyhow::bail!(serial_help());
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

#[cfg(feature = "serial-console")]
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

#[cfg(feature = "serial-console")]
fn parse_nonzero_u32(argument: &OsString, option: &str) -> Result<u32> {
    let value = argument
        .to_string_lossy()
        .parse::<u32>()
        .with_context(|| format!("{option} must be a positive whole number"))?;
    anyhow::ensure!(value > 0, "{option} must be greater than zero");
    Ok(value)
}

#[cfg(feature = "serial-console")]
fn parse_serial_data_bits(argument: &OsString) -> Result<u8> {
    match argument.to_string_lossy().as_ref() {
        "5" => Ok(5),
        "6" => Ok(6),
        "7" => Ok(7),
        "8" => Ok(8),
        value => anyhow::bail!("--data-bits must be 5, 6, 7, or 8, got {value:?}"),
    }
}

#[cfg(feature = "serial-console")]
fn parse_serial_parity(argument: &OsString) -> Result<SerialParity> {
    match argument.to_string_lossy().to_ascii_lowercase().as_str() {
        "none" => Ok(SerialParity::None),
        "odd" => Ok(SerialParity::Odd),
        "even" => Ok(SerialParity::Even),
        value => anyhow::bail!("--parity must be none, odd, or even, got {value:?}"),
    }
}

#[cfg(feature = "serial-console")]
fn parse_serial_stop_bits(argument: &OsString) -> Result<u8> {
    match argument.to_string_lossy().as_ref() {
        "1" => Ok(1),
        "2" => Ok(2),
        value => anyhow::bail!("--stop-bits must be 1 or 2, got {value:?}"),
    }
}

#[cfg(feature = "serial-console")]
fn parse_serial_flow_control(argument: &OsString) -> Result<SerialFlowControl> {
    match argument.to_string_lossy().to_ascii_lowercase().as_str() {
        "none" => Ok(SerialFlowControl::None),
        "software" => Ok(SerialFlowControl::Software),
        "hardware" => Ok(SerialFlowControl::Hardware),
        value => anyhow::bail!("--flow-control must be none, software, or hardware, got {value:?}"),
    }
}

#[cfg(feature = "serial-console")]
impl SerialCommand {
    fn run(&self) -> Result<()> {
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

#[cfg(feature = "serial-console")]
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

#[cfg(feature = "serial-console")]
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

#[cfg(all(feature = "serial-console", unix))]
struct RawTerminal {
    original: libc::termios,
}

#[cfg(all(feature = "serial-console", unix))]
impl RawTerminal {
    fn enable() -> Result<Option<Self>> {
        use std::mem::MaybeUninit;

        let mut original = MaybeUninit::<libc::termios>::uninit();
        // SAFETY: stdin remains open for the lifetime of the process and the pointer is writable.
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, original.as_mut_ptr()) } != 0 {
            return Ok(None);
        }
        // SAFETY: tcgetattr initialized original after returning zero above.
        let original = unsafe { original.assume_init() };
        let mut raw = original;
        // SAFETY: raw is a valid termios structure owned by this function.
        unsafe { libc::cfmakeraw(&mut raw) };
        // SAFETY: stdin is a valid file descriptor and raw is a valid termios structure.
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) } != 0 {
            return Err(io::Error::last_os_error()).context("enabling raw terminal input");
        }
        Ok(Some(Self { original }))
    }
}

#[cfg(all(feature = "serial-console", unix))]
impl Drop for RawTerminal {
    fn drop(&mut self) {
        // SAFETY: stdin is a valid file descriptor and original was read from it in enable.
        unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.original) };
    }
}

#[cfg(all(feature = "serial-console", windows))]
struct RawTerminal {
    handle: windows::Win32::Foundation::HANDLE,
    original: windows::Win32::System::Console::CONSOLE_MODE,
}

#[cfg(all(feature = "serial-console", windows))]
impl RawTerminal {
    fn enable() -> Result<Option<Self>> {
        use windows::Win32::System::Console::{
            CONSOLE_MODE, ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT,
            GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, SetConsoleMode,
        };

        // SAFETY: the API obtains the current process's standard input handle.
        let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) }?;
        let mut original = CONSOLE_MODE(0);
        // SAFETY: handle comes from GetStdHandle and original points to writable storage.
        if unsafe { GetConsoleMode(handle, &mut original) }.is_err() {
            return Ok(None);
        }
        let raw = original & !(ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT);
        // SAFETY: handle comes from GetStdHandle and raw is a valid console mode bitset.
        unsafe { SetConsoleMode(handle, raw) }.context("enabling raw terminal input")?;
        Ok(Some(Self { handle, original }))
    }
}

#[cfg(all(feature = "serial-console", windows))]
impl Drop for RawTerminal {
    fn drop(&mut self) {
        use windows::Win32::System::Console::SetConsoleMode;

        // SAFETY: handle and original were captured from the active console in enable.
        let _ = unsafe { SetConsoleMode(self.handle, self.original) };
    }
}

#[cfg(all(feature = "serial-console", not(any(unix, windows))))]
struct RawTerminal;

#[cfg(all(feature = "serial-console", not(any(unix, windows))))]
impl RawTerminal {
    fn enable() -> Result<Option<Self>> {
        Ok(None)
    }
}

#[cfg(feature = "http-server")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HttpServerCommand {
    root: PathBuf,
    port: Option<u16>,
    config_path: Option<PathBuf>,
}

#[cfg(feature = "http-server")]
pub(crate) fn http_server_help() -> &'static str {
    "Serve static files over HTTP\n\nUsage: zetta http server [OPTIONS]\n\nOptions:\n  -r, --root PATH                   Directory to serve (default: current directory)\n  -p, --port PORT                   TCP port (default: http_server_port from configuration)\n  -c, --config PATH                 Read the HTTP port default from this configuration file\n  -h, --help                        Print help\n\nPress Ctrl-C to stop the server."
}

#[cfg(feature = "http-server")]
pub(crate) fn parse_http_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<CliServiceCommand> {
    let args = args.into_iter().collect::<Vec<_>>();
    if args
        .iter()
        .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
    {
        anyhow::bail!(http_server_help());
    }
    anyhow::ensure!(
        args.first().is_some_and(|argument| argument == "server"),
        "usage: zetta http server [OPTIONS]; run `zetta http server --help` for details"
    );
    let (root, port, config_path) = parse_server_options(&args[1..], "HTTP")?;
    Ok(CliServiceCommand::Http(HttpServerCommand {
        root,
        port,
        config_path,
    }))
}

#[cfg(feature = "http-server")]
impl HttpServerCommand {
    fn resolved_port(&self) -> Result<u16> {
        Ok(match self.port {
            Some(port) => port,
            None => {
                crate::Config::load(self.config_path.as_deref(), None)
                    .context("loading configuration for the HTTP server")?
                    .http_server_port
            }
        })
    }

    fn run(&self) -> Result<()> {
        let port = self.resolved_port()?;
        let server = crate::start_http_server(&self.root, port)?;
        eprintln!(
            "Serving {} at http://{}; press Ctrl-C to stop.",
            server.root.display(),
            server.address
        );
        stream_server_logs(server.reader, server.writer)
    }
}

#[cfg(feature = "tftp-server")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TftpServerCommand {
    root: PathBuf,
    port: Option<u16>,
    config_path: Option<PathBuf>,
}

#[cfg(feature = "tftp-server")]
pub(crate) fn tftp_server_help() -> &'static str {
    "Serve files with TFTP\n\nUsage: zetta tftp server [OPTIONS]\n\nOptions:\n  -r, --root PATH                   Directory to serve (default: current directory)\n  -p, --port PORT                   UDP port (default: tftp_server_port from configuration)\n  -c, --config PATH                 Read the TFTP port default from this configuration file\n  -h, --help                        Print help\n\nPress Ctrl-C to stop the server."
}

#[cfg(feature = "tftp-server")]
pub(crate) fn parse_tftp_server_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<CliServiceCommand> {
    let args = args.into_iter().collect::<Vec<_>>();
    if args
        .iter()
        .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
    {
        anyhow::bail!(tftp_server_help());
    }
    let (root, port, config_path) = parse_server_options(&args, "TFTP")?;
    Ok(CliServiceCommand::Tftp(TftpServerCommand {
        root,
        port,
        config_path,
    }))
}

#[cfg(any(feature = "http-server", feature = "tftp-server"))]
fn parse_server_options(
    args: &[OsString],
    service: &str,
) -> Result<(PathBuf, Option<u16>, Option<PathBuf>)> {
    let mut root = PathBuf::from(".");
    let mut root_set = false;
    let mut port = None;
    let mut config_path = None;
    let mut arguments = args.iter();
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            "--root" | "-r" => {
                anyhow::ensure!(!root_set, "--root may only be specified once");
                root = arguments
                    .next()
                    .context("--root requires a directory")?
                    .into();
                root_set = true;
            }
            "--port" | "-p" => {
                anyhow::ensure!(port.is_none(), "--port may only be specified once");
                port = Some(parse_port(
                    arguments.next().context("--port requires a port number")?,
                )?);
            }
            "--config" | "-c" => {
                anyhow::ensure!(config_path.is_none(), "--config may only be specified once");
                config_path = Some(arguments.next().context("--config requires a path")?.into());
            }
            option if option.starts_with('-') => {
                anyhow::bail!("unknown {service} server option {option:?}")
            }
            value => anyhow::bail!("unexpected {service} server argument {value:?}"),
        }
    }
    Ok((root, port, config_path))
}

#[cfg(any(feature = "http-server", feature = "tftp-server"))]
fn parse_port(argument: &OsString) -> Result<u16> {
    let port = argument
        .to_string_lossy()
        .parse::<u16>()
        .context("--port must be a number from 1 to 65535")?;
    anyhow::ensure!(port != 0, "--port must be a number from 1 to 65535");
    Ok(port)
}

#[cfg(feature = "tftp-server")]
impl TftpServerCommand {
    fn resolved_port(&self) -> Result<u16> {
        Ok(match self.port {
            Some(port) => port,
            None => {
                crate::Config::load(self.config_path.as_deref(), None)
                    .context("loading configuration for the TFTP server")?
                    .tftp_server_port
            }
        })
    }

    fn run(&self) -> Result<()> {
        let port = self.resolved_port()?;
        let server = crate::start_server(&self.root, port)?;
        eprintln!(
            "Serving {} with TFTP at {}; press Ctrl-C to stop.",
            server.root.display(),
            server.address
        );
        stream_server_logs(server.reader, server.writer)
    }
}

#[cfg(any(feature = "http-server", feature = "tftp-server"))]
fn stream_server_logs(
    mut reader: Box<dyn io::Read + Send>,
    _control: Box<dyn io::Write + Send>,
) -> Result<()> {
    let mut stdout = io::stdout().lock();
    let mut buffer = [0; 4096];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => {
                if let Err(error) = stdout.write_all(&buffer[..count]) {
                    if error.kind() == io::ErrorKind::BrokenPipe {
                        return Ok(());
                    }
                    return Err(error).context("writing server log output");
                }
                stdout.flush().context("flushing server log output")?;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error).context("reading server log output"),
        }
    }
}

#[cfg(test)]
#[path = "tests/cli_services.rs"]
mod tests;
