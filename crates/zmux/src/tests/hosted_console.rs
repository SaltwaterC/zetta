use super::*;

fn mode(lf_implies_cr: bool, wrap: bool) -> OutputMode {
    OutputMode {
        lf_implies_cr,
        wrap,
    }
}

#[test]
fn lf_gains_a_cr_only_while_the_console_says_so_and_not_after_one() {
    let mut translator = Translator::default();
    assert_eq!(
        translator.translate(b"a\nb\r\nc", mode(true, true)),
        b"a\r\nb\r\nc"
    );
    assert_eq!(translator.translate(b"\n", mode(true, true)), b"\r\n");
    // A CR that ended the previous write still covers this LF.
    translator.translate(b"x\r", mode(true, true));
    assert_eq!(translator.translate(b"\ny", mode(true, true)), b"\ny");
    assert_eq!(translator.translate(b"a\nb", mode(false, true)), b"a\nb");
}

#[test]
fn wrap_changes_become_decawm_and_the_default_says_nothing() {
    let mut translator = Translator::default();
    assert_eq!(translator.translate(b"a", mode(false, true)), b"a");
    assert_eq!(translator.translate(b"b", mode(false, false)), b"\x1b[?7lb");
    assert_eq!(translator.translate(b"c", mode(false, false)), b"c");
    assert_eq!(translator.translate(b"", mode(false, true)), b"\x1b[?7h");
}

#[test]
fn titles_travel_as_osc_with_controls_neutralized() {
    assert_eq!(title_sequence("a\x07b é"), b"\x1b]0;a b \xc3\xa9\x1b\\");
}

#[test]
fn queries_wait_for_the_parser_but_the_daemon_pipe_does_not() {
    let pipeline = Arc::new(Pipeline::default());
    pipeline.publish(b"abc".to_vec()).unwrap();
    // The daemon's copy is available at once, before anything is parsed.
    assert_eq!(&*pipeline.next(true).unwrap(), b"abc");
    let waiter = {
        let pipeline = Arc::clone(&pipeline);
        std::thread::spawn(move || pipeline.flush())
    };
    std::thread::sleep(Duration::from_millis(50));
    assert!(!waiter.is_finished(), "a query must wait for the parse");
    let chunk = pipeline.next(false).unwrap();
    pipeline.parsed(chunk.len());
    waiter.join().unwrap().unwrap();
    pipeline.end();
    assert!(pipeline.next(true).is_none() && pipeline.next(false).is_none());
}
