#[cfg(all(feature = "clipboard", any(target_os = "linux", target_os = "freebsd")))]
use std::env;
#[cfg(any(
    feature = "serial-console",
    feature = "http-server",
    feature = "tftp-server",
    feature = "notifications",
    feature = "clipboard"
))]
use std::ffi::OsString;
#[cfg(all(feature = "notifications", any(not(target_os = "macos"), test)))]
use std::fs;
#[cfg(any(
    feature = "serial-console",
    feature = "http-server",
    feature = "tftp-server",
    feature = "clipboard"
))]
use std::io::{self, Read as _, Write as _};
#[cfg(feature = "notifications")]
use std::path::Path;
#[cfg(any(
    feature = "http-server",
    feature = "tftp-server",
    feature = "notifications"
))]
use std::path::PathBuf;
#[cfg(all(feature = "notifications", target_os = "macos"))]
use std::process::Command;
#[cfg(all(
    feature = "notifications",
    any(target_os = "linux", target_os = "freebsd")
))]
use std::process::{Command, Stdio};
#[cfg(feature = "serial-console")]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[cfg(any(
    feature = "serial-console",
    feature = "http-server",
    feature = "tftp-server",
    feature = "notifications",
    feature = "clipboard"
))]
use anyhow::Context as _;
#[cfg(any(
    feature = "serial-console",
    feature = "http-server",
    feature = "tftp-server",
    feature = "notifications",
    feature = "clipboard"
))]
use anyhow::Result;

#[cfg(any(
    feature = "serial-console",
    feature = "http-server",
    feature = "tftp-server",
    feature = "notifications",
    feature = "clipboard"
))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CliServiceCommand {
    #[cfg(feature = "serial-console")]
    Serial(SerialCommand),
    #[cfg(feature = "http-server")]
    Http(HttpServerCommand),
    #[cfg(feature = "tftp-server")]
    Tftp(TftpServerCommand),
    #[cfg(feature = "notifications")]
    Notify(NotifyCommand),
    #[cfg(feature = "clipboard")]
    Copy(CopyCommand),
    #[cfg(feature = "clipboard")]
    Paste(PasteCommand),
    #[cfg(feature = "clipboard")]
    CopyDaemon,
}

#[cfg(any(
    feature = "serial-console",
    feature = "http-server",
    feature = "tftp-server",
    feature = "notifications",
    feature = "clipboard"
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
            #[cfg(feature = "notifications")]
            Self::Notify(command) => command.run(),
            #[cfg(feature = "clipboard")]
            Self::Copy(command) => command.run(),
            #[cfg(feature = "clipboard")]
            Self::Paste(command) => command.run(),
            #[cfg(feature = "clipboard")]
            Self::CopyDaemon => run_clipboard_copy_daemon(),
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

#[cfg(feature = "notifications")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NotifyCommand {
    summary: String,
    body: Option<String>,
    app_name: Option<String>,
    icon: Option<String>,
    sound: Option<String>,
    timeout: Option<notify_rust::Timeout>,
}

#[cfg(feature = "notifications")]
pub(crate) fn notify_help() -> &'static str {
    "Show a desktop notification\n\nUsage: zetta notify [OPTIONS] SUMMARY [BODY]\n\nSUMMARY is the notification's title; BODY is optional additional text.\n\nOptions:\n  -a, --app-name NAME                Set the notification's application name\n  -i, --icon PATH                    Show an image from PATH with the notification (default: Zetta's icon)\n  -s, --sound NAME                   zetta-default, zetta-ok, zetta-alarm, or a platform-specific system sound name\n  -t, --timeout WHEN                 default, never, or a number of milliseconds (default: default)\n  -h, --help                         Print help\n\nShows the notification through the desktop's native notification system: D-Bus\non Linux and BSD, Notification Center on macOS, and toast notifications on\nWindows. Without --icon, Zetta's own icon is shown; it is bundled in the\nbinary, so it is always available. --app-name has no effect on macOS and\n--timeout is ignored by some macOS notification centers; every other option\nbehaves the same on all platforms.\n\n--sound zetta-default, zetta-ok, and zetta-alarm are bundled tones that Zetta\nsynthesizes and plays itself, so they always play the same way regardless of\nthe host's sound theme or configuration. Any other value is passed through as\na platform-specific system sound name (for example a freedesktop sound-theme\nname on Linux, a system sound name on macOS, or a toast sound identifier on\nWindows) and is only played if the platform recognizes it."
}

#[cfg(feature = "notifications")]
pub(crate) fn parse_notify_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<CliServiceCommand> {
    let mut app_name = None;
    let mut icon = None;
    let mut sound = None;
    let mut timeout = None;
    let mut positional = Vec::new();
    let mut arguments = args.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            "--app-name" | "-a" => {
                anyhow::ensure!(app_name.is_none(), "--app-name may only be specified once");
                app_name = Some(
                    arguments
                        .next()
                        .context("--app-name requires a name")?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            "--icon" | "-i" => {
                anyhow::ensure!(icon.is_none(), "--icon may only be specified once");
                icon = Some(
                    arguments
                        .next()
                        .context("--icon requires a path")?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            "--sound" | "-s" => {
                anyhow::ensure!(sound.is_none(), "--sound may only be specified once");
                sound = Some(
                    arguments
                        .next()
                        .context("--sound requires a name")?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            "--timeout" | "-t" => {
                anyhow::ensure!(timeout.is_none(), "--timeout may only be specified once");
                let value = arguments
                    .next()
                    .context("--timeout requires default, never, or a number of milliseconds")?
                    .to_string_lossy()
                    .into_owned();
                timeout = Some(parse_notify_timeout(&value)?);
            }
            "--help" | "-h" => anyhow::bail!("{}", notify_help()),
            option if option.starts_with('-') => anyhow::bail!("unknown notify option {option:?}"),
            _ => positional.push(argument),
        }
    }
    anyhow::ensure!(
        (1..=2).contains(&positional.len()),
        "usage: zetta notify [OPTIONS] SUMMARY [BODY]; run `zetta notify --help` for details"
    );
    let summary = positional[0].to_string_lossy().into_owned();
    anyhow::ensure!(!summary.is_empty(), "SUMMARY must not be empty");
    let body = positional
        .get(1)
        .map(|value| value.to_string_lossy().into_owned());
    Ok(CliServiceCommand::Notify(NotifyCommand {
        summary,
        body,
        app_name,
        icon,
        sound,
        timeout,
    }))
}

#[cfg(feature = "notifications")]
fn parse_notify_timeout(value: &str) -> Result<notify_rust::Timeout> {
    value.parse().map_err(|_: std::num::ParseIntError| {
        anyhow::anyhow!(
            "--timeout must be default, never, or a whole number of milliseconds, got {value:?}"
        )
    })
}

#[cfg(all(feature = "notifications", not(target_os = "macos")))]
fn default_notification_icon_path() -> Result<PathBuf> {
    write_default_notification_icon(&crate::config::platform_config_dir())
}

#[cfg(all(feature = "notifications", any(not(target_os = "macos"), test)))]
fn notification_app_name(command: &NotifyCommand) -> &str {
    command.app_name.as_deref().unwrap_or(crate::ZETTA_APP_ID)
}

#[cfg(all(
    feature = "notifications",
    not(any(target_os = "macos", target_os = "windows"))
))]
fn notification_icon_path(command: &NotifyCommand) -> Result<String> {
    Ok(match &command.icon {
        Some(icon) => icon.clone(),
        None => default_notification_icon_path()?
            .to_string_lossy()
            .into_owned(),
    })
}

#[cfg(all(
    feature = "notifications",
    not(any(target_os = "macos", target_os = "windows"))
))]
fn set_unix_notification_identity(
    notification: &mut notify_rust::Notification,
    command: &NotifyCommand,
) -> Result<()> {
    let icon = notification_icon_path(command)?;
    if command.app_name.is_none() && command.icon.is_none() {
        notification.hint(notify_rust::Hint::DesktopEntry(
            crate::ZETTA_APP_ID.to_owned(),
        ));
    }
    notification.icon(&icon);
    Ok(())
}

#[cfg(all(
    feature = "notifications",
    any(target_os = "linux", target_os = "freebsd")
))]
fn try_show_portal_notification(command: &NotifyCommand) -> Result<bool> {
    // Unlike org.freedesktop.Notifications, the portal contract explicitly
    // requires notifications to outlive the process that submitted them.
    // The portal does not expose the millisecond timeout supported by
    // notify-rust, and it cannot override the application identity, so keep
    // those cases on the D-Bus fallback below.
    if command.app_name.is_some()
        || command.icon.is_some()
        || matches!(
            command.timeout,
            Some(notify_rust::Timeout::Milliseconds(_)) | Some(notify_rust::Timeout::Never)
        )
        || command
            .sound
            .as_deref()
            .is_some_and(|sound| crate::notification_sounds::BuiltinSound::parse(sound).is_none())
    {
        return Ok(false);
    }

    let icon = notification_icon_path(command)?;
    let icon_uri = match url::Url::from_file_path(&icon) {
        Ok(icon_uri) => icon_uri,
        Err(()) => return Ok(false),
    };
    let portal_notification = ashpd::desktop::notification::Notification::new(&command.summary)
        .body(command.body.as_deref())
        .icon(ashpd::desktop::Icon::Uri(ashpd::Uri::parse(
            icon_uri.as_str(),
        )?));
    let notification_id = format!("zetta-{}", std::process::id());
    let sent = futures::executor::block_on(async {
        let proxy = ashpd::desktop::notification::NotificationProxy::new().await?;
        proxy
            .add_notification(&notification_id, portal_notification)
            .await
    });
    Ok(sent.is_ok())
}

#[cfg(all(
    feature = "notifications",
    any(target_os = "linux", target_os = "freebsd")
))]
const NOTIFICATION_DAEMON_ENV: &str = "ZETTA_NOTIFICATION_DAEMON";

#[cfg(all(
    feature = "notifications",
    any(target_os = "linux", target_os = "freebsd")
))]
fn spawn_notification_daemon() -> Result<()> {
    let executable = std::env::current_exe().context("locating the zetta executable")?;
    let mut command = Command::new(executable);
    command
        .args(std::env::args_os().skip(1))
        .env(NOTIFICATION_DAEMON_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Detach the notification worker from the terminal that invoked the CLI.
    // It must keep its D-Bus connection alive after this parent exits so GNOME
    // does not withdraw the notification with the sender process.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // SAFETY: setsid(2) is async-signal-safe and is the only call made in
        // the forked child before it execs.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    command
        .spawn()
        .context("spawning the desktop notification worker")?;
    Ok(())
}

#[cfg(all(
    feature = "notifications",
    any(target_os = "linux", target_os = "freebsd")
))]
fn keep_notification_worker_alive(timeout: Option<notify_rust::Timeout>) {
    let timeout = timeout.unwrap_or_default();
    match timeout {
        // GNOME's default is commonly five seconds. Keep the sender alive a
        // little longer so the server can expire and archive the notification
        // instead of treating the worker's exit as a dismissal.
        notify_rust::Timeout::Default => std::thread::sleep(std::time::Duration::from_secs(10)),
        notify_rust::Timeout::Milliseconds(milliseconds) => {
            if milliseconds == 0 {
                // A never-expiring notification needs a live sender. This is
                // intentionally indefinite, matching the notification's
                // lifetime.
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(60));
                }
            } else {
                std::thread::sleep(
                    std::time::Duration::from_millis(u64::from(milliseconds))
                        + std::time::Duration::from_secs(1),
                );
            }
        }
        // A never-expiring notification needs a live sender. This is
        // intentionally indefinite, matching the notification's lifetime.
        notify_rust::Timeout::Never => loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
        },
    }
}

// Without an explicit `App User Model ID`, notify-rust's Windows backend
// (tauri-winrt-notification) falls back to `Toast::POWERSHELL_APP_ID` - a
// built-in Windows AUMID whose own doc comment warns the toast "will
// erroneously report its origin as powershell", with PowerShell's icon.
// Register Zetta's own AUMID (idempotent; cheap enough to redo on every
// `zetta notify` invocation, mirroring `register_app_user_model_id` in
// crates/gpui_windows/src/system_notifications.rs) and point the toast at it.
//
// `IconUri` must be a plain path to an image file - unlike a shortcut's
// `IconLocation`, it does not understand the `<path>,<index>` resource syntax,
// so pointing it at the exe itself silently produces a blank icon. Reuse the
// same on-disk icon already passed to `Notification::image_path`.
#[cfg(all(feature = "notifications", target_os = "windows"))]
fn register_windows_notification_identity(
    notification: &mut notify_rust::Notification,
    icon_path: &Path,
) {
    let result = windows_registry::CURRENT_USER
        .create(format!(
            r"Software\Classes\AppUserModelId\{}",
            crate::ZETTA_APP_ID
        ))
        .and_then(|key| {
            key.set_string("DisplayName", "Zetta")?;
            key.set_string("IconBackgroundColor", "0")?;
            key.set_hstring("IconUri", &icon_path.into())
        });
    if let Err(error) = result {
        eprintln!(
            "zetta: failed to register AppUserModelID; notifications may not display correctly: {error}"
        );
    }
    notification.app_id(crate::ZETTA_APP_ID);
}

// D-Bus and winrt-notification take an icon as a filesystem path rather than
// raw bytes, so the icon embedded via ZettaEmbeddedAssets is cached on disk
// once and reused rather than rewritten on every `zetta notify` invocation.
#[cfg(all(feature = "notifications", any(not(target_os = "macos"), test)))]
fn write_default_notification_icon(config_dir: &Path) -> Result<PathBuf> {
    let icon = crate::zetta_assets::embedded_notification_icon()
        .context("embedded notification icon is missing")?;
    let path = config_dir.join("notification-icon.png");
    let up_to_date = fs::read(&path).is_ok_and(|existing| existing == *icon);
    if !up_to_date {
        fs::create_dir_all(config_dir)
            .with_context(|| format!("creating {}", config_dir.display()))?;
        fs::write(&path, &icon).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(path)
}

#[cfg(all(feature = "notifications", target_os = "macos"))]
const MACOS_NOTIFICATION_REEXEC_ENV: &str = "ZETTA_INTERNAL_NOTIFICATION_BUNDLE_REEXEC";

#[cfg(all(feature = "notifications", target_os = "macos"))]
fn macos_bundle_executable(path: &Path) -> Option<PathBuf> {
    let executable = path.canonicalize().ok()?;
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle
            .extension()
            .is_some_and(|extension| extension == "app"))
    .then_some(executable)
}

/// A process entered through `/usr/local/bin/zetta` does not inherit the
/// bundle identity of the signed executable behind that symlink. Re-enter the
/// exact same command through its canonical `.app` path so Notification Center
/// sees Zetta's bundle identifier. Standalone development builds return false
/// and use the script-host fallback below instead.
#[cfg(all(feature = "notifications", target_os = "macos"))]
fn rerun_notification_from_macos_bundle() -> Result<bool> {
    if std::env::var_os(MACOS_NOTIFICATION_REEXEC_ENV).is_some() {
        return Ok(false);
    }
    let current_executable = std::env::current_exe().context("locating the Zetta executable")?;
    let Some(bundle_executable) = macos_bundle_executable(&current_executable) else {
        return Ok(false);
    };
    let output = Command::new(&bundle_executable)
        .args(std::env::args_os().skip(1))
        .env(MACOS_NOTIFICATION_REEXEC_ENV, "1")
        .output()
        .with_context(|| {
            format!(
                "restarting notification command through {}",
                bundle_executable.display()
            )
        })?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        let message = message
            .trim()
            .strip_prefix("Zetta failed to start: ")
            .unwrap_or(message.trim());
        anyhow::bail!(
            "{}",
            if message.is_empty() {
                format!(
                    "bundled macOS notification command exited with {}",
                    output.status
                )
            } else {
                message.to_owned()
            }
        );
    }
    Ok(true)
}

/// `UNUserNotificationCenter` rejects binaries that are not inside an app
/// bundle. Keep `target/debug/zetta notify` and other standalone copies useful
/// by asking macOS's bundled script host to submit the notification instead.
#[cfg(all(feature = "notifications", target_os = "macos"))]
fn show_unbundled_macos_notification(command: &NotifyCommand, sound: Option<&str>) -> Result<()> {
    const SCRIPT: &str = r#"
function run(argv) {
    const app = Application.currentApplication();
    app.includeStandardAdditions = true;
    const options = { withTitle: argv[0] };
    if (argv[2]) options.subtitle = argv[2];
    if (argv[3]) options.soundName = argv[3];
    app.displayNotification(argv[1], options);
}
"#;
    let status = Command::new("/usr/bin/osascript")
        .args(["-l", "JavaScript", "-e", SCRIPT, "--"])
        .arg(&command.summary)
        .arg(command.body.as_deref().unwrap_or_default())
        .arg(command.app_name.as_deref().unwrap_or("Zetta"))
        .arg(sound.unwrap_or_default())
        .status()
        .context("showing an unbundled macOS desktop notification")?;
    anyhow::ensure!(
        status.success(),
        "macOS notification script exited with {status}"
    );
    Ok(())
}

#[cfg(all(feature = "notifications", target_os = "macos"))]
fn show_bundled_macos_notification(command: &NotifyCommand, sound: Option<&str>) -> Result<()> {
    let authorized = mac_usernotifications::blocking::request_auth()
        .map_err(|error| anyhow::anyhow!("{error}"))
        .context("requesting macOS desktop notification authorization")?;
    anyhow::ensure!(
        authorized,
        "macOS desktop notification authorization was denied; enable notifications for Zetta in System Settings"
    );

    let mut notification = mac_usernotifications::Notification::new()
        .title(&command.summary)
        .message(command.body.as_deref().unwrap_or_default())
        .maybe_sound(sound);
    // The signed app bundle already supplies the small Zetta identity icon.
    // Only an explicit --icon is an attachment; adding the embedded icon here
    // would render it a second time on the opposite side of the banner.
    if let Some(icon) = macos_notification_attachment(command) {
        notification = notification.image_path(icon);
    }
    mac_usernotifications::blocking::send(notification)
        .map_err(|error| anyhow::anyhow!("{error}"))
        .context("showing the desktop notification")?;
    Ok(())
}

#[cfg(all(feature = "notifications", target_os = "macos"))]
fn macos_notification_attachment(command: &NotifyCommand) -> Option<&str> {
    command.icon.as_deref()
}

#[cfg(all(feature = "notifications", target_os = "macos"))]
fn macos_notification_sound(command: &NotifyCommand) -> Option<&str> {
    command
        .sound
        .as_deref()
        .filter(|sound| crate::notification_sounds::BuiltinSound::parse(sound).is_none())
}

#[cfg(feature = "notifications")]
impl NotifyCommand {
    fn run(&self) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            self.run_macos()
        }

        #[cfg(not(target_os = "macos"))]
        {
            self.run_non_macos()
        }
    }

    #[cfg(target_os = "macos")]
    fn run_macos(&self) -> Result<()> {
        if mac_usernotifications::check_bundle().is_err() && rerun_notification_from_macos_bundle()?
        {
            return Ok(());
        }

        let bundled_sound = self
            .sound
            .as_deref()
            .and_then(crate::notification_sounds::BuiltinSound::parse);
        let notification_sound = macos_notification_sound(self);

        if mac_usernotifications::check_bundle().is_ok() {
            show_bundled_macos_notification(self, notification_sound)?;
        } else {
            show_unbundled_macos_notification(self, notification_sound)?;
        }
        if let Some(sound) = bundled_sound {
            sound.play()?;
        }
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    fn run_non_macos(&self) -> Result<()> {
        let bundled_sound = self
            .sound
            .as_deref()
            .and_then(crate::notification_sounds::BuiltinSound::parse);

        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if (bundled_sound.is_some() || self.sound.is_none()) && try_show_portal_notification(self)?
        {
            if let Some(bundled_sound) = bundled_sound {
                bundled_sound.play()?;
            }
            return Ok(());
        }

        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if std::env::var_os(NOTIFICATION_DAEMON_ENV).is_none() {
            return spawn_notification_daemon();
        }

        let mut notification = notify_rust::Notification::new();
        notification.summary(&self.summary);
        if let Some(body) = &self.body {
            notification.body(body);
        }
        notification.appname(notification_app_name(self));
        #[cfg(target_os = "windows")]
        {
            // notify-rust's Windows backend has no small "app logo" placement -
            // any icon passed to `image_path` renders as a large image below
            // the notification text. That's right for a user's deliberately
            // attached `--icon`, but Zetta's default icon shouldn't also be
            // shown that way: `register_windows_notification_identity` already
            // makes it appear correctly-sized next to the app name via the
            // AUMID registration, so only attach an inline image when the user
            // explicitly asked for one.
            register_windows_notification_identity(
                &mut notification,
                &default_notification_icon_path()?,
            );
            if let Some(icon) = &self.icon {
                notification.image_path(icon);
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            #[cfg(not(target_os = "macos"))]
            set_unix_notification_identity(&mut notification, self)?;
        }
        let notification_sound = self.sound.as_deref().filter(|_| bundled_sound.is_none());
        if let Some(sound) = notification_sound {
            notification.sound_name(sound);
        }
        if let Some(timeout) = self.timeout {
            notification.timeout(timeout);
        }
        let _notification_handle = notification
            .show()
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("showing the desktop notification")?;
        if let Some(bundled_sound) = bundled_sound {
            bundled_sound.play()?;
        }
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if std::env::var_os(NOTIFICATION_DAEMON_ENV).is_some() {
            keep_notification_worker_alive(self.timeout);
        }
        Ok(())
    }
}

#[cfg(feature = "clipboard")]
const CLIPBOARD_DAEMON_FLAG: &str = "--internal-clipboard-daemon";

#[cfg(feature = "clipboard")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CopyCommand;

#[cfg(feature = "clipboard")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PasteCommand;

#[cfg(feature = "clipboard")]
pub(crate) fn copy_help() -> &'static str {
    "Copy standard input to the clipboard\n\nUsage: zetta copy [OPTIONS]\n\nReads standard input and writes it to the system clipboard as UTF-8 text, mirroring macOS's pbcopy. Available as zcopy in shell integration, and as pbcopy on platforms other than macOS so pbcopy muscle memory keeps working there too.\n\nOptions:\n  -pboard NAME                 Accepted for pbcopy compatibility (general, ruler, find, or font); Zetta has only one clipboard, so this has no effect\n  -h, -help, --help            Print help\n\nOn Linux and FreeBSD, Zetta forks a short-lived background process that keeps serving the clipboard after this command exits, since the X11 and Wayland clipboards are only available while their owning process is running. macOS and Windows keep the clipboard through their own system services, so no such process is needed there."
}

#[cfg(feature = "clipboard")]
pub(crate) fn paste_help() -> &'static str {
    "Print the clipboard's contents\n\nUsage: zetta paste [OPTIONS]\n\nWrites the system clipboard's text contents to standard output, mirroring macOS's pbpaste. Available as zpaste in shell integration, and as pbpaste on platforms other than macOS so pbpaste muscle memory keeps working there too. Prints nothing if the clipboard is empty or holds no text.\n\nOptions:\n  -pboard NAME                 Accepted for pbpaste compatibility (general, ruler, find, or font); Zetta has only one clipboard, so this has no effect\n  -Prefer TYPE                 Accepted for pbpaste compatibility (txt, rtf, or ps); Zetta only stores plain text, so this has no effect\n  -h, -help, --help            Print help"
}

#[cfg(feature = "clipboard")]
fn parse_pboard_name(argument: &OsString) -> Result<()> {
    let value = argument.to_string_lossy();
    anyhow::ensure!(
        matches!(
            value.to_ascii_lowercase().as_str(),
            "general" | "ruler" | "find" | "font"
        ),
        "-pboard must be general, ruler, find, or font, got {value:?}"
    );
    Ok(())
}

#[cfg(feature = "clipboard")]
fn parse_prefer_type(argument: &OsString) -> Result<()> {
    let value = argument.to_string_lossy();
    anyhow::ensure!(
        matches!(value.to_ascii_lowercase().as_str(), "txt" | "rtf" | "ps"),
        "-Prefer must be txt, rtf, or ps, got {value:?}"
    );
    Ok(())
}

#[cfg(feature = "clipboard")]
pub(crate) fn parse_copy_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<CliServiceCommand> {
    let mut pboard_seen = false;
    let mut daemon = false;
    let mut arguments = args.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            CLIPBOARD_DAEMON_FLAG => daemon = true,
            "-pboard" | "--pboard" => {
                anyhow::ensure!(!pboard_seen, "-pboard may only be specified once");
                pboard_seen = true;
                parse_pboard_name(
                    &arguments
                        .next()
                        .context("-pboard requires general, ruler, find, or font")?,
                )?;
            }
            "--help" | "-h" | "-help" => anyhow::bail!("{}", copy_help()),
            option if option.starts_with('-') => anyhow::bail!("unknown copy option {option:?}"),
            value => anyhow::bail!("unexpected copy argument {value:?}"),
        }
    }
    Ok(if daemon {
        CliServiceCommand::CopyDaemon
    } else {
        CliServiceCommand::Copy(CopyCommand)
    })
}

#[cfg(feature = "clipboard")]
pub(crate) fn parse_paste_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<CliServiceCommand> {
    let mut pboard_seen = false;
    let mut prefer_seen = false;
    let mut arguments = args.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            "-pboard" | "--pboard" => {
                anyhow::ensure!(!pboard_seen, "-pboard may only be specified once");
                pboard_seen = true;
                parse_pboard_name(
                    &arguments
                        .next()
                        .context("-pboard requires general, ruler, find, or font")?,
                )?;
            }
            "-Prefer" | "--Prefer" | "-prefer" | "--prefer" => {
                anyhow::ensure!(!prefer_seen, "-Prefer may only be specified once");
                prefer_seen = true;
                parse_prefer_type(
                    &arguments
                        .next()
                        .context("-Prefer requires txt, rtf, or ps")?,
                )?;
            }
            "--help" | "-h" | "-help" => anyhow::bail!("{}", paste_help()),
            option if option.starts_with('-') => anyhow::bail!("unknown paste option {option:?}"),
            value => anyhow::bail!("unexpected paste argument {value:?}"),
        }
    }
    Ok(CliServiceCommand::Paste(PasteCommand))
}

#[cfg(all(feature = "clipboard", any(target_os = "linux", target_os = "freebsd")))]
fn spawn_clipboard_copy_daemon(text: String) -> Result<()> {
    use std::os::unix::process::CommandExt as _;
    use std::process::Stdio;

    let executable = env::current_exe().context("locating the zetta executable")?;
    let mut command = std::process::Command::new(executable);
    command
        .arg("copy")
        .arg(CLIPBOARD_DAEMON_FLAG)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .current_dir("/");
    // SAFETY: setsid(2) is async-signal-safe and is the only call made in the forked child
    // before it execs; detaching into its own session keeps the clipboard daemon alive after
    // this shell's session, and its controlling terminal, goes away.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut daemon = command.spawn().context("spawning the clipboard daemon")?;
    daemon
        .stdin
        .take()
        .context("the clipboard daemon did not provide a standard input pipe")?
        .write_all(text.as_bytes())
        .context("sending clipboard contents to the daemon")?;
    Ok(())
}

#[cfg(feature = "clipboard")]
fn run_clipboard_copy_daemon() -> Result<()> {
    let mut input = String::new();
    io::stdin()
        .lock()
        .read_to_string(&mut input)
        .context("reading standard input")?;
    let mut clipboard = arboard::Clipboard::new().context("opening the system clipboard")?;
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        use arboard::SetExtLinux as _;
        clipboard
            .set()
            .wait()
            .text(input)
            .context("serving the system clipboard")?;
    }
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    clipboard
        .set_text(input)
        .context("writing to the system clipboard")?;
    Ok(())
}

#[cfg(feature = "clipboard")]
impl CopyCommand {
    fn run(&self) -> Result<()> {
        let mut input = String::new();
        io::stdin()
            .lock()
            .read_to_string(&mut input)
            .context("reading standard input")?;
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            spawn_clipboard_copy_daemon(input)
        }
        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        {
            let mut clipboard =
                arboard::Clipboard::new().context("opening the system clipboard")?;
            clipboard
                .set_text(input)
                .context("writing to the system clipboard")?;
            Ok(())
        }
    }
}

#[cfg(feature = "clipboard")]
impl PasteCommand {
    fn run(&self) -> Result<()> {
        let mut clipboard = arboard::Clipboard::new().context("opening the system clipboard")?;
        match clipboard.get_text() {
            Ok(text) => io::stdout()
                .write_all(text.as_bytes())
                .context("writing the clipboard contents to standard output")?,
            Err(arboard::Error::ContentNotAvailable) => {}
            Err(error) => return Err(error).context("reading the system clipboard"),
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/cli_services.rs"]
mod tests;
