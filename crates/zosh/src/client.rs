//! Mosh session loop driven by crossterm.

use std::{
    ffi::OsString,
    io::{self, Read as _, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use anyhow::{Context as _, Result};
use mosh_rs::{
    Base64Key, DisplayPreference, HostEvent, MoshSession, Screen, sender::KEEP_ALIVE_DEFAULT_MS,
};

#[cfg(not(unix))]
use std::{io::IsTerminal as _, sync::mpsc, thread, time::Duration};

#[cfg(not(unix))]
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::{
    display::{self, DisplayScreen},
    escape::{EscapeAction, EscapeKey, EscapeState},
    notification::Notifier,
    terminal,
};

const IDLE_WAIT_MS: u64 = 100;
type ClientSession = MoshSession<DisplayScreen>;

/// Bounds on `--keep-alive=MS`.
///
/// The floor is Mosh's own minimum frame interval
/// (`sender::SEND_INTERVAL_MIN_MS`): below it a keep-alive cannot go out
/// any sooner anyway. The ceiling is Mosh's unassisted heartbeat, since
/// asking for a keep-alive slower than the one already there is asking
/// for nothing.
pub(crate) const KEEP_ALIVE_MIN_MS: u64 = 20;
pub(crate) const KEEP_ALIVE_MAX_MS: u64 = 3000;
/// The environment variable that carries `--keep-alive` to an external
/// endpoint client, alongside Mosh's own `MOSH_*` settings.
pub(crate) const KEEP_ALIVE_ENV: &str = "MOSH_KEEPALIVE";

/// Parsed endpoint arguments for `zosh SERVER_IP UDP_PORT`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientArgs {
    pub host: String,
    pub port: u16,
    pub help: bool,
    pub version: bool,
    pub colors: bool,
    pub keep_alive: Option<u64>,
}

/// The part of the Mosh launcher contract that is consumed by the bundled
/// endpoint client. Keeping this explicit lets `zosh` launch its own session
/// without round-tripping through an environment-mutating child process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SessionSettings {
    pub(crate) prediction: DisplayPreference,
    pub(crate) predict_overwrite: bool,
    pub(crate) initialize_terminal: bool,
    /// How long the session may go without sending before it emits a
    /// keep-alive, or `None` for Mosh's own three-second heartbeat.
    pub(crate) keep_alive: Option<u64>,
}

impl SessionSettings {
    pub(crate) fn from_environment() -> Self {
        let prediction = std::env::var("MOSH_PREDICTION_DISPLAY")
            .ok()
            .and_then(|value| {
                let preference = DisplayPreference::parse(&value);
                if preference.is_none() {
                    eprintln!(
                        "zosh: ignoring MOSH_PREDICTION_DISPLAY={value:?} (expected always, never, adaptive or experimental)"
                    );
                }
                preference
            })
            .unwrap_or_default();
        Self {
            prediction,
            predict_overwrite: std::env::var("MOSH_PREDICTION_OVERWRITE")
                .is_ok_and(|value| value == "yes"),
            // The standalone Zosh command defaults to the normal screen. The
            // launcher passes this setting explicitly; keep the legacy
            // endpoint entry point consistent when it is invoked directly.
            initialize_terminal: false,
            keep_alive: keep_alive_from_environment(),
        }
    }
}

fn keep_alive_from_environment() -> Option<u64> {
    let value = std::env::var(KEEP_ALIVE_ENV).ok()?;
    match parse_keep_alive_interval(&value) {
        Ok(interval) => Some(interval),
        Err(error) => {
            eprintln!("zosh: ignoring {KEEP_ALIVE_ENV}={value:?} ({error})");
            None
        }
    }
}

/// The interval `-k`, `--keep-alive`, `-k=MS` or `--keep-alive=MS` asks
/// for, or `None` when `value` is none of them.
fn keep_alive_argument(value: &str) -> Result<Option<u64>> {
    let (name, interval) = match value.split_once('=') {
        Some((name, interval)) => (name, Some(interval)),
        None => (value, None),
    };
    if name != "-k" && name != "--keep-alive" {
        return Ok(None);
    }
    match interval {
        Some(interval) => parse_keep_alive_interval(interval).map(Some),
        None => Ok(Some(KEEP_ALIVE_DEFAULT_MS)),
    }
}

/// Parse a `--keep-alive=MS` value, in milliseconds.
pub(crate) fn parse_keep_alive_interval(value: &str) -> Result<u64> {
    let interval = value
        .parse::<u64>()
        .with_context(|| format!("invalid keep-alive interval {value:?}"))?;
    anyhow::ensure!(
        (KEEP_ALIVE_MIN_MS..=KEEP_ALIVE_MAX_MS).contains(&interval),
        "keep-alive interval must be between {KEEP_ALIVE_MIN_MS} and {KEEP_ALIVE_MAX_MS} milliseconds"
    );
    Ok(interval)
}

/// Run the endpoint client with the supplied argument vector.
pub(crate) fn run_endpoint(arguments: impl IntoIterator<Item = OsString>) -> Result<()> {
    let args = parse_args(arguments)?;
    if args.help {
        print_help();
        return Ok(());
    }
    if args.version {
        println!("zosh {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.colors {
        println!("{}", terminal::color_count());
        return Ok(());
    }
    #[cfg(unix)]
    if let Err(message) = crate::locale::ensure_utf8() {
        anyhow::bail!("{message}");
    }
    let key = std::env::var("MOSH_KEY").context("MOSH_KEY is not set")?;
    // Keep the session key out of this process's environment once it has
    // been decoded. This is the same contract as stock mosh-client.
    unsafe { std::env::remove_var("MOSH_KEY") };
    let key = Base64Key::from_printable(&key).context("invalid MOSH_KEY")?;
    run_session(&args, &key)
}

pub(crate) fn parse_args(arguments: impl IntoIterator<Item = OsString>) -> Result<ClientArgs> {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    let mut help = false;
    let mut version = false;
    let mut colors = false;
    let mut keep_alive = None;
    let mut positional = Vec::new();
    for argument in arguments {
        let value = argument.to_string_lossy().into_owned();
        // Matched ahead of the flags because it is the only option here
        // that takes an attached value. Splitting every argument on '='
        // instead would misread a positional host that contains one.
        if let Some(interval) = keep_alive_argument(&value)? {
            anyhow::ensure!(keep_alive.is_none(), "duplicate --keep-alive");
            keep_alive = Some(interval);
            continue;
        }
        match value.as_str() {
            "--help" | "-h" => help = true,
            "--version" | "-V" => version = true,
            "-c" => colors = true,
            value if value.starts_with('-') => anyhow::bail!("unknown zosh option {value:?}"),
            value => positional.push(value.to_owned()),
        }
    }
    if help || version || colors {
        anyhow::ensure!(
            positional.is_empty(),
            "--help/--version/-c cannot be combined with an endpoint"
        );
        anyhow::ensure!(
            !(help && version),
            "--help and --version cannot be combined"
        );
        anyhow::ensure!(
            !(colors && (help || version)),
            "-c cannot be combined with help/version"
        );
        return Ok(ClientArgs {
            host: String::new(),
            port: 0,
            help,
            version,
            colors,
            keep_alive,
        });
    }
    anyhow::ensure!(positional.len() == 2, "usage: zosh SERVER_IP UDP_PORT");
    let port = positional[1]
        .parse::<u16>()
        .with_context(|| format!("invalid UDP port {:?}", positional[1]))?;
    anyhow::ensure!(port != 0, "UDP port must be between 1 and 65535");
    Ok(ClientArgs {
        host: positional.remove(0),
        port,
        help,
        version,
        colors,
        keep_alive,
    })
}

fn print_help() {
    println!(
        "Zetta Mosh client\n\nUsage: zosh SERVER_IP UDP_PORT\n       zosh -c\n\nReads the session key from MOSH_KEY. `-c` prints the terminal color count for the Mosh bootstrap.\n\nOptions:\n  -c                 Print terminal color count\n  -k, --keep-alive   Hold the link to a packet every {KEEP_ALIVE_DEFAULT_MS} ms (=MS to change, {KEEP_ALIVE_MIN_MS}-{KEEP_ALIVE_MAX_MS})\n  -h, --help         Print help\n  -V, --version      Print version"
    );
}

fn run_session(args: &ClientArgs, key: &Base64Key) -> Result<()> {
    let mut settings = SessionSettings::from_environment();
    // An explicit argument beats the environment, the way the endpoint
    // client's other settings do not need to because only the launcher
    // sets them.
    if args.keep_alive.is_some() {
        settings.keep_alive = args.keep_alive;
    }
    run_session_with_settings(&args.host, args.port, key, settings)
}

pub(crate) fn run_session_with_settings(
    host: &str,
    port: u16,
    key: &Base64Key,
    settings: SessionSettings,
) -> Result<()> {
    let (cols, rows) = terminal::size();
    let cols = cols.max(1);
    let rows = rows.max(1);
    let mut session =
        MoshSession::connect_with_screen(host, port, key, DisplayScreen::new(rows, cols))
            .context("connecting to the Mosh UDP endpoint")?;
    configure_session(&mut session, settings);
    let mut terminal_guard =
        terminal::TerminalGuard::enter_with_initialization(settings.initialize_terminal)?;
    if terminal_guard.is_initialized() {
        let mut stdout = io::stdout();
        display::open(&mut stdout).context("initializing the Mosh display")?;
    }
    install_panic_cleanup(&terminal_guard);
    let result = session_loop(
        &mut session,
        &mut terminal_guard,
        (cols, rows),
        settings.initialize_terminal,
    );
    terminal_guard.restore();
    if result.is_ok() {
        print_exit_message().context("printing the Mosh exit message")?;
    }
    result
}

fn install_panic_cleanup(terminal_guard: &terminal::TerminalGuard) {
    if terminal_guard.is_initialized() {
        let state = terminal_guard.state();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic| {
            terminal::restore_state(state.clone());
            previous(panic);
        }));
    }
}

fn configure_session(session: &mut ClientSession, settings: SessionSettings) {
    session
        .prediction_mut()
        .set_display_preference(settings.prediction);
    if settings.predict_overwrite {
        session.prediction_mut().set_predict_overwrite(true);
    }
    session.set_keep_alive(settings.keep_alive);
    if std::env::var_os("MOSH_TITLE_NOPREFIX").is_none() {
        session.set_title_prefix("[mosh] ");
    }
}

fn session_loop(
    session: &mut ClientSession,
    terminal_guard: &mut terminal::TerminalGuard,
    mut size: (u16, u16),
    initialize_terminal: bool,
) -> Result<()> {
    session.send_resize(i32::from(size.0), i32::from(size.1));
    let escape = EscapeKey::from_env(std::env::var("MOSH_ESCAPE_KEY").ok().as_deref());
    let mut escape_state = EscapeState::new(escape);

    #[cfg(unix)]
    let mut input = [0_u8; 4096];
    #[cfg(unix)]
    let mut input_closed = false;

    #[cfg(not(unix))]
    let interactive = io::stdin().is_terminal();
    #[cfg(not(unix))]
    let pipe_input = (!interactive).then(spawn_pipe_reader);
    #[cfg(not(unix))]
    let mut input_closed = false;

    let signals = signal_state(terminal_guard.is_initialized());
    let started = Instant::now();
    let mut notifier = Notifier::new(escape.name().as_deref());
    let mut stdout = io::stdout();
    let mut pending_resize = None;
    loop {
        if signals
            .interrupt
            .as_ref()
            .is_some_and(|flag| flag.swap(false, Ordering::SeqCst))
        {
            // A terminal that still has ISIG enabled delivers Ctrl-C as a
            // process signal instead of the byte 0x03. Treat that signal as
            // the same input the remote application would have received in
            // raw mode; registering SIGINT as a shutdown signal loses TUI
            // interrupts and leaves the user's prompt on a dirty screen.
            session.send_input(&[0x03]);
        }
        if signals
            .terminate
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
        {
            notifier.say("Exiting on signal...", elapsed(started));
            session.shutdown();
            finish_shutdown(session, &mut notifier, started, size.0, &mut stdout)?;
            return Ok(());
        }

        let current = terminal::size();
        let current = (current.0.max(1), current.1.max(1));
        if current != size {
            size = current;
            session.send_resize(i32::from(size.0), i32::from(size.1));
            pending_resize = Some(size);
        }

        let wait = session.wait_time_ms().min(IDLE_WAIT_MS);

        #[cfg(unix)]
        {
            let input_ready = wait_for_input_or_network(session, wait, !input_closed)?;
            if input_ready {
                let read = io::stdin()
                    .read(&mut input)
                    .context("reading terminal input")?;
                if read == 0 {
                    input_closed = true;
                } else {
                    if let Some(action) = apply_input(session, &mut escape_state, &input[..read]) {
                        let columns = size.0;
                        let mut context = ActionContext {
                            session,
                            terminal_guard,
                            size: &mut size,
                            initialize_terminal,
                            notifier: &mut notifier,
                            started,
                            columns,
                            stdout: &mut stdout,
                        };
                        if handle_escape_action(action, &mut context)? {
                            return Ok(());
                        }
                    }
                }
            }
        }

        #[cfg(not(unix))]
        if interactive && event::poll(Duration::from_millis(wait))? {
            while event::poll(Duration::ZERO)? {
                let action =
                    read_event(session, &mut escape_state, &mut size, &mut pending_resize)?;
                if let Some(action) = action {
                    let columns = size.0;
                    let mut context = ActionContext {
                        session,
                        terminal_guard,
                        size: &mut size,
                        initialize_terminal,
                        notifier: &mut notifier,
                        started,
                        columns,
                        stdout: &mut stdout,
                    };
                    if handle_escape_action(action, &mut context)? {
                        return Ok(());
                    }
                }
            }
        } else if let Some(receiver) = pipe_input.as_ref() {
            if !input_closed {
                match receiver.recv_timeout(Duration::from_millis(wait.max(1))) {
                    Ok(bytes) if !bytes.is_empty() => {
                        if let Some(action) = apply_input(session, &mut escape_state, &bytes) {
                            let columns = size.0;
                            let mut context = ActionContext {
                                session,
                                terminal_guard,
                                size: &mut size,
                                initialize_terminal,
                                notifier: &mut notifier,
                                started,
                                columns,
                                stdout: &mut stdout,
                            };
                            if handle_escape_action(action, &mut context)? {
                                return Ok(());
                            }
                        }
                    }
                    Ok(_) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                        input_closed = true;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            } else {
                thread::sleep(Duration::from_millis(wait.max(1)));
            }
        }

        let events = session.pump_ready().context("pumping the Mosh session")?;
        let server_size = (session.displayed().cols(), session.displayed().rows());
        let resize_ready = pending_resize.is_some_and(|expected| expected == server_size);
        let server_reported_resize = events_contain_resize(&events);
        if resize_ready || (server_reported_resize && pending_resize.is_none()) {
            let previous = session.displayed().clone();
            let repaint = session.repaint();
            let input_modes = session.displayed().input_modes_diff(&previous);
            let scrollback_clear = if session.displayed().clears_scrollback_since(&previous) {
                display::clear_scrollback_sequence()
            } else {
                &[]
            };
            display::repaint_after_resize(&mut stdout, &repaint, &input_modes, scrollback_clear)?;
            if resize_ready {
                pending_resize = None;
            }
        } else if pending_resize.is_none() {
            paint(
                &mut stdout,
                session,
                &mut notifier,
                elapsed(started),
                size.0,
            )?;
        } else {
            // Do not apply a frame for the old geometry to a terminal that
            // has already been resized. The server's next frame will carry
            // the new geometry; repaint it atomically when it arrives.
        }
        if session.finished() {
            return Ok(());
        }
    }
}

fn events_contain_resize(events: &[HostEvent]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, HostEvent::Resize { .. }))
}

#[cfg(not(unix))]
fn spawn_pipe_reader() -> mpsc::Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut stdin = io::stdin();
        let mut bytes = [0_u8; 4096];
        loop {
            match stdin.read(&mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    if sender.send(bytes[..count].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    receiver
}

#[cfg(unix)]
fn wait_for_input_or_network(
    session: &ClientSession,
    timeout_ms: u64,
    watch_input: bool,
) -> io::Result<bool> {
    use std::os::fd::AsRawFd as _;

    let stdin = io::stdin();
    let mut descriptors = Vec::with_capacity(session.socket_handles().len() + 1);
    let input_index = if watch_input {
        descriptors.push(libc::pollfd {
            fd: stdin.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        });
        Some(0)
    } else {
        None
    };
    descriptors.extend(session.socket_handles().into_iter().map(|fd| libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    }));
    let timeout = timeout_ms.min(i32::MAX as u64) as i32;
    // SAFETY: every descriptor is borrowed from a live stdin or Mosh socket,
    // and `descriptors` remains allocated until poll has returned.
    let result = unsafe {
        libc::poll(
            descriptors.as_mut_ptr(),
            descriptors.len() as libc::nfds_t,
            timeout,
        )
    };
    if result < 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            return Ok(false);
        }
        return Err(error);
    }
    Ok(input_index.is_some_and(|index| {
        let events = descriptors[index].revents;
        events & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0
    }))
}

fn apply_input(
    session: &mut ClientSession,
    escape: &mut EscapeState,
    bytes: &[u8],
) -> Option<EscapeAction> {
    let (send, action) = process_input_bytes(escape, bytes);
    if !send.is_empty() {
        session.send_input(&send);
    }
    action
}

fn process_input_bytes(escape: &mut EscapeState, bytes: &[u8]) -> (Vec<u8>, Option<EscapeAction>) {
    escape.feed_all(bytes)
}

struct ActionContext<'a, W: Write> {
    session: &'a mut ClientSession,
    terminal_guard: &'a mut terminal::TerminalGuard,
    size: &'a mut (u16, u16),
    initialize_terminal: bool,
    notifier: &'a mut Notifier,
    started: Instant,
    columns: u16,
    stdout: &'a mut W,
}

fn handle_escape_action<W: Write>(
    action: EscapeAction,
    context: &mut ActionContext<'_, W>,
) -> Result<bool> {
    match action {
        EscapeAction::Quit => {
            context
                .notifier
                .say("Exiting on user request...", elapsed(context.started));
            context.session.shutdown();
            finish_shutdown(
                context.session,
                context.notifier,
                context.started,
                context.columns,
                context.stdout,
            )?;
            Ok(true)
        }
        EscapeAction::Suspend => {
            suspend_and_resume(
                context.session,
                context.terminal_guard,
                context.size,
                context.initialize_terminal,
            )?;
            Ok(false)
        }
        EscapeAction::Pending | EscapeAction::Send(_) => unreachable!(),
    }
}

struct SignalState {
    terminate: Option<Arc<AtomicBool>>,
    interrupt: Option<Arc<AtomicBool>>,
}

fn signal_state(forward_interrupt: bool) -> SignalState {
    #[cfg(unix)]
    {
        let terminate = Arc::new(AtomicBool::new(false));
        for signal in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGHUP] {
            let _ = signal_hook::flag::register(signal, Arc::clone(&terminate));
        }
        if forward_interrupt {
            let interrupt = Arc::new(AtomicBool::new(false));
            let _ =
                signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&interrupt));
            SignalState {
                terminate: Some(terminate),
                interrupt: Some(interrupt),
            }
        } else {
            // A non-interactive invocation has no terminal byte stream to
            // which Ctrl-C can be translated, so preserve normal process
            // termination for that case.
            let _ =
                signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&terminate));
            SignalState {
                terminate: Some(terminate),
                interrupt: None,
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = forward_interrupt;
        SignalState {
            terminate: None,
            interrupt: None,
        }
    }
}

fn suspend_and_resume(
    session: &mut ClientSession,
    terminal_guard: &mut terminal::TerminalGuard,
    size: &mut (u16, u16),
    initialize_terminal: bool,
) -> Result<()> {
    #[cfg(unix)]
    {
        terminal_guard.restore();
        eprintln!("zosh: suspended; resuming terminal session");
        anyhow::ensure!(
            unsafe { libc::raise(libc::SIGTSTP) } == 0,
            "suspending zosh"
        );
        *terminal_guard = terminal::TerminalGuard::enter_with_initialization(initialize_terminal)?;
        let mut stdout = io::stdout();
        display::open(&mut stdout).context("reinitializing the Mosh display")?;
        let current = terminal::size();
        *size = (current.0.max(1), current.1.max(1));
        session.send_resize(i32::from(size.0), i32::from(size.1));
        let previous = session.displayed().clone();
        let repaint = session.repaint();
        stdout
            .write_all(&repaint)
            .and_then(|()| stdout.write_all(&session.displayed().input_modes()))
            .context("repainting after resuming zosh")?;
        if session.displayed().clears_scrollback_since(&previous) {
            stdout.write_all(display::clear_scrollback_sequence())?;
        }
        stdout.flush().context("flushing resumed zosh display")?;
    }
    #[cfg(not(unix))]
    {
        let _ = (session, terminal_guard, size, initialize_terminal);
        eprintln!("zosh: suspend is not available on this frontend");
    }
    Ok(())
}

#[cfg(not(unix))]
fn read_event(
    session: &mut ClientSession,
    escape: &mut EscapeState,
    size: &mut (u16, u16),
    pending_resize: &mut Option<(u16, u16)>,
) -> Result<Option<EscapeAction>> {
    let event = event::read().context("reading terminal input")?;
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat => {
            let bytes = key_bytes(key);
            let (send, action) = escape.feed_all(&bytes);
            if !send.is_empty() {
                session.send_input(&send);
            }
            Ok(action)
        }
        Event::Paste(text) => {
            session.send_input(text.as_bytes());
            Ok(None)
        }
        Event::Resize(columns, rows) => {
            *size = (columns.max(1), rows.max(1));
            session.send_resize(i32::from(size.0), i32::from(size.1));
            *pending_resize = Some(*size);
            Ok(None)
        }
        _ => Ok(None),
    }
}

#[cfg(not(unix))]
pub(crate) fn key_bytes(key: KeyEvent) -> Vec<u8> {
    match key.code {
        KeyCode::Char(character) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            control_byte(character).into_iter().collect()
        }
        KeyCode::Char(character) if key.modifiers.contains(KeyModifiers::ALT) => {
            let mut bytes = vec![0x1b];
            bytes.extend(character.to_string().as_bytes());
            bytes
        }
        KeyCode::Char(character) => character.to_string().into_bytes(),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Esc => vec![0x1b],
        KeyCode::Null => vec![0],
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::F(number) if number <= 12 => function_key_bytes(number),
        _ => Vec::new(),
    }
}

#[cfg(not(unix))]
fn function_key_bytes(number: u8) -> Vec<u8> {
    match number {
        1 => b"\x1bOP".to_vec(),
        2 => b"\x1bOQ".to_vec(),
        3 => b"\x1bOR".to_vec(),
        4 => b"\x1bOS".to_vec(),
        5 => b"\x1b[15~".to_vec(),
        6 => b"\x1b[17~".to_vec(),
        7 => b"\x1b[18~".to_vec(),
        8 => b"\x1b[19~".to_vec(),
        9 => b"\x1b[20~".to_vec(),
        10 => b"\x1b[21~".to_vec(),
        11 => b"\x1b[23~".to_vec(),
        12 => b"\x1b[24~".to_vec(),
        _ => Vec::new(),
    }
}

#[cfg(not(unix))]
pub(crate) fn control_byte(character: char) -> Option<u8> {
    let character = character.to_ascii_uppercase();
    if character.is_ascii() && character.is_ascii_control() {
        return Some(character as u8);
    }
    if character.is_ascii_alphabetic() {
        return Some((character as u8) & 0x1f);
    }
    match character {
        ' ' => Some(0),
        '@' => Some(0),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        '?' => Some(0x7f),
        _ => None,
    }
}

fn paint(
    stdout: &mut impl Write,
    session: &mut ClientSession,
    notifier: &mut Notifier,
    now: u64,
    columns: u16,
) -> io::Result<()> {
    let bar = notifier.bar(session.link_health(), now, columns);
    let bytes = session.render_with(&bar);
    if !bytes.is_empty() {
        stdout.write_all(&bytes)?;
        stdout.flush()?;
    }
    Ok(())
}

fn print_exit_message() -> io::Result<()> {
    let mut stdout = io::stdout();
    stdout.write_all(b"\r\n[mosh is exiting.]\r\n")?;
    stdout.flush()
}

fn finish_shutdown(
    session: &mut ClientSession,
    notifier: &mut Notifier,
    started: Instant,
    columns: u16,
    stdout: &mut impl Write,
) -> Result<()> {
    while !session.finished() {
        session.pump().context("finishing the Mosh shutdown")?;
        paint(stdout, session, notifier, elapsed(started), columns)
            .context("painting the Mosh shutdown")?;
    }
    Ok(())
}

fn elapsed(started: Instant) -> u64 {
    started.elapsed().as_millis() as u64
}

#[cfg(test)]
#[path = "tests/client.rs"]
mod tests;
