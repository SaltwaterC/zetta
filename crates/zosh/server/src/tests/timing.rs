use super::*;

#[test]
fn log_records_monotonic_metadata_and_flushes_before_stop_ack() {
    let origin = Instant::now();
    let (tx, rx) = mpsc::sync_channel(4);
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    tx.send(Event::Record {
        at: origin + Duration::from_micros(1234),
        name: "input_state",
        a: 7,
        b: 3,
    })
    .unwrap();
    tx.send(Event::Stop(done_tx)).unwrap();
    let mut output = Vec::new();
    write_events(&mut output, rx, origin).unwrap();
    done_rx.try_recv().unwrap();
    let output = String::from_utf8(output).unwrap();
    assert!(output.starts_with("# zosh timing v1 pid="));
    assert!(output.contains("# elapsed_us event a b dropped_total\n"));
    assert!(output.contains("1234 input_state 7 3 0\n"));
}

#[test]
fn failed_sink_ends_logger_without_acknowledging_flush() {
    struct FailedSink;
    impl Write for FailedSink {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("failed sink"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let (tx, rx) = mpsc::sync_channel(1);
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    tx.send(Event::Stop(done_tx)).unwrap();
    assert!(write_events(FailedSink, rx, Instant::now()).is_err());
    assert!(done_rx.try_recv().is_err());
}
