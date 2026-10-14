use super::*;
use crate::event::VoidListener;
use crate::term::Config;
use crate::term::test::TermSize;

#[test]
fn a_gather_chunk_releases_the_grid_between_parse_segments_and_recycles_once() {
    let size = TermSize::new(80, 24);
    let config = Config { scrolling_history: 500, ..Config::default() };
    let terminal = Arc::new(FairMutex::new(Term::new(config.clone(), &size, VoidListener)));
    let (_, filled) = mpsc::sync_channel(1);
    let (free, recycled) = mpsc::sync_channel(1);
    let mut worker = Worker {
        outcome: ParseOutcome {
            parser: ansi::Processor::new(),
            #[cfg(windows)]
            parse_calls: 0,
            #[cfg(windows)]
            parse_time: Duration::ZERO,
        },
        filled,
        pending: None,
        free,
        terminal: terminal.clone(),
        event_proxy: VoidListener,
        replay_barrier: ReplayBarrier::open(),
    };

    // CSI and UTF-8 cross parse boundaries, and the final segment is short.
    let mut payload = vec![b'a'; MAX_LOCKED_READ - 2];
    payload.extend_from_slice(b"\x1b[31m");
    payload.resize(2 * MAX_LOCKED_READ - 1, b'b');
    payload.extend_from_slice("漢".as_bytes());
    payload.resize(3 * MAX_LOCKED_READ + 17, b'c');
    payload.extend_from_slice(b"\r\ndone");
    let len = payload.len();
    let first = Chunk {
        bytes: payload.clone().into_boxed_slice(),
        len,
        parsed: 0,
        mode: None,
        acknowledgement: None,
    };
    worker.parse_batch(first);
    assert_eq!(worker.pending.as_ref().unwrap().parsed, MAX_LOCKED_READ);
    assert!(recycled.try_recv().is_err(), "an in-use chunk cannot return to the reader");
    let mut segments = 1;
    while let Some(chunk) = worker.pending.take() {
        worker.parse_batch(chunk);
        segments += 1;
    }
    assert_eq!(segments, len.div_ceil(MAX_LOCKED_READ));
    let returned = recycled.try_recv().unwrap();
    assert!(returned.is_empty());
    assert_eq!(returned.parsed, 0);
    assert_eq!(returned.bytes.len(), len);
    assert!(recycled.try_recv().is_err());

    let mut serial = Term::new(config, &size, VoidListener);
    ansi::Processor::<ansi::StdSyncHandler>::new().advance(&mut serial, &payload);
    assert!(terminal.lock().grid() == serial.grid());
    assert_eq!(terminal.lock().grid().cursor.point, serial.grid().cursor.point);
}

#[test]
fn queued_modes_are_applied_in_order_and_acknowledged_after_parsing() {
    use crate::hosted_console::{OutputAcknowledgement, OutputMode};
    use crate::index::{Column, Line, Point};
    struct Count(std::sync::atomic::AtomicUsize);
    impl OutputAcknowledgement for Count {
        fn parsed(&self, bytes: usize) {
            self.0.fetch_add(bytes, std::sync::atomic::Ordering::Relaxed);
        }
    }
    let terminal = Arc::new(FairMutex::new(Term::new(
        Config::default(),
        &TermSize::new(20, 10),
        VoidListener,
    )));
    let (queue, filled) = mpsc::sync_channel(2);
    let (free, recycled) = mpsc::sync_channel(2);
    let acknowledgement = Arc::new(Count(std::sync::atomic::AtomicUsize::new(0)));
    let chunk = |bytes: &[u8], lf_implies_cr| Chunk {
        bytes: bytes.to_vec().into_boxed_slice(),
        len: bytes.len(),
        parsed: 0,
        mode: Some(OutputMode { lf_implies_cr, wrap: true }),
        acknowledgement: Some(acknowledgement.clone()),
    };
    queue.send(chunk(b"\nY", false)).unwrap();
    let mut worker = Worker {
        outcome: ParseOutcome {
            parser: ansi::Processor::new(),
            #[cfg(windows)]
            parse_calls: 0,
            #[cfg(windows)]
            parse_time: Duration::ZERO,
        },
        filled,
        pending: None,
        free,
        terminal: terminal.clone(),
        event_proxy: VoidListener,
        replay_barrier: ReplayBarrier::open(),
    };
    worker.parse_batch(chunk(b"ab\nX", true));
    assert_eq!(terminal.lock().grid().cursor.point, Point::new(Line(2), Column(2)));
    assert_eq!(acknowledgement.0.load(std::sync::atomic::Ordering::Relaxed), 6);
    for _ in 0..2 {
        let chunk = recycled.try_recv().unwrap();
        assert_eq!(chunk.mode, None);
        assert!(chunk.acknowledgement.is_none());
    }
}
