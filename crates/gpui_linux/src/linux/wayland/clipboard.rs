use std::{
    cell::RefCell,
    fs::File,
    io::{ErrorKind, Read, Write},
    os::fd::{AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd},
    rc::Rc,
    time::{Duration, Instant},
};

use anyhow::{Context as _, anyhow};
use calloop::{
    LoopHandle, PostAction, RegistrationToken,
    timer::{TimeoutAction, Timer},
};
use filedescriptor::Pipe;
use futures::channel::oneshot;
use strum::IntoEnumIterator;
use wayland_client::{Connection, Proxy, backend::ObjectId, protocol::wl_data_offer::WlDataOffer};
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1;

use crate::linux::{
    WaylandClientStatePtr,
    platform::{CLIPBOARD_READ_DEADLINE, PIPE_READ_TIMEOUT, read_fd_with_timeout},
};
use gpui::{ClipboardEntry, ClipboardItem, Image, ImageFormat, hash};

/// Text mime types that we'll offer to other programs.
pub(crate) const TEXT_MIME_TYPES: [&str; 3] =
    ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"];
pub(crate) const FILE_LIST_MIME_TYPE: &str = "text/uri-list";

/// Text mime types that we'll accept from other programs.
pub(crate) const ALLOWED_TEXT_MIME_TYPES: [&str; 2] = ["text/plain;charset=utf-8", "UTF8_STRING"];

pub(crate) struct Clipboard {
    connection: Connection,
    loop_handle: LoopHandle<'static, WaylandClientStatePtr>,
    self_mime: String,

    // Internal clipboard
    contents: Option<ClipboardItem>,
    primary_contents: Option<ClipboardItem>,

    // External clipboard
    cached_read: Option<ClipboardItem>,
    current_offer: Option<DataOffer<WlDataOffer>>,
    cached_primary_read: Option<ClipboardItem>,
    current_primary_offer: Option<DataOffer<ZwpPrimarySelectionOfferV1>>,
}

pub(crate) trait ReceiveData {
    fn receive_data(&self, mime_type: String, fd: BorrowedFd<'_>);
}

impl ReceiveData for WlDataOffer {
    fn receive_data(&self, mime_type: String, fd: BorrowedFd<'_>) {
        self.receive(mime_type, fd);
    }
}

impl ReceiveData for ZwpPrimarySelectionOfferV1 {
    fn receive_data(&self, mime_type: String, fd: BorrowedFd<'_>) {
        self.receive(mime_type, fd);
    }
}

#[derive(Clone, Debug)]
/// Wrapper for `WlDataOffer` and `ZwpPrimarySelectionOfferV1`, used to help track mime types.
pub(crate) struct DataOffer<T: ReceiveData> {
    pub inner: T,
    mime_types: Vec<String>,
}

impl<T: ReceiveData> DataOffer<T> {
    pub fn new(offer: T) -> Self {
        Self {
            inner: offer,
            mime_types: Vec::new(),
        }
    }

    pub fn add_mime_type(&mut self, mime_type: String) {
        self.mime_types.push(mime_type)
    }

    fn has_mime_type(&self, mime_type: &str) -> bool {
        self.mime_types.iter().any(|t| t == mime_type)
    }

    fn read_bytes(&self, connection: &Connection, mime_type: &str) -> Option<Vec<u8>> {
        let pipe = Pipe::new().unwrap();
        self.inner.receive_data(mime_type.to_string(), unsafe {
            BorrowedFd::borrow_raw(pipe.write.as_raw_fd())
        });
        let fd = pipe.read;
        drop(pipe.write);

        connection.flush().unwrap();

        match read_fd_with_timeout(fd, PIPE_READ_TIMEOUT) {
            Ok(bytes) => Some(bytes),
            Err(err) => {
                log::error!("error reading clipboard pipe: {err:?}");
                None
            }
        }
    }

    fn text_candidate(&self) -> Option<ReadCandidate> {
        let mime_type = self.mime_types.iter().find(|&mime_type| {
            ALLOWED_TEXT_MIME_TYPES
                .iter()
                .any(|&allowed| allowed == mime_type)
        })?;
        Some(ReadCandidate::Text(mime_type.clone()))
    }

    fn image_candidates(&self) -> impl Iterator<Item = ReadCandidate> + '_ {
        ImageFormat::iter()
            .filter(|format| self.has_mime_type(format.mime_type()))
            .map(ReadCandidate::Image)
    }

    fn read_text(&self, connection: &Connection) -> Option<ClipboardItem> {
        let candidate = self.text_candidate()?;
        let bytes = self.read_bytes(connection, candidate.mime_type())?;
        candidate.decode(bytes)
    }

    fn read_image(&self, connection: &Connection) -> Option<ClipboardItem> {
        for candidate in self.image_candidates() {
            if let Some(bytes) = self.read_bytes(connection, candidate.mime_type()) {
                return candidate.decode(bytes);
            }
        }
        None
    }
}

/// Asks the owner to start sending `mime_type` and returns the end it
/// writes into. The request is sent and flushed before returning, so the
/// owner is already answering this offer whatever replaces it afterwards.
fn start_receive<T: ReceiveData>(
    offer: &T,
    connection: &Connection,
    mime_type: &str,
) -> anyhow::Result<File> {
    let mut pipe = Pipe::new().context("creating the clipboard pipe")?;
    pipe.read
        .set_non_blocking(true)
        .context("making the clipboard pipe non-blocking")?;
    offer.receive_data(mime_type.to_string(), unsafe {
        BorrowedFd::borrow_raw(pipe.write.as_raw_fd())
    });
    drop(pipe.write);
    connection
        .flush()
        .context("flushing the clipboard receive request")?;
    // SAFETY: `into_raw_fd` hands over sole ownership of the descriptor.
    Ok(unsafe { File::from_raw_fd(pipe.read.into_raw_fd()) })
}

fn plan<T: ReceiveData + Proxy + Clone>(
    offer: Option<&DataOffer<T>>,
    cached: &Option<ClipboardItem>,
    own_contents: &Option<ClipboardItem>,
    self_mime: &str,
) -> ReadPlan<T> {
    let Some(offer) = offer else {
        return ReadPlan::Ready(None);
    };
    if let Some(cached) = cached {
        return ReadPlan::Ready(Some(cached.clone()));
    }
    if offer.has_mime_type(self_mime) {
        return ReadPlan::Ready(own_contents.clone());
    }
    ReadPlan::Transfer(OfferTransfer::new(offer))
}

/// One of the formats a read tries, in the order the synchronous read tries
/// them: the preferred text type, then each image type the offer carries.
#[derive(Clone, Debug)]
enum ReadCandidate {
    Text(String),
    Image(ImageFormat),
}

impl ReadCandidate {
    fn mime_type(&self) -> &str {
        match self {
            Self::Text(mime_type) => mime_type,
            Self::Image(format) => format.mime_type(),
        }
    }

    fn decode(&self, bytes: Vec<u8>) -> Option<ClipboardItem> {
        match self {
            Self::Text(_) => {
                let text_content = match String::from_utf8(bytes) {
                    Ok(content) => content,
                    Err(e) => {
                        log::error!("Failed to convert clipboard content to UTF-8: {}", e);
                        return None;
                    }
                };
                // Normalize the text to unix line endings, otherwise
                // copying from eg: firefox inserts a lot of blank
                // lines, and that is super annoying.
                let result = text_content.replace("\r\n", "\n");
                Some(ClipboardItem::new_string(result))
            }
            &Self::Image(format) => {
                let id = hash(&bytes);
                Some(ClipboardItem {
                    entries: vec![ClipboardEntry::Image(Image { format, bytes, id })],
                })
            }
        }
    }
}

/// What an asynchronous read needs, decided on the GUI thread at the moment
/// the read is requested.
pub(crate) enum ReadPlan<T: ReceiveData> {
    /// No transfer: no offer, a cached read of it, or our own contents.
    Ready(Option<ClipboardItem>),
    Transfer(OfferTransfer<T>),
}

/// A read of one specific offer — the one current when the paste was asked
/// for. A replacement offer arriving mid-transfer does not redirect it.
pub(crate) struct OfferTransfer<T: ReceiveData> {
    offer: T,
    candidates: Vec<ReadCandidate>,
}

impl<T: ReceiveData + Proxy> OfferTransfer<T> {
    fn new(offer: &DataOffer<T>) -> Self
    where
        T: Clone,
    {
        Self {
            offer: offer.inner.clone(),
            candidates: offer
                .text_candidate()
                .into_iter()
                .chain(offer.image_candidates())
                .collect(),
        }
    }

    pub fn offer_id(&self) -> ObjectId {
        self.offer.id()
    }

    /// Issues the first receive now, on the calling (GUI) thread, and returns
    /// a future that reads the reply as the owner writes it. A candidate that
    /// fails falls through to the next, as the synchronous read does; that
    /// later receive is only sent while the offer still exists.
    pub fn start<D: 'static>(
        self,
        connection: Connection,
        loop_handle: LoopHandle<'static, D>,
    ) -> impl Future<Output = Option<ClipboardItem>> + 'static
    where
        T: 'static,
    {
        let policy = PipeReadPolicy::CLIPBOARD;
        let mut candidates = self.candidates.into_iter();
        let offer = self.offer;
        let first = candidates
            .next()
            .map(|candidate| receive(&offer, &connection, &loop_handle, candidate, policy));
        async move {
            let mut next = first;
            while let Some(attempt) = next.take() {
                if let Some(item) = attempt.await {
                    return Some(item);
                }
                if !offer.is_alive() {
                    log::debug!("clipboard offer was destroyed before a fallback format");
                    return None;
                }
                next = candidates
                    .next()
                    .map(|candidate| receive(&offer, &connection, &loop_handle, candidate, policy));
            }
            None
        }
    }
}

fn receive<T: ReceiveData, D: 'static>(
    offer: &T,
    connection: &Connection,
    loop_handle: &LoopHandle<'static, D>,
    candidate: ReadCandidate,
    policy: PipeReadPolicy,
) -> impl Future<Output = Option<ClipboardItem>> + use<T, D> {
    let read = start_receive(offer, connection, candidate.mime_type())
        .and_then(|pipe| read_pipe(loop_handle, pipe, policy));
    async move {
        let bytes = match read {
            Ok(read) => read.await,
            Err(err) => Err(err),
        };
        match bytes {
            Ok(bytes) => candidate.decode(bytes),
            Err(err) => {
                log::error!(
                    "error reading clipboard pipe for {}: {err:?}",
                    candidate.mime_type()
                );
                None
            }
        }
    }
}

/// The two limits an asynchronous pipe read runs under.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PipeReadPolicy {
    /// Longest the owner may go without sending anything.
    pub idle: Duration,
    /// Longest the whole transfer may take.
    pub deadline: Duration,
}

impl PipeReadPolicy {
    pub const CLIPBOARD: Self = Self {
        idle: PIPE_READ_TIMEOUT,
        deadline: CLIPBOARD_READ_DEADLINE,
    };
}

struct PipeRead {
    buffer: Vec<u8>,
    started: Instant,
    last_progress: Instant,
    done: Option<oneshot::Sender<anyhow::Result<Vec<u8>>>>,
    pipe_source: Option<RegistrationToken>,
    timer_source: Option<RegistrationToken>,
}

impl PipeRead {
    fn finish<D>(&mut self, result: anyhow::Result<Vec<u8>>, loop_handle: &LoopHandle<'static, D>) {
        // Each source removes itself by its return value; this removes the
        // other one, so neither outlives the read.
        if result.is_ok() {
            if let Some(timer) = self.timer_source.take() {
                loop_handle.remove(timer);
            }
        } else if let Some(pipe) = self.pipe_source.take() {
            loop_handle.remove(pipe);
        }
        if let Some(done) = self.done.take() {
            done.send(result).ok();
        }
    }

    fn next_check(&self, policy: PipeReadPolicy) -> Instant {
        (self.last_progress + policy.idle).min(self.started + policy.deadline)
    }
}

/// Reads `pipe` to its end from the event loop, as data arrives, without
/// blocking the loop's thread. Fails once the owner has sent nothing for
/// `policy.idle`, or once `policy.deadline` has passed since the read began.
pub(crate) fn read_pipe<D: 'static>(
    loop_handle: &LoopHandle<'static, D>,
    pipe: File,
    policy: PipeReadPolicy,
) -> anyhow::Result<impl Future<Output = anyhow::Result<Vec<u8>>> + use<D>> {
    let (done, finished) = oneshot::channel();
    let now = Instant::now();
    let state = Rc::new(RefCell::new(PipeRead {
        buffer: Vec::new(),
        started: now,
        last_progress: now,
        done: Some(done),
        pipe_source: None,
        timer_source: None,
    }));

    let pipe_source = loop_handle
        .insert_source(
            calloop::generic::Generic::new(pipe, calloop::Interest::READ, calloop::Mode::Level),
            {
                let state = state.clone();
                let loop_handle = loop_handle.clone();
                move |_, pipe, _| {
                    let pipe = unsafe { pipe.get_mut() };
                    let mut state = state.borrow_mut();
                    let mut chunk = [0u8; 64 * 1024];
                    loop {
                        match pipe.read(&mut chunk) {
                            Ok(0) => {
                                state.pipe_source = None;
                                let bytes = std::mem::take(&mut state.buffer);
                                state.finish(Ok(bytes), &loop_handle);
                                break Ok(PostAction::Remove);
                            }
                            Ok(len) => {
                                state.buffer.extend_from_slice(&chunk[..len]);
                                state.last_progress = Instant::now();
                            }
                            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                                break Ok(PostAction::Continue);
                            }
                            Err(err) if err.kind() == ErrorKind::Interrupted => {}
                            Err(err) => {
                                state.pipe_source = None;
                                state.finish(Err(err.into()), &loop_handle);
                                break Ok(PostAction::Remove);
                            }
                        }
                    }
                }
            },
        )
        .map_err(|err| anyhow!("registering the clipboard pipe: {err}"))?;
    state.borrow_mut().pipe_source = Some(pipe_source);

    let first_check = state.borrow().next_check(policy);
    let timer_source = loop_handle.insert_source(Timer::from_deadline(first_check), {
        let state = state.clone();
        let loop_handle = loop_handle.clone();
        move |now, _, _| {
            let mut state = state.borrow_mut();
            let failure = if now >= state.started + policy.deadline {
                Some(anyhow!(
                    "clipboard owner did not finish within {:?}",
                    policy.deadline
                ))
            } else if now >= state.last_progress + policy.idle {
                Some(anyhow!(
                    "clipboard owner sent nothing for {:?}",
                    policy.idle
                ))
            } else {
                None
            };
            match failure {
                Some(failure) => {
                    state.timer_source = None;
                    state.finish(Err(failure), &loop_handle);
                    TimeoutAction::Drop
                }
                None => TimeoutAction::ToInstant(state.next_check(policy)),
            }
        }
    });
    match timer_source {
        Ok(timer_source) => state.borrow_mut().timer_source = Some(timer_source),
        Err(err) => {
            loop_handle.remove(pipe_source);
            return Err(anyhow!("registering the clipboard read timer: {err}"));
        }
    }

    Ok(async move {
        finished
            .await
            .unwrap_or_else(|_| Err(anyhow!("clipboard read was abandoned")))
    })
}

impl Clipboard {
    pub fn new(
        connection: Connection,
        loop_handle: LoopHandle<'static, WaylandClientStatePtr>,
    ) -> Self {
        Self {
            connection,
            loop_handle,
            self_mime: format!("pid/{}", std::process::id()),

            contents: None,
            primary_contents: None,

            cached_read: None,
            current_offer: None,
            cached_primary_read: None,
            current_primary_offer: None,
        }
    }

    pub fn set(&mut self, item: ClipboardItem) {
        self.contents = Some(item);
    }

    pub fn set_primary(&mut self, item: ClipboardItem) {
        self.primary_contents = Some(item);
    }

    pub fn set_offer(&mut self, data_offer: Option<DataOffer<WlDataOffer>>) {
        self.cached_read = None;
        self.current_offer = data_offer;
    }

    pub fn set_primary_offer(&mut self, data_offer: Option<DataOffer<ZwpPrimarySelectionOfferV1>>) {
        self.cached_primary_read = None;
        self.current_primary_offer = data_offer;
    }

    pub fn self_mime(&self) -> String {
        self.self_mime.clone()
    }

    pub fn send(&self, _mime_type: String, fd: OwnedFd) {
        if let Some(text) = self.contents.as_ref().and_then(|contents| contents.text()) {
            self.send_internal(fd, text.as_bytes().to_owned());
        }
    }

    pub fn send_primary(&self, _mime_type: String, fd: OwnedFd) {
        if let Some(text) = self
            .primary_contents
            .as_ref()
            .and_then(|contents| contents.text())
        {
            self.send_internal(fd, text.as_bytes().to_owned());
        }
    }

    pub fn read(&mut self) -> Option<ClipboardItem> {
        let offer = self.current_offer.as_ref()?;
        if let Some(cached) = self.cached_read.clone() {
            return Some(cached);
        }

        if offer.has_mime_type(&self.self_mime) {
            return self.contents.clone();
        }

        let item = offer
            .read_text(&self.connection)
            .or_else(|| offer.read_image(&self.connection))?;

        self.cached_read = Some(item.clone());
        Some(item)
    }

    pub fn read_primary(&mut self) -> Option<ClipboardItem> {
        let offer = self.current_primary_offer.as_ref()?;
        if let Some(cached) = self.cached_primary_read.clone() {
            return Some(cached);
        }

        if offer.has_mime_type(&self.self_mime) {
            return self.primary_contents.clone();
        }

        let item = offer
            .read_text(&self.connection)
            .or_else(|| offer.read_image(&self.connection))?;

        self.cached_primary_read = Some(item.clone());
        Some(item)
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// The asynchronous counterpart of [`Clipboard::read`]: the same fast
    /// paths, answered at once, and otherwise the current offer to read.
    pub fn plan_read(&self) -> ReadPlan<WlDataOffer> {
        plan(
            self.current_offer.as_ref(),
            &self.cached_read,
            &self.contents,
            &self.self_mime,
        )
    }

    pub fn plan_read_primary(&self) -> ReadPlan<ZwpPrimarySelectionOfferV1> {
        plan(
            self.current_primary_offer.as_ref(),
            &self.cached_primary_read,
            &self.primary_contents,
            &self.self_mime,
        )
    }

    /// Keeps a finished read for the next paste, unless its offer has since
    /// been replaced — the cache always describes the current offer.
    pub fn cache_read(&mut self, offer: &ObjectId, item: &Option<ClipboardItem>) {
        if let Some(item) = item
            && self
                .current_offer
                .as_ref()
                .is_some_and(|current| current.inner.id() == *offer)
        {
            self.cached_read = Some(item.clone());
        }
    }

    pub fn cache_primary_read(&mut self, offer: &ObjectId, item: &Option<ClipboardItem>) {
        if let Some(item) = item
            && self
                .current_primary_offer
                .as_ref()
                .is_some_and(|current| current.inner.id() == *offer)
        {
            self.cached_primary_read = Some(item.clone());
        }
    }

    fn send_internal(&self, fd: OwnedFd, bytes: Vec<u8>) {
        let mut written = 0;
        self.loop_handle
            .insert_source(
                calloop::generic::Generic::new(
                    File::from(fd),
                    calloop::Interest::WRITE,
                    calloop::Mode::Level,
                ),
                move |_, file, _| {
                    let file = unsafe { file.get_mut() };
                    loop {
                        match file.write(&bytes[written..]) {
                            Ok(n) if written + n == bytes.len() => {
                                written += n;
                                break Ok(PostAction::Remove);
                            }
                            Ok(n) => written += n,
                            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                                break Ok(PostAction::Continue);
                            }
                            Err(_) => break Ok(PostAction::Remove),
                        }
                    }
                },
            )
            .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt as _;
    use std::thread;

    fn pipe() -> (File, filedescriptor::FileDescriptor) {
        let mut pipe = Pipe::new().unwrap();
        pipe.read.set_non_blocking(true).unwrap();
        let read = unsafe { File::from_raw_fd(pipe.read.into_raw_fd()) };
        (read, pipe.write)
    }

    /// Runs the event loop until the read resolves. Every dispatch is
    /// bounded, so a read that never resolved fails at `limit` rather than
    /// hanging the test.
    fn run(
        policy: PipeReadPolicy,
        limit: Duration,
        writer: impl FnOnce(filedescriptor::FileDescriptor) + Send + 'static,
    ) -> anyhow::Result<Vec<u8>> {
        let mut event_loop = calloop::EventLoop::<'static, ()>::try_new().unwrap();
        let (read_end, write_end) = pipe();
        let mut read = Box::pin(read_pipe(&event_loop.handle(), read_end, policy).unwrap());
        let writer = thread::spawn(move || writer(write_end));
        let started = Instant::now();
        let result = loop {
            event_loop
                .dispatch(Some(Duration::from_millis(5)), &mut ())
                .unwrap();
            if let Some(result) = (&mut read).now_or_never() {
                break result;
            }
            assert!(started.elapsed() < limit, "the read never resolved");
        };
        writer.join().unwrap();
        result
    }

    const GENEROUS: PipeReadPolicy = PipeReadPolicy {
        idle: Duration::from_secs(5),
        deadline: Duration::from_secs(20),
    };

    #[test]
    fn a_slow_owner_is_read_to_the_end() {
        let bytes = run(GENEROUS, Duration::from_secs(10), |mut owner| {
            for chunk in [b"slow ".as_slice(), b"clip", b"board"] {
                thread::sleep(Duration::from_millis(30));
                owner.write_all(chunk).unwrap();
            }
        })
        .unwrap();
        assert_eq!(bytes, b"slow clipboard");
    }

    #[test]
    fn a_large_payload_arrives_intact() {
        let payload: Vec<u8> = (0..8 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
        let expected = payload.clone();
        let bytes = run(GENEROUS, Duration::from_secs(10), move |mut owner| {
            owner.write_all(&payload).unwrap();
        })
        .unwrap();
        assert!(
            bytes == expected,
            "payload of {} bytes differed",
            bytes.len()
        );
    }

    #[test]
    fn an_owner_that_exits_without_writing_reads_as_empty() {
        let bytes = run(GENEROUS, Duration::from_secs(10), drop).unwrap();
        assert!(bytes.is_empty());
    }

    #[test]
    fn an_owner_that_goes_quiet_fails_after_the_idle_limit() {
        let policy = PipeReadPolicy {
            idle: Duration::from_millis(50),
            deadline: Duration::from_secs(20),
        };
        let started = Instant::now();
        let err = run(policy, Duration::from_secs(10), |mut owner| {
            owner.write_all(b"partial").unwrap();
            thread::sleep(Duration::from_millis(400));
        })
        .unwrap_err();
        assert!(err.to_string().contains("sent nothing"), "{err}");
        assert!(started.elapsed() >= Duration::from_millis(50));
    }

    #[test]
    fn an_owner_that_keeps_trickling_fails_at_the_deadline() {
        let policy = PipeReadPolicy {
            idle: Duration::from_millis(200),
            deadline: Duration::from_millis(150),
        };
        let err = run(policy, Duration::from_secs(10), |mut owner| {
            // Never quiet for long enough to trip the idle limit; stops once
            // the reader has gone and the pipe breaks.
            for _ in 0..100 {
                if owner.write_all(b".").is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
        })
        .unwrap_err();
        assert!(err.to_string().contains("did not finish"), "{err}");
    }
}
