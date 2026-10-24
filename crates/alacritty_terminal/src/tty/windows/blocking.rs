//! Code for running a reader/writer on another thread while driving it through `polling`.

use std::io::prelude::*;
use std::marker::PhantomData;
use std::os::windows::io::AsRawHandle;
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;
use std::{io, thread};

use piper::{Reader, Writer, pipe};
use polling::os::iocp::{CompletionPacket, PollerIocpExt};
use polling::{Event, PollMode, Poller};
use windows_sys::Win32::Foundation::{ERROR_NOT_FOUND, HANDLE};
use windows_sys::Win32::System::IO::CancelIoEx;

use crate::thread::spawn_named;

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
    finished: std::sync::atomic::AtomicBool,
    /// The source reached the end of its output, as opposed to being stopped.
    /// Published before `finished`.
    ended: std::sync::atomic::AtomicBool,
    state: Mutex<()>,
    changed: Condvar,
    thread: Mutex<Option<thread::Thread>>,
}

/// Publishes the source reader's exit when the reader thread leaves, whether
/// it returns or unwinds. `pause` cancels the source handle until it sees
/// `finished`, so an exit that skipped this would leave it cancelling a closed
/// handle value, and with it whatever I/O Windows later reuses that value for.
struct ReaderExit {
    control: Arc<ReaderControl>,
    interest: Arc<Registration>,
    /// The source reached the end of its output, as opposed to being stopped.
    ended: bool,
}

impl Drop for ReaderExit {
    fn drop(&mut self) {
        // A panic drops the source, so nothing more will come out of it.
        let ended = self.ended || thread::panicking();
        {
            // Pair the finished predicate with the pauser's condvar wait. Without
            // this lock, completion could notify between its check and wait.
            let _state = self.control.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            self.control.ended.store(ended, std::sync::atomic::Ordering::Release);
            self.control.finished.store(true, std::sync::atomic::Ordering::Release);
            self.control.changed.notify_all();
        }
        // No byte follows to wake a drain waiting on the end of output.
        if ended {
            Wake::wake_by_ref(&self.interest);
        }
    }
}

/// Stops new reads after a pause. Cancellation must also cover the interval
/// between this check and entering `ReadFile`, so the pauser retries it until
/// the source thread has returned.
struct CancellableSource<R> {
    source: R,
    control: Arc<ReaderControl>,
}

impl<R: Read> Read for CancellableSource<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.control.stop.load(std::sync::atomic::Ordering::Acquire) {
            return Ok(0);
        }
        self.source.read(buf)
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

        Self {
            interest,
            pipe: reader,
            source: Some(source),
            writer: Some(writer),
            first_register: true,
            reader_thread: None,
            paused: false,
            source_handle: source_handle as usize,
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
        let control = Arc::new(ReaderControl {
            stop: std::sync::atomic::AtomicBool::new(false),
            finished: std::sync::atomic::AtomicBool::new(false),
            ended: std::sync::atomic::AtomicBool::new(false),
            state: Mutex::new(()),
            changed: Condvar::new(),
            thread: Mutex::new(None),
        });
        let control_for_reader = control.clone();
        let interest = self.interest.clone();
        let join = spawn_named("alacritty-tty-reader-thread", move || {
            let mut source = CancellableSource { source, control: control_for_reader.clone() };
            // Declared after the source so that an unwind publishes the exit
            // before it drops the source and closes the handle `pause` cancels.
            let mut exit =
                ReaderExit { control: control_for_reader.clone(), interest, ended: false };
            *control_for_reader.thread.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(thread::current());
            let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
            let mut context = Context::from_waker(&waker);

            let (source, writer) = loop {
                if control_for_reader.stop.load(std::sync::atomic::Ordering::Acquire) {
                    break (source, writer);
                }
                match writer.poll_fill(&mut context, &mut source) {
                    Poll::Ready(Ok(0)) => {
                        // A stopped source also reads as empty.
                        exit.ended =
                            !control_for_reader.stop.load(std::sync::atomic::Ordering::Acquire);
                        break (source, writer);
                    },
                    Poll::Ready(Ok(_)) => continue,
                    Poll::Ready(Err(error)) if error.kind() == io::ErrorKind::Interrupted => {
                        continue;
                    },
                    Poll::Ready(Err(error)) => {
                        if control_for_reader.stop.load(std::sync::atomic::Ordering::Acquire) {
                            break (source, writer);
                        }
                        // The console closing its end is the end of its output.
                        if error.kind() == io::ErrorKind::BrokenPipe {
                            exit.ended = true;
                            break (source, writer);
                        }
                        log::error!("error writing to pipe: {error}");
                        break (source, writer);
                    },
                    Poll::Pending => thread::park(),
                }
            };
            drop(exit);
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
        if let Some(thread) = reader_thread
            .control
            .thread
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .cloned()
        {
            thread.unpark();
        }

        let mut state =
            reader_thread.control.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        // `AnonRead` uses synchronous ReadFile. CancelIoEx is explicitly
        // cross-thread and wakes that call without closing the handle, so the
        // reader can return both the source and its still-live piper writer.
        // `ReaderExit` sets `finished` even on a panic. A thread that ended
        // before creating it never will, and has already closed the handle.
        while !reader_thread.control.finished.load(std::sync::atomic::Ordering::Acquire)
            && !reader_thread.join.is_finished()
        {
            let cancelled = unsafe { CancelIoEx(self.source_handle as HANDLE, std::ptr::null()) };
            if cancelled == 0 {
                let error = io::Error::last_os_error();
                // ERROR_NOT_FOUND can also precede the native read: the source
                // may have passed its stop check without entering ReadFile yet.
                // Retry cancellation after releasing the completion mutex.
                if error.raw_os_error() != Some(ERROR_NOT_FOUND as i32) {
                    log::debug!("cancelling the Windows PTY reader returned {error}");
                }
            }
            (state, _) = reader_thread
                .control
                .changed
                .wait_timeout(state, Duration::from_millis(1))
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        drop(state);

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

    /// Whether the source reached the end of its output on its own, rather
    /// than being paused, and every byte it read has been drained.
    pub fn source_ended(&self) -> bool {
        self.reader_thread
            .as_ref()
            .is_some_and(|thread| thread.control.ended.load(std::sync::atomic::Ordering::Acquire))
            && self.pipe.is_empty()
    }

    /// Register interest in the reader.
    pub fn register(&mut self, poller: &Arc<Poller>, event: Event, mode: PollMode) {
        let mut interest =
            self.interest.interest.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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
        let mut interest =
            self.interest.interest.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        *interest = None;
    }

    /// Try to read from the reader.
    pub fn try_read(&mut self, buf: &mut [u8]) -> usize {
        self.start();

        // Publish available output immediately. The source reader and parser
        // already overlap; waiting for a refill delays both without saving I/O.

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
        let mut interest =
            self.interest.interest.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        *interest = Some(Interest { event, poller: poller.clone(), mode });

        // Send the event to start off with if we have room for data.
        if !self.pipe.is_full() && event.writable {
            poller.post(CompletionPacket::new(event)).ok();
        }
    }

    /// Deregister interest in the writer.
    pub fn deregister(&self) {
        let mut interest =
            self.interest.interest.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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
        let mut interest_lock =
            self.interest.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn buffered_reader(capacity: usize, bytes: &[u8]) -> UnblockedReader<miow::pipe::AnonRead> {
        let (source, _output) = miow::pipe::anonymous(0).unwrap();
        let mut reader = UnblockedReader::new(source, capacity);
        // These tests supply the internal pipe themselves, without starting
        // the source thread or depending on the kernel's refill timing.
        reader.paused = true;
        // Not poll_fill_bytes: given a context, piper yields at random one
        // fill in a hundred even when there is room.
        assert_eq!(reader.writer.as_mut().unwrap().try_fill(bytes), bytes.len());
        reader
    }

    fn read_available(
        reader: &mut UnblockedReader<miow::pipe::AnonRead>,
        bytes: &mut [u8],
    ) -> usize {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let count = reader.try_read(bytes);
            if count != 0 || reader.pipe.is_empty() {
                return count;
            }
            assert!(Instant::now() < deadline, "buffered bytes did not become readable");
            // piper's cooperative yield is not an empty pipe.
            thread::yield_now();
        }
    }

    #[test]
    fn available_reads_do_not_wait_for_unrequested_bytes() {
        let mut reader = buffered_reader(crate::event_loop::READ_BUFFER_SIZE, &[b'x'; 128]);
        let (sent, received) = mpsc::channel();
        let worker = thread::spawn(move || {
            for _ in 0..128 {
                let mut byte = [0];
                // piper can yield cooperatively even while bytes remain.
                while reader.try_read(&mut byte) == 0 {
                    thread::yield_now();
                }
                assert_eq!(byte, [b'x']);
            }
            sent.send(()).unwrap();
        });
        // The old refill wait costs at least 256 ms across these reads. A
        // 100 ms budget leaves ample scheduling slack for immediate reads.
        let result = received.recv_timeout(Duration::from_millis(100));
        worker.join().unwrap();
        result.expect("available bytes must not incur a refill wait per read");
    }

    #[test]
    fn short_output_and_eof_preserve_every_byte() {
        let bytes = b"\x1b[31mfragmented\xe6\xbc\xa2\r\n";
        let mut reader = buffered_reader(128, bytes);
        drop(reader.writer.take());
        let mut output = [0; 64];
        let count = read_available(&mut reader, &mut output);
        assert_eq!(&output[..count], bytes);
        assert_eq!(read_available(&mut reader, &mut output), 0);
    }

    #[test]
    fn a_small_read_leaves_the_remaining_bytes_in_order() {
        let mut reader = buffered_reader(128, b"firstsecond");
        let mut output = [0; 5];
        assert_eq!(read_available(&mut reader, &mut output), 5);
        assert_eq!(&output, b"first");
        let mut output = [0; 6];
        assert_eq!(read_available(&mut reader, &mut output), 6);
        assert_eq!(&output, b"second");
    }

    #[test]
    fn fragmented_source_reads_preserve_a_burst_larger_than_both_pipes() {
        let (source, mut output) = miow::pipe::anonymous(128 * 1024).unwrap();
        let payload = b"\x1b[31mfragmented\xe6\xbc\xa2\r\n".repeat(16 * 1024);
        let expected = payload.clone();
        let producer = thread::spawn(move || {
            for fragment in payload.chunks(997) {
                output.write_all(fragment).unwrap();
            }
        });
        let mut reader = UnblockedReader::new(source, 64 * 1024);
        let mut bytes = Vec::new();
        let mut buffer = [0; 4096];
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let count = reader.try_read(&mut buffer);
            bytes.extend_from_slice(&buffer[..count]);
            let finished = reader
                .reader_thread
                .as_ref()
                .unwrap()
                .control
                .finished
                .load(std::sync::atomic::Ordering::Acquire);
            if finished && reader.pipe.is_empty() {
                break;
            }
            assert!(Instant::now() < deadline, "source reader did not reach EOF");
            if count == 0 {
                thread::yield_now();
            }
        }
        producer.join().unwrap();
        reader.pause().unwrap();
        assert_eq!(bytes, expected);
    }

    #[test]
    fn rearming_wakes_for_each_spaced_burst() {
        let (source, mut output) = miow::pipe::anonymous(128 * 1024).unwrap();
        let mut reader = UnblockedReader::new(source, 64 * 1024);
        let poller = Arc::new(Poller::new().unwrap());
        let interest = Event::readable(7);
        reader.register(&poller, interest, PollMode::Level);
        let mut events = polling::Events::new();
        // Discard the registration's initial kick before testing an idle wake.
        poller.wait(&mut events, Some(Duration::from_secs(1))).unwrap();
        assert!(!events.is_empty());
        for burst in [b"first".as_slice(), b"second"] {
            let mut bytes = [0; 32];
            assert_eq!(reader.try_read(&mut bytes), 0);
            reader.rearm();
            // A read that yields wakes the poller too. Drop any such wake, so
            // that only the burst's own can satisfy the wait below.
            poller.wait(&mut events, Some(Duration::ZERO)).unwrap();
            events.clear();
            output.write_all(burst).unwrap();
            poller.wait(&mut events, Some(Duration::from_secs(1))).unwrap();
            assert!(events.iter().any(|event| event.key == 7 && event.readable));
            let count = read_available(&mut reader, &mut bytes);
            assert_eq!(&bytes[..count], burst);
        }
        drop(output);
        reader.pause().unwrap();
        reader.deregister();
    }

    #[test]
    fn the_end_of_the_source_wakes_an_idle_reader() {
        let (source, mut output) = miow::pipe::anonymous(0).unwrap();
        let mut reader = UnblockedReader::new(source, 128);
        let poller = Arc::new(Poller::new().unwrap());
        reader.register(&poller, Event::readable(7), PollMode::Level);
        let mut events = polling::Events::new();
        output.write_all(b"last").unwrap();
        let mut bytes = [0; 32];
        let deadline = Instant::now() + Duration::from_secs(1);
        let count = loop {
            let count = reader.try_read(&mut bytes);
            if count != 0 {
                break count;
            }
            assert!(Instant::now() < deadline, "the bytes never arrived");
            thread::yield_now();
        };
        assert_eq!(&bytes[..count], b"last");
        // Consume the registration's kick and the wake for those bytes.
        poller.wait(&mut events, Some(Duration::from_millis(100))).unwrap();
        events.clear();
        poller.wait(&mut events, Some(Duration::from_millis(100))).unwrap();
        assert!(!reader.source_ended());
        reader.rearm();
        events.clear();
        // No byte follows the end, so only the reader thread can wake the loop.
        drop(output);
        poller.wait(&mut events, Some(Duration::from_secs(1))).unwrap();
        assert!(events.iter().any(|event| event.key == 7 && event.readable));
        assert!(reader.source_ended());
        assert_eq!(reader.try_read(&mut bytes), 0);
        reader.deregister();
    }

    #[test]
    fn pause_cancels_a_read_that_starts_after_the_first_cancellation() {
        struct DelayedRead {
            source: miow::pipe::AnonRead,
            entered: Option<mpsc::Sender<()>>,
            release: mpsc::Receiver<()>,
        }

        impl AsRawHandle for DelayedRead {
            fn as_raw_handle(&self) -> std::os::windows::io::RawHandle {
                self.source.as_raw_handle()
            }
        }

        impl Read for DelayedRead {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                if let Some(entered) = self.entered.take() {
                    entered.send(()).unwrap();
                    self.release.recv().unwrap();
                }
                self.source.read(buffer)
            }
        }

        let (source, mut output) = miow::pipe::anonymous(0).unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (paused_tx, paused_rx) = mpsc::channel();
        let source = DelayedRead { source, entered: Some(entered_tx), release: release_rx };
        let mut reader = UnblockedReader::new(source, 128);
        assert_eq!(reader.try_read(&mut [0; 128]), 0);
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let pauser = thread::spawn(move || {
            reader.pause().unwrap();
            paused_tx.send(reader).unwrap();
        });
        // The first CancelIoEx sees no native operation while DelayedRead is
        // waiting here. The subsequent ReadFile must still be cancelled.
        thread::sleep(Duration::from_millis(20));
        release_tx.send(()).unwrap();
        let paused = paused_rx.recv_timeout(Duration::from_secs(1));
        if paused.is_err() {
            // Unblock the broken implementation before asserting, so the
            // negative control fails cleanly instead of leaking a reader.
            output.write_all(b"x").unwrap();
            drop(paused_rx.recv_timeout(Duration::from_secs(1)).unwrap());
        }
        pauser.join().unwrap();
        assert!(paused.is_ok(), "pause must not require another byte from the child");
    }

    /// A source whose first read panics, after reporting that it was entered.
    struct PanickingRead {
        source: miow::pipe::AnonRead,
        entered: mpsc::Sender<()>,
    }

    impl AsRawHandle for PanickingRead {
        fn as_raw_handle(&self) -> std::os::windows::io::RawHandle {
            self.source.as_raw_handle()
        }
    }

    impl Read for PanickingRead {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            self.entered.send(()).unwrap();
            panic!("the source reader panicked");
        }
    }

    fn panicked_reader() -> (UnblockedReader<PanickingRead>, miow::pipe::AnonWrite) {
        let (source, output) = miow::pipe::anonymous(0).unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let mut reader = UnblockedReader::new(PanickingRead { source, entered: entered_tx }, 128);
        assert_eq!(reader.try_read(&mut [0; 128]), 0);
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while !reader.reader_thread.as_ref().unwrap().join.is_finished() {
            assert!(Instant::now() < deadline, "the reader thread did not unwind");
            thread::yield_now();
        }
        (reader, output)
    }

    #[test]
    fn pause_returns_after_the_reader_panics() {
        let (mut reader, _output) = panicked_reader();
        let (paused_tx, paused_rx) = mpsc::channel();
        // A hung pause leaks this thread; there is no byte that would free it.
        thread::spawn(move || {
            paused_tx.send(reader.pause().is_err()).unwrap();
        });
        let reported = paused_rx.recv_timeout(Duration::from_secs(1));
        assert_eq!(reported, Ok(true), "pause must report the panic instead of spinning");
    }

    #[test]
    fn a_reader_panic_ends_the_source() {
        let (reader, _output) = panicked_reader();
        // Without this, the pane would wait forever for output nothing can produce.
        assert!(reader.source_ended());
    }

    #[test]
    #[ignore = "manual optimized-build pipe throughput check"]
    fn unblocked_pipe_throughput_benchmark() {
        const BYTES: usize = 256 * 1024 * 1024;
        for unblocked in [false, true] {
            let (mut source, mut output) = miow::pipe::anonymous(128 * 1024).unwrap();
            let producer = thread::spawn(move || {
                let fragment = [b'x'; 16 * 1024];
                for _ in 0..BYTES / fragment.len() {
                    output.write_all(&fragment).unwrap();
                }
            });
            let mut buffer = [0; crate::event_loop::READ_CHUNK_SIZE];
            let mut received = 0;
            let started = Instant::now();
            if unblocked {
                let mut reader = UnblockedReader::new(source, crate::event_loop::READ_BUFFER_SIZE);
                while received < BYTES {
                    received += reader.try_read(&mut buffer);
                }
                reader.pause().unwrap();
            } else {
                while received < BYTES {
                    received += source.read(&mut buffer).unwrap();
                }
            }
            producer.join().unwrap();
            assert_eq!(received, BYTES);
            eprintln!(
                "unblocked={unblocked}: {:.1} MiB/s",
                256.0 / started.elapsed().as_secs_f64()
            );
        }
    }
}
