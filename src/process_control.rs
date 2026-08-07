use std::{
    fs,
    io::{BufRead as _, BufReader, Read as _, Write as _},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, RecvTimeoutError, Sender, channel},
    },
    thread,
    time::{Duration, Instant},
};
use zeroize::Zeroize as _;

#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(windows)]
use uds_windows::{UnixListener, UnixStream};

use anyhow::{Context as _, Result};
use futures::channel::mpsc::UnboundedSender;
use gpui::{Hsla, Rgba};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sysinfo::{Pid, ProcessesToUpdate, System};
use ui::IconName;

use crate::pane::{OverlayFontSize, PaneOverlayRequest, normalize_overlay_color_hex};

const CONTROL_VERSION: u32 = 3;
const MAX_CONTROL_MESSAGE_BYTES: usize = 4096;
const CONTROL_COMPLETION_TIMEOUT: Duration = Duration::from_secs(2);
const CONTROL_COMPLETION_POLL_INTERVAL: Duration = Duration::from_millis(25);
const CONTROL_CLIENT_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReconnectSessionResult {
    Reconnected,
    AuthenticationFailed,
    SessionNotFound,
    StillStarting,
    Rejected,
}

pub(crate) enum ProcessControlCommand {
    OpenWindow {
        completion: Sender<bool>,
    },
    ReconnectSession {
        runner_id: u64,
        session_id: u64,
        secret: Option<String>,
        completion: Sender<ReconnectSessionResult>,
    },
    SetTabIcon {
        icon: Option<IconName>,
        completion: Sender<bool>,
    },
    SetPaneTheme {
        theme: Option<String>,
        completion: Sender<bool>,
    },
    ListPaneThemes {
        completion: Sender<Vec<String>>,
    },
    SetPaneOverlay {
        text: Option<String>,
        font_size: Option<OverlayFontSize>,
        opacity: Option<f32>,
        color: Option<Hsla>,
        completion: Sender<bool>,
    },
}

#[derive(Clone, Debug, PartialEq)]
enum ControlRequestCommand {
    OpenWindow,
    ReconnectSession {
        runner_id: u64,
        session_id: u64,
        secret: Option<String>,
    },
    SetTabIcon {
        icon: Option<IconName>,
    },
    SetPaneTheme {
        theme: Option<String>,
    },
    ListPaneThemes,
    SetPaneOverlay {
        text: Option<String>,
        font_size: Option<OverlayFontSize>,
        opacity: Option<f32>,
        color: Option<String>,
    },
}

#[derive(Serialize, Deserialize)]
struct ControlEndpoint {
    version: u32,
    process_id: u32,
    socket_path: PathBuf,
    token: String,
}

#[derive(Serialize, Deserialize)]
struct ControlRequest {
    token: String,
    command: String,
    runner_id: Option<u64>,
    session_id: Option<u64>,
    secret: Option<String>,
    icon: Option<String>,
    pane_theme: Option<String>,
    pane_overlay: Option<String>,
    pane_overlay_font_size: Option<String>,
    pane_overlay_opacity: Option<u8>,
    pane_overlay_color: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct ControlResponse {
    status: String,
    #[serde(default)]
    themes: Vec<String>,
}

pub(crate) struct ProcessControlServer {
    endpoint_path: PathBuf,
    socket_path: PathBuf,
    stopping: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl ProcessControlServer {
    pub(crate) fn start(commands: UnboundedSender<ProcessControlCommand>) -> Result<Self> {
        Self::start_at(commands, control_endpoint_path(std::process::id()))
    }

    fn start_at(
        commands: UnboundedSender<ProcessControlCommand>,
        endpoint_path: PathBuf,
    ) -> Result<Self> {
        let parent = endpoint_path
            .parent()
            .context("control endpoint has no parent")?;
        fs::create_dir_all(parent)?;
        let socket_path = control_socket_path(&endpoint_path);
        remove_socket_if_present(&socket_path)?;
        let listener = UnixListener::bind(&socket_path)
            .context("binding the Zetta process control listener")?;
        let token = random_hex(32).context("generating the Zetta process control token")?;
        let endpoint = ControlEndpoint {
            version: CONTROL_VERSION,
            process_id: std::process::id(),
            socket_path: socket_path.clone(),
            token: token.clone(),
        };
        write_endpoint(&endpoint_path, &endpoint)?;
        let stopping = Arc::new(AtomicBool::new(false));
        let stopping_for_thread = stopping.clone();
        let thread = thread::Builder::new()
            .name("zetta-process-control".to_owned())
            .spawn(move || {
                for stream in listener.incoming() {
                    if stopping_for_thread.load(Ordering::Acquire) {
                        break;
                    }
                    let Ok(mut stream) = stream else {
                        continue;
                    };
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                    let mut response_themes = Vec::new();
                    let status = match handle_control_request(&mut stream, &token) {
                        Some(ControlRequestCommand::OpenWindow) => {
                            let (completion, completed) = channel();
                            let accepted = commands
                                .unbounded_send(ProcessControlCommand::OpenWindow { completion })
                                .is_ok()
                                && wait_for_control_completion(&completed, &stopping_for_thread);
                            if accepted { "ok" } else { "rejected" }
                        }
                        Some(ControlRequestCommand::ReconnectSession {
                            runner_id,
                            session_id,
                            secret,
                        }) => {
                            let (completion, completed) = channel();
                            let result = if commands
                                .unbounded_send(ProcessControlCommand::ReconnectSession {
                                    runner_id,
                                    session_id,
                                    secret,
                                    completion,
                                })
                                .is_ok()
                            {
                                wait_for_reconnect_completion(&completed, &stopping_for_thread)
                            } else {
                                ReconnectSessionResult::Rejected
                            };
                            reconnect_session_status(result)
                        }
                        Some(ControlRequestCommand::SetTabIcon { icon }) => {
                            let (completion, completed) = channel();
                            let accepted = commands
                                .unbounded_send(ProcessControlCommand::SetTabIcon {
                                    icon,
                                    completion,
                                })
                                .is_ok()
                                && wait_for_control_completion(&completed, &stopping_for_thread);
                            if accepted { "ok" } else { "rejected" }
                        }
                        Some(ControlRequestCommand::SetPaneTheme { theme }) => {
                            let (completion, completed) = channel();
                            let accepted = commands
                                .unbounded_send(ProcessControlCommand::SetPaneTheme {
                                    theme,
                                    completion,
                                })
                                .is_ok()
                                && wait_for_control_completion(&completed, &stopping_for_thread);
                            if accepted { "ok" } else { "rejected" }
                        }
                        Some(ControlRequestCommand::ListPaneThemes) => {
                            let (completion, completed) = channel();
                            let accepted = commands
                                .unbounded_send(ProcessControlCommand::ListPaneThemes {
                                    completion,
                                })
                                .is_ok();
                            match accepted
                                .then(|| {
                                    wait_for_theme_list_completion(&completed, &stopping_for_thread)
                                })
                                .flatten()
                            {
                                Some(themes) => {
                                    response_themes = themes;
                                    "ok"
                                }
                                None => "rejected",
                            }
                        }
                        Some(ControlRequestCommand::SetPaneOverlay {
                            text,
                            font_size,
                            opacity,
                            color,
                        }) => {
                            let (completion, completed) = channel();
                            let color = color
                                .and_then(|hex| {
                                    Rgba::try_from(normalize_overlay_color_hex(&hex).as_str()).ok()
                                })
                                .map(Hsla::from);
                            let accepted = commands
                                .unbounded_send(ProcessControlCommand::SetPaneOverlay {
                                    text,
                                    font_size,
                                    opacity,
                                    color,
                                    completion,
                                })
                                .is_ok()
                                && wait_for_control_completion(&completed, &stopping_for_thread);
                            if accepted { "ok" } else { "rejected" }
                        }
                        None => "rejected",
                    };
                    let status = if status == "ok" && stopping_for_thread.load(Ordering::Acquire) {
                        "rejected"
                    } else {
                        status
                    };
                    let _ = write_message(
                        &mut stream,
                        &ControlResponse {
                            status: status.to_owned(),
                            themes: response_themes,
                        },
                    );
                }
            })
            .context("starting the Zetta process control thread")?;
        Ok(Self {
            endpoint_path,
            socket_path,
            stopping,
            thread: Some(thread),
        })
    }

    pub(crate) fn is_accepting(&self) -> bool {
        !self.stopping.load(Ordering::Acquire)
    }

    pub(crate) fn begin_shutdown(&self) {
        if self.stopping.swap(true, Ordering::AcqRel) {
            return;
        }
        // Stop advertising this process before GPUI begins shutting down. A new
        // launch must start its own application instead of handing off to a
        // process that can no longer keep the requested window alive.
        let _ = fs::remove_file(&self.endpoint_path);
        let _ = UnixStream::connect(&self.socket_path);
        let _ = fs::remove_file(&self.socket_path);
    }
}

fn wait_for_control_completion(completed: &Receiver<bool>, stopping: &AtomicBool) -> bool {
    let deadline = Instant::now() + CONTROL_COMPLETION_TIMEOUT;
    loop {
        if stopping.load(Ordering::Acquire) {
            return false;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        match completed.recv_timeout(remaining.min(CONTROL_COMPLETION_POLL_INTERVAL)) {
            Ok(accepted) => return accepted && !stopping.load(Ordering::Acquire),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return false,
        }
    }
}

fn wait_for_theme_list_completion(
    completed: &Receiver<Vec<String>>,
    stopping: &AtomicBool,
) -> Option<Vec<String>> {
    let deadline = Instant::now() + CONTROL_COMPLETION_TIMEOUT;
    loop {
        if stopping.load(Ordering::Acquire) {
            return None;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        match completed.recv_timeout(remaining.min(CONTROL_COMPLETION_POLL_INTERVAL)) {
            Ok(themes) => return (!stopping.load(Ordering::Acquire)).then_some(themes),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
}

fn wait_for_reconnect_completion(
    completed: &Receiver<ReconnectSessionResult>,
    stopping: &AtomicBool,
) -> ReconnectSessionResult {
    let deadline = Instant::now() + CONTROL_COMPLETION_TIMEOUT;
    loop {
        if stopping.load(Ordering::Acquire) {
            return ReconnectSessionResult::Rejected;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return ReconnectSessionResult::Rejected;
        }
        match completed.recv_timeout(remaining.min(CONTROL_COMPLETION_POLL_INTERVAL)) {
            Ok(result) => return result,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return ReconnectSessionResult::Rejected,
        }
    }
}

fn reconnect_session_status(result: ReconnectSessionResult) -> &'static str {
    match result {
        ReconnectSessionResult::Reconnected => "ok",
        ReconnectSessionResult::AuthenticationFailed => "authentication_failed",
        ReconnectSessionResult::SessionNotFound => "session_not_found",
        ReconnectSessionResult::StillStarting => "session_starting",
        ReconnectSessionResult::Rejected => "rejected",
    }
}

fn handle_control_request(stream: &mut UnixStream, token: &str) -> Option<ControlRequestCommand> {
    let mut request = read_message::<ControlRequest>(stream).ok()?;
    decode_control_request(&mut request, token)
}

fn decode_control_request(
    request: &mut ControlRequest,
    token: &str,
) -> Option<ControlRequestCommand> {
    if request.token != token {
        if let Some(secret) = request.secret.as_mut() {
            secret.zeroize();
        }
        return None;
    }
    let command = match request.command.as_str() {
        "open_window"
            if request.runner_id.is_none()
                && request.session_id.is_none()
                && request.secret.is_none() =>
        {
            Some(ControlRequestCommand::OpenWindow)
        }
        "reconnect_session" => {
            request
                .runner_id
                .zip(request.session_id)
                .map(
                    |(runner_id, session_id)| ControlRequestCommand::ReconnectSession {
                        runner_id,
                        session_id,
                        secret: request.secret.take(),
                    },
                )
        }
        "set_tab_icon"
            if request.runner_id.is_none()
                && request.session_id.is_none()
                && request.secret.is_none() =>
        {
            let icon = match request.icon.take() {
                Some(icon) => Some(icon.parse().ok()?),
                None => None,
            };
            Some(ControlRequestCommand::SetTabIcon { icon })
        }
        "set_pane_theme"
            if request.runner_id.is_none()
                && request.session_id.is_none()
                && request.secret.is_none() =>
        {
            Some(ControlRequestCommand::SetPaneTheme {
                theme: request.pane_theme.take(),
            })
        }
        "list_pane_themes"
            if request.runner_id.is_none()
                && request.session_id.is_none()
                && request.secret.is_none()
                && request.pane_theme.is_none() =>
        {
            Some(ControlRequestCommand::ListPaneThemes)
        }
        "set_overlay"
            if request.runner_id.is_none()
                && request.session_id.is_none()
                && request.secret.is_none() =>
        {
            let font_size = match request.pane_overlay_font_size.take() {
                Some(name) => Some(OverlayFontSize::parse(&name)?),
                None => None,
            };
            if let Some(hex) = request.pane_overlay_color.as_deref() {
                Rgba::try_from(normalize_overlay_color_hex(hex).as_str()).ok()?;
            }
            Some(ControlRequestCommand::SetPaneOverlay {
                text: request.pane_overlay.take(),
                font_size,
                opacity: request
                    .pane_overlay_opacity
                    .take()
                    .map(|percent| f32::from(percent) / 100.0),
                color: request.pane_overlay_color.take(),
            })
        }
        _ => None,
    };
    if command.is_none()
        && let Some(secret) = request.secret.as_mut()
    {
        secret.zeroize();
    }
    command
}

impl Drop for ProcessControlServer {
    fn drop(&mut self) {
        self.begin_shutdown();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.endpoint_path);
        let _ = fs::remove_file(&self.socket_path);
    }
}

pub(crate) fn request_existing_process_window() -> Result<bool> {
    let directory = crate::background_sessions::session_catalog_dir();
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("reading Zetta process control endpoints"),
    };
    for entry in entries {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("control-") && name.ends_with(".json"))
        {
            continue;
        }
        let endpoint = match fs::read(&path)
            .ok()
            .and_then(|contents| serde_json::from_slice::<ControlEndpoint>(&contents).ok())
        {
            Some(endpoint) if endpoint.version == CONTROL_VERSION => endpoint,
            _ => continue,
        };
        if !process_is_running(endpoint.process_id) {
            let _ = fs::remove_file(path);
            let _ = fs::remove_file(endpoint.socket_path);
            continue;
        }
        if send_open_window_request(&endpoint).unwrap_or(false) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn request_existing_process_tab_icon(icon: Option<IconName>) -> Result<bool> {
    let directory = crate::background_sessions::session_catalog_dir();
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("reading Zetta process control endpoints"),
    };
    for entry in entries {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("control-") && name.ends_with(".json"))
        {
            continue;
        }
        let endpoint = match fs::read(&path)
            .ok()
            .and_then(|contents| serde_json::from_slice::<ControlEndpoint>(&contents).ok())
        {
            Some(endpoint) if endpoint.version == CONTROL_VERSION => endpoint,
            _ => continue,
        };
        if !process_is_running(endpoint.process_id) {
            let _ = fs::remove_file(path);
            let _ = fs::remove_file(endpoint.socket_path);
            continue;
        }
        if send_set_tab_icon_request(&endpoint, icon).unwrap_or(false) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn request_existing_process_pane_theme(theme: Option<String>) -> Result<bool> {
    let directory = crate::background_sessions::session_catalog_dir();
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("reading Zetta process control endpoints"),
    };
    for entry in entries {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("control-") && name.ends_with(".json"))
        {
            continue;
        }
        let endpoint = match fs::read(&path)
            .ok()
            .and_then(|contents| serde_json::from_slice::<ControlEndpoint>(&contents).ok())
        {
            Some(endpoint) if endpoint.version == CONTROL_VERSION => endpoint,
            _ => continue,
        };
        if !process_is_running(endpoint.process_id) {
            let _ = fs::remove_file(path);
            let _ = fs::remove_file(endpoint.socket_path);
            continue;
        }
        if send_set_pane_theme_request(&endpoint, theme.clone()).unwrap_or(false) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn request_existing_process_pane_theme_list() -> Result<Option<Vec<String>>> {
    let directory = crate::background_sessions::session_catalog_dir();
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("reading Zetta process control endpoints"),
    };
    for entry in entries {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("control-") && name.ends_with(".json"))
        {
            continue;
        }
        let endpoint = match fs::read(&path)
            .ok()
            .and_then(|contents| serde_json::from_slice::<ControlEndpoint>(&contents).ok())
        {
            Some(endpoint) if endpoint.version == CONTROL_VERSION => endpoint,
            _ => continue,
        };
        if !process_is_running(endpoint.process_id) {
            let _ = fs::remove_file(path);
            let _ = fs::remove_file(endpoint.socket_path);
            continue;
        }
        if let Some(themes) = send_list_pane_themes_request(&endpoint).unwrap_or(None) {
            return Ok(Some(themes));
        }
    }
    Ok(None)
}

pub(crate) fn request_existing_process_pane_overlay(request: PaneOverlayRequest) -> Result<bool> {
    let directory = crate::background_sessions::session_catalog_dir();
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("reading Zetta process control endpoints"),
    };
    for entry in entries {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("control-") && name.ends_with(".json"))
        {
            continue;
        }
        let endpoint = match fs::read(&path)
            .ok()
            .and_then(|contents| serde_json::from_slice::<ControlEndpoint>(&contents).ok())
        {
            Some(endpoint) if endpoint.version == CONTROL_VERSION => endpoint,
            _ => continue,
        };
        if !process_is_running(endpoint.process_id) {
            let _ = fs::remove_file(path);
            let _ = fs::remove_file(endpoint.socket_path);
            continue;
        }
        if send_set_overlay_request(&endpoint, &request).unwrap_or(false) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn request_reconnect_session(
    process_id: u32,
    runner_id: u64,
    session_id: u64,
    secret: Option<String>,
) -> Result<ReconnectSessionResult> {
    let endpoint_path = control_endpoint_path(process_id);
    let contents = fs::read(&endpoint_path).with_context(|| {
        format!(
            "reading Zetta process control endpoint {}",
            endpoint_path.display()
        )
    })?;
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&contents).context("parsing Zetta process control endpoint")?;
    anyhow::ensure!(
        endpoint.version == CONTROL_VERSION && endpoint.process_id == process_id,
        "Zetta process control endpoint is outdated"
    );
    send_reconnect_session_request(&endpoint, runner_id, session_id, secret)
}

fn send_open_window_request(endpoint: &ControlEndpoint) -> Result<bool> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: "open_window".to_owned(),
            runner_id: None,
            session_id: None,
            secret: None,
            icon: None,
            pane_theme: None,
            pane_overlay: None,
            pane_overlay_font_size: None,
            pane_overlay_opacity: None,
            pane_overlay_color: None,
        },
    )?;
    let response = read_message::<ControlResponse>(&mut stream)?;
    Ok(response.status == "ok")
}

fn send_set_tab_icon_request(endpoint: &ControlEndpoint, icon: Option<IconName>) -> Result<bool> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: "set_tab_icon".to_owned(),
            runner_id: None,
            session_id: None,
            secret: None,
            icon: icon.map(|icon| {
                let name: &'static str = icon.into();
                name.to_owned()
            }),
            pane_theme: None,
            pane_overlay: None,
            pane_overlay_font_size: None,
            pane_overlay_opacity: None,
            pane_overlay_color: None,
        },
    )?;
    let response = read_message::<ControlResponse>(&mut stream)?;
    Ok(response.status == "ok")
}

fn send_set_pane_theme_request(endpoint: &ControlEndpoint, theme: Option<String>) -> Result<bool> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: "set_pane_theme".to_owned(),
            runner_id: None,
            session_id: None,
            secret: None,
            icon: None,
            pane_theme: theme,
            pane_overlay: None,
            pane_overlay_font_size: None,
            pane_overlay_opacity: None,
            pane_overlay_color: None,
        },
    )?;
    let response = read_message::<ControlResponse>(&mut stream)?;
    Ok(response.status == "ok")
}

fn send_list_pane_themes_request(endpoint: &ControlEndpoint) -> Result<Option<Vec<String>>> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: "list_pane_themes".to_owned(),
            runner_id: None,
            session_id: None,
            secret: None,
            icon: None,
            pane_theme: None,
            pane_overlay: None,
            pane_overlay_font_size: None,
            pane_overlay_opacity: None,
            pane_overlay_color: None,
        },
    )?;
    let response = read_message::<ControlResponse>(&mut stream)?;
    Ok((response.status == "ok").then_some(response.themes))
}

fn send_set_overlay_request(
    endpoint: &ControlEndpoint,
    request: &PaneOverlayRequest,
) -> Result<bool> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    write_message(
        &mut stream,
        &ControlRequest {
            token: endpoint.token.clone(),
            command: "set_overlay".to_owned(),
            runner_id: None,
            session_id: None,
            secret: None,
            icon: None,
            pane_theme: None,
            pane_overlay: request.text.clone(),
            pane_overlay_font_size: request
                .font_size
                .map(OverlayFontSize::cli_name)
                .map(str::to_owned),
            pane_overlay_opacity: request.opacity,
            pane_overlay_color: request.color.clone(),
        },
    )?;
    let response = read_message::<ControlResponse>(&mut stream)?;
    Ok(response.status == "ok")
}

fn send_reconnect_session_request(
    endpoint: &ControlEndpoint,
    runner_id: u64,
    session_id: u64,
    mut secret: Option<String>,
) -> Result<ReconnectSessionResult> {
    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    let mut request = ControlRequest {
        token: endpoint.token.clone(),
        command: "reconnect_session".to_owned(),
        runner_id: Some(runner_id),
        session_id: Some(session_id),
        secret: secret.take(),
        icon: None,
        pane_theme: None,
        pane_overlay: None,
        pane_overlay_font_size: None,
        pane_overlay_opacity: None,
        pane_overlay_color: None,
    };
    let result = write_message(&mut stream, &request).and_then(|()| {
        let response = read_message::<ControlResponse>(&mut stream)?;
        Ok(match response.status.as_str() {
            "ok" => ReconnectSessionResult::Reconnected,
            "authentication_failed" => ReconnectSessionResult::AuthenticationFailed,
            "session_not_found" => ReconnectSessionResult::SessionNotFound,
            "session_starting" => ReconnectSessionResult::StillStarting,
            _ => ReconnectSessionResult::Rejected,
        })
    });
    if let Some(secret) = request.secret.as_mut() {
        secret.zeroize();
    }
    result
}

fn read_message<T: DeserializeOwned>(stream: &mut UnixStream) -> Result<T> {
    let mut bytes = Vec::new();
    let mut reader = BufReader::new(stream).take((MAX_CONTROL_MESSAGE_BYTES + 1) as u64);
    reader.read_until(b'\n', &mut bytes)?;
    anyhow::ensure!(
        bytes.last() == Some(&b'\n'),
        "process control message is too long or incomplete"
    );
    bytes.pop();
    serde_json::from_slice(&bytes).context("parsing process control message")
}

fn write_message(stream: &mut UnixStream, message: &impl Serialize) -> Result<()> {
    serde_json::to_writer(&mut *stream, message)?;
    stream.write_all(b"\n")?;
    Ok(())
}

fn random_hex(byte_count: usize) -> Result<String> {
    let mut bytes = vec![0; byte_count];
    getrandom::fill(&mut bytes)?;
    Ok(encode_hex(&bytes))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0xf) as usize] as char);
    }
    encoded
}

fn process_is_running(process_id: u32) -> bool {
    let process_id = Pid::from_u32(process_id);
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[process_id]), true);
    system.process(process_id).is_some()
}

fn control_endpoint_path(process_id: u32) -> PathBuf {
    crate::background_sessions::session_catalog_dir().join(format!("control-{process_id}.json"))
}

fn control_socket_path(endpoint_path: &Path) -> PathBuf {
    endpoint_path.with_extension("sock")
}

fn remove_socket_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("removing stale socket {}", path.display()))
        }
    }
}

fn write_endpoint(path: &Path, endpoint: &ControlEndpoint) -> Result<()> {
    let parent = path.parent().context("control endpoint has no parent")?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension("json.tmp");
    let contents = serde_json::to_vec(endpoint)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?
            .write_all(&contents)?;
    }
    #[cfg(not(unix))]
    fs::write(&temporary, contents)?;
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(temporary, path)?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/process_control.rs"]
mod tests;
