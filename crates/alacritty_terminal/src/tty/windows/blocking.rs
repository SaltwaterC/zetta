//! Code for running a reader/writer on another thread while driving it through `polling`.

use std::io::prelude::*;
use std::marker::PhantomData;
use std::os::windows::io::AsRawHandle;
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};
use std::{io, thread};

use piper::{Reader, Writer, pipe};
use polling::os::iocp::{CompletionPacket, PollerIocpExt};
use polling::{Event, PollMode, Poller};
use windows_sys::Win32::Foundation::{ERROR_NOT_FOUND, HANDLE};
use windows_sys::Win32::System::IO::CancelIoEx;

use crate::thread::spawn_named;

const CONPTY_READ_BATCH_SIZE: usize = crate::event_loop::READ_BUFFER_SIZE;

struct BatchState {
    generation: Mutex<u64>,
    ready: Condvar,
}

struct Registration {
    interest: Mutex<Option<Interest>>,
    end: PipeEnd,
}

#[derive(Copy, Clone)]
enum PipeEnd {
    Reader,
    Writer,
}

struct Interest {
    /// The event to send about completion.
    event: Event,

    /// The poller to send the event to.
    poller: Arc<Poller>,

    /// The mode that we are in.
    mode: PollMode,
}

/// Coordinates stopping the source reader without racing the synchronous
/// `ReadFile` inside it.
struct ReaderControl {
    stop: std::sync::atomic::AtomicBool,
    reading: std::sync::atomic::AtomicBool,
    finished: std::sync::atomic::AtomicBool,
    state: Mutex<()>,
    changed: Condvar,
    thread: Mutex<Option<thread::Thread>>,
}

/// Marks the interval in which the source is inside `ReadFile`. The control
/// mutex makes the stop check and transition into that interval atomic with
/// the pauser's decision to cancel or wait for completion.
struct CancellableSource<R> {
    source: R,
    control: Arc<ReaderControl>,
}

impl<R: Read> Read for CancellableSource<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let _state = self.control.state.lock().unwrap();
        if self.control.stop.load(std::sync::atomic::Ordering::Acquire) {
            return Ok(0);
        }
        self.control.reading.store(true, std::sync::atomic::Ordering::Release);
        self.control.changed.notify_all();
        drop(_state);

        let result = self.source.read(buf);
        self.control.reading.store(false, std::sync::atomic::Ordering::Release);
        self.control.changed.notify_all();
        result
    }
}

/// The source reader and its pipe writer, which are moved to the reader thread
/// together. Keeping the writer with the source is important when the thread is
/// paused: the `Reader` may still contain bytes that must be drained before the
/// source is handed back to another owner.
struct ReaderThread<R> {
    control: Arc<ReaderControl>,
    join: thread::JoinHandle<(R, Writer)>,
}

/// Poll a reader in another thread.
pub struct UnblockedReader<R> {
    /// The event to send about completion.
    interest: Arc<Registration>,

    /// The pipe that we are reading from.
    pipe: Reader,

    /// The source and pipe writer stay here until somebody actually registers
    /// or reads this wrapper. A daemon can therefore hold a duplicate of a
    /// ConPTY output pipe without consuming bytes while another process has
    /// exclusive ownership of the console.
    source: Option<R>,
    writer: Option<Writer>,

    /// Is this the first time registering?
    first_register: bool,

    /// Whether the source reader thread has been started.
    reader_thread: Option<ReaderThread<R>>,

    /// A paused reader keeps its source and pipe writer here, but must not
    /// start another source read until its owner resumes it. This is the
    /// ownership boundary used by the Windows multiplexer when a detached
    /// pane is handed back to an exclusive client.
    paused: bool,

    /// The source handle remains valid while the reader thread owns `source`,
    /// and lets `pause` cancel a synchronous `ReadFile` from another thread.
    source_handle: usize,

    /// Notification used to coalesce small kernel reads before parsing.
    batch: Arc<BatchState>,
}

impl<R: Read + Send + AsRawHandle + 'static> UnblockedReader<R> {
    /// Spawn a new unblocked reader.
    pub fn new(source: R, pipe_capacity: usize) -> Self {
        let source_handle = source.as_raw_handle();
        // Create a new pipe.
        let (reader, writer) = pipe(pipe_capacity);
        let interest = Arc::new(Registration {
            interest: Mutex::<Option<Interest>>::new(None),
            end: PipeEnd::Reader,
        });
        let batch = Arc::new(BatchState { generation: Mutex::new(0), ready: Condvar::new() });

        Self {
            interest,
            pipe: reader,
            source: Some(source),
            writer: Some(writer),
            first_register: true,
            reader_thread: None,
            paused: false,
            source_handle: source_handle as usize,
            batch,
        }
    }

    /// Starts the source reader on first use.
    ///
    /// `Pty::attach` is also used by the multiplexer daemon. It must be able
    /// to keep a pipe ready for detached/shared mode without draining it from
    /// underneath an exclusive client, so construction cannot start this
    /// thread eagerly.
    fn start(&mut self) {
        if self.paused || self.reader_thread.is_some() {
            return;
        }
        let source = self.source.take().expect("reader source is present before start");
        let mut writer = self.writer.take().expect("reader writer is present before start");
        let reader_batch = self.batch.clone();
        let control = Arc::new(ReaderControl {
            stop: std::sync::atomic::AtomicBool::new(false),
            reading: std::sync::atomic::AtomicBool::new(false),
            finished: std::sync::atomic::AtomicBool::new(false),
            state: Mutex::new(()),
            changed: Condvar::new(),
            thread: Mutex::new(None),
        });
        let control_for_reader = control.clone();
        let join = spawn_named("alacritty-tty-reader-thread", move || {
            *control_for_reader.thread.lock().unwrap() = Some(thread::current());
            let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
            let mut context = Context::from_waker(&waker);
            let mut source = CancellableSource { source, control: control_for_reader.clone() };

            let (source, writer) = loop {
                if control_for_reader.stop.load(std::sync::atomic::Ordering::Acquire) {
                    break (source, writer);
                }
                match writer.poll_fill(&mut context, &mut source) {
                    Poll::Ready(Ok(0)) => break (source, writer),
                    Poll::Ready(Ok(_)) => {
                        let mut generation = reader_batch.generation.lock().unwrap();
                        *generation = generation.wrapping_add(1);
                        reader_batch.ready.notify_all();
                        continue;
                    },
                    Poll::Ready(Err(error)) if error.kind() == io::ErrorKind::Interrupted => {
                        continue;
                    },
                    Poll::Ready(Err(error)) => {
                        if control_for_reader.stop.load(std::sync::atomic::Ordering::Acquire) {
                            break (source, writer);
                        }
                        log::error!("error writing to pipe: {error}");
                        break (source, writer);
                    },
                    Poll::Pending => thread::park(),
                }
            };
            control_for_reader.finished.store(true, std::sync::atomic::Ordering::Release);
            control_for_reader.changed.notify_all();
            (source.source, writer)
        });
        self.reader_thread = Some(ReaderThread { control, join });
    }

    /// Pauses source reads and returns the source to this wrapper.
    ///
    /// `deregister` only removes the notification; it does not stop the
    /// source thread. That distinction is harmless for an ordinary terminal,
    /// but fatal for a ConPTY shared by a daemon and an attached client: the
    /// daemon could continue consuming bytes after the client received the
    /// console handles. Cancel the synchronous Windows read and join the
    /// thread so there is exactly one active reader at the handover boundary.
    pub fn pause(&mut self) -> io::Result<()> {
        self.paused = true;
        let Some(reader_thread) = self.reader_thread.take() else {
            return Ok(());
        };

        reader_thread.control.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(thread) = reader_thread.control.thread.lock().unwrap().as_ref().cloned() {
            thread.unpark();
        }

        let mut state = reader_thread.control.state.lock().unwrap();
        while !reader_thread.control.finished.load(std::sync::atomic::Ordering::Acquire)
            && !reader_thread.control.reading.load(std::sync::atomic::Ordering::Acquire)
        {
            state = reader_thread.control.changed.wait(state).unwrap();
        }
        let reading = reader_thread.control.reading.load(std::sync::atomic::Ordering::Acquire);
        drop(state);

        // `AnonRead` uses synchronous ReadFile. CancelIoEx is explicitly
        // cross-thread and wakes that call without closing the handle, so the
        // reader can return both the source and its still-live piper writer.
        if reading {
            let cancelled = unsafe { CancelIoEx(self.source_handle as HANDLE, std::ptr::null()) };
            if cancelled == 0 {
                let error = io::Error::last_os_error();
                // ERROR_NOT_FOUND means the synchronous read completed between
                // the state check and CancelIoEx. The stop flag prevents a new
                // read, so this is not a pause failure.
                if error.raw_os_error() != Some(ERROR_NOT_FOUND as i32) {
                    log::debug!("cancelling the Windows PTY reader returned {error}");
                }
            }
        }

        let (source, writer) = reader_thread
            .join
            .join()
            .map_err(|_| io::Error::other("the Windows PTY reader thread panicked"))?;
        self.source = Some(source);
        self.writer = Some(writer);
        Ok(())
    }

    /// Resumes source reads after a handover has returned ownership to the
    /// daemon. The next normal read starts the thread lazily.
    pub fn resume(&mut self) {
        self.paused = false;
    }

    /// Drains only bytes already copied into the internal pipe. Unlike
    /// `try_read`, this never starts a source reader, which is needed while a
    /// handover is being completed.
    pub fn try_read_buffered(&mut self, buf: &mut [u8]) -> usize {
        let waker = Waker::from(self.interest.clone());
        match self.pipe.poll_drain_bytes(&mut Context::from_waker(&waker), buf) {
            Poll::Pending => 0,
            Poll::Ready(n) => n,
        }
    }

    /// Register interest in the reader.
    pub fn register(&mut self, poller: &Arc<Poller>, event: Event, mode: PollMode) {
        let mut interest = self.interest.interest.lock().unwrap();
        *interest = Some(Interest { event, poller: poller.clone(), mode });
        drop(interest);
        self.start();

        // Send the event to start off with if we have any data.
        if (!self.pipe.is_empty() && event.readable) || self.first_register {
            self.first_register = false;
            poller.post(CompletionPacket::new(event)).ok();
        }
    }

    /// Deregister interest in the reader.
    pub fn deregister(&self) {
        let mut interest = self.interest.interest.lock().unwrap();
        *interest = None;
    }

    /// Try to read from the reader.
    pub fn try_read(&mut self, buf: &mut [u8]) -> usize {
        self.start();
        if !self.pipe.is_empty() && self.pipe.len() < CONPTY_READ_BATCH_SIZE {
            let deadline = Instant::now() + Duration::from_millis(2);
            let mut generation = self.batch.generation.lock().unwrap();
            while self.pipe.len() < CONPTY_READ_BATCH_SIZE {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                let current_generation = *generation;
                let (next_generation, timeout) = self
                    .batch
                    .ready
                    .wait_timeout_while(generation, remaining, |generation| {
                        *generation == current_generation
                    })
                    .unwrap();
                generation = next_generation;
                if timeout.timed_out() {
                    break;
                }
            }
        }

        let waker = Waker::from(self.interest.clone());

        match self.pipe.poll_drain_bytes(&mut Context::from_waker(&waker), buf) {
            Poll::Pending => 0,
            Poll::Ready(n) => n,
        }
    }

    /// Register the IOCP notification for the next byte without consuming it.
    pub fn rearm(&mut self) {
        self.start();
        let waker = Waker::from(self.interest.clone());
        if matches!(self.pipe.poll(&mut Context::from_waker(&waker)), Poll::Ready(true)) {
            Wake::wake_by_ref(&self.interest);
        }
    }
}

impl<R: Read + Send + AsRawHandle + 'static> Read for UnblockedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        Ok(self.try_read(buf))
    }
}

/// Poll a writer in another thread.
pub struct UnblockedWriter<W> {
    /// The interest to send about completion.
    interest: Arc<Registration>,

    /// The pipe that we are writing to.
    pipe: Writer,

    /// We logically own the writer, but we don't actually use it.
    _reader: PhantomData<W>,
}

impl<W: Write + Send + 'static> UnblockedWriter<W> {
    /// Spawn a new unblocked writer.
    pub fn new(mut sink: W, pipe_capacity: usize) -> Self {
        // Create a new pipe.
        let (mut reader, writer) = pipe(pipe_capacity);
        let interest = Arc::new(Registration {
            interest: Mutex::<Option<Interest>>::new(None),
            end: PipeEnd::Writer,
        });

        // Spawn the writer thread.
        spawn_named("alacritty-tty-writer-thread", move || {
            let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
            let mut context = Context::from_waker(&waker);

            loop {
                // Write from the pipe into the writer.
                match reader.poll_drain(&mut context, &mut sink) {
                    Poll::Ready(Ok(0)) => {
                        // Either the pipe is closed or the writer is full.
                        // In any case, we are done.
                        return;
                    },

                    Poll::Ready(Ok(_)) => {
                        // Keep writing.
                        continue;
                    },

                    Poll::Ready(Err(e)) if e.kind() == io::ErrorKind::Interrupted => {
                        // We were interrupted; continue.
                        continue;
                    },

                    Poll::Ready(Err(e)) => {
                        log::error!("error writing to pipe: {}", e);
                        return;
                    },

                    Poll::Pending => {
                        // We are now waiting on the other end to advance. Park the
                        // thread until they do.
                        thread::park();
                    },
                }
            }
        });

        Self { interest, pipe: writer, _reader: PhantomData }
    }

    /// Register interest in the writer.
    pub fn register(&self, poller: &Arc<Poller>, event: Event, mode: PollMode) {
        let mut interest = self.interest.interest.lock().unwrap();
        *interest = Some(Interest { event, poller: poller.clone(), mode });

        // Send the event to start off with if we have room for data.
        if !self.pipe.is_full() && event.writable {
            poller.post(CompletionPacket::new(event)).ok();
        }
    }

    /// Deregister interest in the writer.
    pub fn deregister(&self) {
        let mut interest = self.interest.interest.lock().unwrap();
        *interest = None;
    }

    /// Try to write to the writer.
    pub fn try_write(&mut self, buf: &[u8]) -> usize {
        let waker = Waker::from(self.interest.clone());

        match self.pipe.poll_fill_bytes(&mut Context::from_waker(&waker), buf) {
            Poll::Pending => 0,
            Poll::Ready(n) => n,
        }
    }
}

impl<W: Write + Send + 'static> Write for UnblockedWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(self.try_write(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        // Nothing to flush.
        Ok(())
    }
}

struct ThreadWaker(thread::Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

impl Wake for Registration {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        let mut interest_lock = self.interest.lock().unwrap();
        if let Some(interest) = interest_lock.as_ref() {
            // Send the event to the poller.
            let send_event = match self.end {
                PipeEnd::Reader => interest.event.readable,
                PipeEnd::Writer => interest.event.writable,
            };

            if send_event {
                interest.poller.post(CompletionPacket::new(interest.event)).ok();

                // Clear the event if we're in oneshot mode.
                if matches!(interest.mode, PollMode::Oneshot | PollMode::EdgeOneshot) {
                    *interest_lock = None;
                }
            }
        }
    }
}
