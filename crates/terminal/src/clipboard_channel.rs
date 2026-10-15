//! Host clipboard I/O for requests arriving from an interactive child.
//!
//! The sibling helpers own platform-specific clipboard behavior. The guard
//! keeps them from probing their own terminal while serving a request.
//!
//! Requests are printed by the pane, so serving one must not stall the
//! terminal's event path. [`RemoteClipboard`] queues each frame, in arrival
//! order, for one task that runs the zclip host on the background executor and
//! writes the answer back to the pty. The queue is bounded, and a helper that
//! has not finished within [`HELPER_TIMEOUT`] is killed.
//!
//! The tab's copy and paste permissions are shared with that task and read
//! when a frame is served, not when it is queued. Withdrawing one wakes the
//! task to end the transfers of that kind it holds, and the task also wakes
//! when a transfer expires, so a captured clipboard is erased promptly rather
//! than at the pane's next frame. The copy permission also governs OSC 52
//! clipboard writes, which the terminal checks through
//! [`RemoteClipboard::copy_allowed`].

use anyhow::{Context as _, Result, bail, ensure};
use futures::future::{Either, select};
use gpui::{Context, Task};
use std::{
    io::{Read as _, Write as _},
    pin::pin,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use zclip::{
    host::{Host, Permissions},
    protocol::{Frame, Message},
};

use crate::Terminal;

/// Longest a clipboard helper may run. Reading or writing the desktop
/// clipboard takes milliseconds; this only bounds one that hangs, which would
/// otherwise hold a background thread and every later request behind it.
const HELPER_TIMEOUT: Duration = Duration::from_secs(10);
const HELPER_POLL_INTERVAL: Duration = Duration::from_millis(5);
/// Frames waiting for the host. A helper keeps at most a window of sixteen
/// chunks in flight, so this only fills when output floods the channel; the
/// overflow is answered with an error rather than queued without bound.
const QUEUED_REQUESTS: usize = 64;

#[derive(Clone, Copy)]
struct Helpers {
    copy: fn(&str) -> Result<()>,
    paste: fn() -> Result<Option<String>>,
}

impl Default for Helpers {
    fn default() -> Self {
        Self { copy, paste }
    }
}

enum Request {
    Frame(Frame),
    /// Paste was disallowed: end the pastes in flight now.
    RevokePaste,
    /// Copy was disallowed: end the copies in flight now.
    RevokeCopy,
}

/// The tab's grants, shared between the terminal and its worker.
#[derive(Clone)]
struct SharedPermissions {
    copy: Arc<AtomicBool>,
    paste: Arc<AtomicBool>,
}

impl SharedPermissions {
    fn load(&self) -> Permissions {
        Permissions {
            copy: self.copy.load(Ordering::SeqCst),
            paste: self.paste.load(Ordering::SeqCst),
        }
    }
}

/// One terminal's clipboard host, started on its first request.
pub(crate) struct RemoteClipboard {
    worker: Option<(async_channel::Sender<Request>, Task<()>)>,
    helpers: Helpers,
    permissions: SharedPermissions,
}

impl Default for RemoteClipboard {
    /// Copy allowed and paste withheld, as a tab starts: writing the clipboard
    /// is what a pane has always been able to do, reading it is opted into.
    fn default() -> Self {
        Self {
            worker: None,
            helpers: Helpers::default(),
            permissions: SharedPermissions {
                copy: Arc::new(AtomicBool::new(true)),
                paste: Arc::new(AtomicBool::new(false)),
            },
        }
    }
}

impl RemoteClipboard {
    /// Allows or disallows reading the host clipboard. Disallowing it also
    /// ends, and erases, every paste the host is part way through.
    pub(crate) fn set_paste_allowed(&mut self, allowed: bool) {
        let was_allowed = self.permissions.paste.swap(allowed, Ordering::SeqCst);
        if was_allowed && !allowed {
            self.wake(Request::RevokePaste);
        }
    }

    /// Allows or disallows writing the host clipboard. Disallowing it also
    /// ends every copy the host is part way through, so none of them lands.
    pub(crate) fn set_copy_allowed(&mut self, allowed: bool) {
        let was_allowed = self.permissions.copy.swap(allowed, Ordering::SeqCst);
        if was_allowed && !allowed {
            self.wake(Request::RevokeCopy);
        }
    }

    /// Whether the pane may write the host clipboard, by zcopy or OSC 52.
    pub(crate) fn copy_allowed(&self) -> bool {
        self.permissions.copy.load(Ordering::SeqCst)
    }

    fn wake(&self, revocation: Request) {
        if let Some((requests, _)) = &self.worker {
            // A full queue needs no wakeup: the frames in it are served with
            // the permission withdrawn, and serving any frame so ends the
            // transfers it covered.
            let _ = requests.try_send(revocation);
        }
    }

    /// Queues `frame` for the host. The answer is written to the pty later;
    /// what this returns is an answer due now, for a frame the full queue
    /// turned away.
    pub(crate) fn handle(&mut self, frame: Frame, cx: &mut Context<Terminal>) -> Option<Frame> {
        let helpers = self.helpers;
        let permissions = &self.permissions;
        let (requests, _) = self
            .worker
            .get_or_insert_with(|| start_worker(helpers, permissions.clone(), cx));
        let id = frame.id;
        let cancel = matches!(frame.message, Message::Error(_));
        let error = requests.try_send(Request::Frame(frame)).err()?;
        log::warn!("dropping a remote clipboard request: {error}");
        (!cancel).then(|| Frame {
            id,
            message: Message::Error("clipboard host is busy".into()),
        })
    }
}

/// What the worker does next: serve a request, or expire the transfers whose
/// time is up.
enum Work {
    Request(Request),
    Expire,
}

fn start_worker(
    helpers: Helpers,
    permissions: SharedPermissions,
    cx: &mut Context<Terminal>,
) -> (async_channel::Sender<Request>, Task<()>) {
    let (sender, receiver) = async_channel::bounded::<Request>(QUEUED_REQUESTS);
    let task = cx.spawn(async move |terminal, cx| {
        let mut host = Host::default();
        loop {
            let work = match host.next_expiry() {
                None => receiver.recv().await.map(Work::Request),
                Some(deadline) => {
                    let expiry = cx
                        .background_executor()
                        .timer(deadline.saturating_duration_since(Instant::now()));
                    match select(pin!(receiver.recv()), expiry).await {
                        Either::Left((request, _)) => request.map(Work::Request),
                        Either::Right(_) => Ok(Work::Expire),
                    }
                }
            };
            let Ok(work) = work else {
                break;
            };
            let permissions = permissions.clone();
            let (returned, response) = cx
                .background_executor()
                .spawn(async move {
                    let response = serve(&mut host, work, &permissions, helpers);
                    (host, response)
                })
                .await;
            host = returned;
            let Some(response) = response else {
                continue;
            };
            let written = terminal.update(cx, |terminal, _| {
                terminal.write_to_pty(response.encode());
            });
            if written.is_err() {
                break;
            }
        }
    });
    (sender, task)
}

fn serve(
    host: &mut Host,
    work: Work,
    permissions: &SharedPermissions,
    helpers: Helpers,
) -> Option<Frame> {
    let frame = match work {
        Work::Request(Request::Frame(frame)) => frame,
        Work::Request(Request::RevokePaste) => {
            host.revoke_paste();
            return None;
        }
        Work::Request(Request::RevokeCopy) => {
            host.revoke_copy();
            return None;
        }
        Work::Expire => {
            host.expire();
            return None;
        }
    };
    let granted = permissions.load();
    let mut response = host.handle(frame, granted, helpers.copy, helpers.paste);
    // Reading the clipboard can take the helper a while, and a revocation that
    // lands meanwhile holds for the chunk it produced, too.
    if granted.paste && !permissions.paste.load(Ordering::SeqCst) {
        host.revoke_paste();
        if let Some(Frame {
            message: message @ Message::Data { .. },
            ..
        }) = &mut response
        {
            *message = Message::Error(zclip::host::PASTE_DISABLED.into());
        }
    }
    response
}

fn sibling(name: &str) -> Result<std::path::PathBuf> {
    let mut path = std::env::current_exe().context("locating Zetta executable")?;
    path.set_file_name(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    Ok(path)
}

fn helper(name: &str) -> Result<Command> {
    let mut command = Command::new(sibling(name)?);
    command.env("ZCLIP_HOST_BACKEND", "1");
    Ok(command)
}

fn copy(text: &str) -> Result<()> {
    run_helper(
        helper("zcopy")?,
        Some(text.as_bytes().to_vec()),
        None,
        HELPER_TIMEOUT,
    )
    .context("host zcopy")?;
    Ok(())
}

fn paste() -> Result<Option<String>> {
    let mut command = helper("zpaste")?;
    command.stderr(Stdio::null());
    let output = run_helper(
        command,
        None,
        Some(zclip::host::MAX_TRANSFER_BYTES),
        HELPER_TIMEOUT,
    )
    .context("host zpaste")?;
    let text = String::from_utf8(output).context("host clipboard is not UTF-8 text")?;
    Ok(Some(text))
}

/// Runs a helper to completion within `timeout`, killing it otherwise. Its
/// input is written and its output read on threads of their own, so a helper
/// that stops reading or writing cannot hold the caller past the deadline.
/// With `max_output`, standard output is captured up to that many bytes, and
/// more than that is an error.
fn run_helper(
    mut command: Command,
    input: Option<Vec<u8>>,
    max_output: Option<usize>,
    timeout: Duration,
) -> Result<Vec<u8>> {
    let deadline = Instant::now() + timeout;
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(if max_output.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    let mut child = command.spawn().context("starting")?;
    let written = input.map(|input| {
        let stdin = child.stdin.take();
        background(move || {
            stdin
                .context("no standard input")?
                .write_all(&input)
                .context("sending text")
        })
    });
    let output = max_output.map(|max_output| {
        let stdout = child.stdout.take();
        background(move || {
            let mut output = Vec::new();
            // One byte past the limit tells a full read from a larger one;
            // dropping the pipe then ends a helper that keeps writing.
            stdout
                .context("no standard output")?
                .take(max_output as u64 + 1)
                .read_to_end(&mut output)
                .context("reading output")?;
            ensure!(output.len() <= max_output, "output is too large");
            Ok(output)
        })
    });
    let status = wait_until(&mut child, deadline)?;
    if let Some(written) = written {
        finished(&written, deadline)?;
    }
    ensure!(status.success(), "failed: {status}");
    match output {
        Some(output) => finished(&output, deadline),
        None => Ok(Vec::new()),
    }
}

fn background<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> mpsc::Receiver<Result<T>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let _ = sender.send(work());
    });
    receiver
}

fn finished<T>(work: &mpsc::Receiver<Result<T>>, deadline: Instant) -> Result<T> {
    work.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| anyhow::anyhow!("did not finish in time"))?
}

fn wait_until(child: &mut Child, deadline: Instant) -> Result<std::process::ExitStatus> {
    loop {
        if let Some(status) = child.try_wait().context("waiting")? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("did not finish in time and was stopped");
        }
        thread::sleep(HELPER_POLL_INTERVAL);
    }
}

#[cfg(test)]
#[path = "tests/clipboard_channel.rs"]
mod tests;
