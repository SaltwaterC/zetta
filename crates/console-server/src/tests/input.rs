use super::*;
fn state() -> State {
    State {
        records: VecDeque::new(),
        charge: crate::budget::Budget::session().empty(),
        pending: Default::default(),
        ignore_next_keyup: false,
        first_event: None,
        requests: VecDeque::new(),
        keys: keys::Decoder::default(),
        closed: false,
        clients: Default::default(),
        handles: Default::default(),
        alert: Controls::default(),
        cached_ansi: VecDeque::new(),
    }
}
/// Characters of the queued key-down records.
fn typed(state: &State) -> Vec<u16> {
    state
        .records
        .iter()
        .filter(|record| record[4] != 0)
        .map(|record| u16::from_le_bytes([record[14], record[15]]))
        .collect()
}
fn event() -> Handle {
    Handle::new(unsafe {
        windows_sys::Win32::System::Threading::CreateEventW(
            std::ptr::null(),
            1,
            0,
            std::ptr::null(),
        )
    })
    .unwrap()
}
fn input() -> Input {
    input_within(crate::budget::Budget::session())
}
fn input_within(budget: Arc<crate::budget::Budget>) -> Input {
    struct Output;
    impl ConsoleOutput for Output {
        fn write(&mut self, _: &[u8], _: OutputMode) -> io::Result<()> {
            Ok(())
        }
    }
    Input::new(
        Arc::new(event()),
        event(),
        Arc::new(Mutex::new(Box::new(Output))),
        None,
        Default::default(),
        budget,
    )
}
fn read(records: bool, process: usize) -> Request {
    Request {
        descriptor: Descriptor {
            process,
            ..Default::default()
        },
        body: Vec::new(),
        records,
        unicode: true,
        peek: false,
        nowait: false,
        read: ReadParams::default(),
        cooked: None,
        control_key_state: 0,
        waited: false,
    }
}
#[test]
fn win32_records_keep_modifiers_repeat_and_split_escape_sequences() {
    let mut input = state();
    decode_input(&mut input, b"\x1b[65;30;955;1;", false, false);
    assert!(input.records.is_empty());
    decode_input(&mut input, b"16;3_", false, false);
    assert_eq!(
        input.records.pop_front().unwrap(),
        record(65, 30, 955, 1, 16, 3)
    );
}
#[test]
fn split_utf8_paste_keeps_surrogates_and_enter() {
    let mut input = state();
    decode_input(&mut input, &[0xf0, 0x9f], false, false);
    assert!(input.records.is_empty());
    decode_input(&mut input, &[0x99, 0x82, b'\r'], false, false);
    assert_eq!(typed(&input), [0xd83d, 0xde42, 13]);
}
#[test]
fn vt_input_mode_stores_terminal_input_as_text_and_reencodes_late_win32_keys() {
    let mut input = state();
    decode_input(&mut input, b"\x1b[Ax", false, true);
    assert_eq!(typed(&input), "\x1b[Ax".encode_utf16().collect::<Vec<_>>());
    let mut input = state();
    decode_input(
        &mut input,
        b"\x1b[38;72;0;1;0;1_\x1b[38;72;0;0;0;1_",
        false,
        true,
    );
    assert_eq!(typed(&input), "\x1b[A".encode_utf16().collect::<Vec<_>>());
}
#[test]
fn ansi_input_records_carry_one_code_page_byte_each() {
    let records = [
        record(65, 30, u16::from(b'a'), 1, 0, 1),
        record(0, 0, 0x3042, 1, 0, 1),
    ];
    let queue: VecDeque<_> = records.into_iter().collect();
    let chars = |bytes: &[u8]| -> Vec<u16> {
        bytes
            .chunks(20)
            .map(|record| u16::from_le_bytes([record[14], record[15]]))
            .collect()
    };
    let mut cached = VecDeque::new();
    let (bytes, consumed) = ansi_records(&mut cached, &queue, 8, false, 932);
    assert_eq!(consumed, 2);
    assert_eq!(chars(&bytes), [u16::from(b'a'), 0x82, 0xa0]);
    // A character split across the end of the caller's buffer is consumed,
    // and its trail byte is the next read's first record.
    let (bytes, consumed) = ansi_records(&mut cached, &queue, 2, false, 932);
    assert_eq!((chars(&bytes), consumed), (vec![u16::from(b'a'), 0x82], 2));
    let empty = VecDeque::new();
    let (bytes, consumed) = ansi_records(&mut cached, &empty, 1, true, 932);
    assert_eq!((chars(&bytes), consumed), (vec![0xa0], 0));
    let (bytes, _) = ansi_records(&mut cached, &empty, 1, false, 932);
    assert_eq!(chars(&bytes), [0xa0]);
    assert!(cached.is_empty());
}

#[test]
fn processed_ctrl_c_is_detected_in_raw_and_win32_input() {
    // A raw ETX, a win32-input-mode Ctrl+C, and Ctrl+C whose layout yields no
    // character: OpenConsole keys on the virtual key, not only on U+0003.
    for bytes in [
        b"\x03".as_slice(),
        b"\x1b[67;46;3;1;8;1_".as_slice(),
        b"\x1b[67;46;0;1;4;1_".as_slice(),
    ] {
        let mut input = state();
        assert!(decode_input(&mut input, bytes, true, false).ctrl_c);
        assert!(typed(&input).is_empty());
        assert!(input.alert.ctrl_c);
        let mut input = state();
        assert!(!decode_input(&mut input, bytes, false, false).any());
        assert!(!input.records.is_empty());
    }
    // Alt+Ctrl+C is AltGr territory and stays an ordinary key.
    let mut input = state();
    assert!(!decode_input(&mut input, b"\x1b[67;46;0;1;10;1_", true, false).any());
}
#[test]
fn ctrl_break_discards_queued_input_even_without_processed_mode() {
    let mut input = state();
    let controls = decode_input(&mut input, b"ab\x1b[3;70;0;1;8;1_cd", false, false);
    assert!(controls.ctrl_break && !controls.ctrl_c);
    assert_eq!(typed(&input), [u16::from(b'c'), u16::from(b'd')]);
}
#[test]
fn ctrl_c_ends_cooked_reads_and_leaves_record_reads_waiting() {
    let input = input();
    let mut state = lock(&input.0.state);
    state.requests.extend([read(false, 1), read(true, 1)]);
    state.alert.ctrl_c = true;
    let alerted = input.take_alerted(&mut state);
    assert_eq!(alerted.len(), 1);
    assert!(!alerted[0].records);
    assert_eq!(state.requests.len(), 1);
    assert!(!state.alert.any());
}
#[test]
fn ctrl_c_leaves_raw_reads_pending_but_ctrl_break_ends_them() {
    let input = input();
    input.set_modes(0, 7);
    let mut state = lock(&input.0.state);
    state.requests.push_back(read(false, 1));
    state.alert.ctrl_c = true;
    assert!(input.take_alerted(&mut state).is_empty());
    state.alert.ctrl_break = true;
    assert_eq!(input.take_alerted(&mut state).len(), 1);
    assert!(state.requests.is_empty());
}
#[test]
fn flushing_input_discards_records_but_keeps_reads_waiting() {
    let input = input();
    {
        let mut state = lock(&input.0.state);
        state.records.push_back(record(65, 30, 97, 1, 0, 1));
        state.requests.push_back(read(true, 1));
    }
    input.flush();
    let state = lock(&input.0.state);
    assert!(state.records.is_empty());
    assert_eq!(state.requests.len(), 1);
}
#[test]
fn input_backpressure_never_takes_the_blocked_output_lock() {
    struct Output;
    impl ConsoleOutput for Output {
        fn write(&mut self, _: &[u8], _: OutputMode) -> io::Result<()> {
            Ok(())
        }
    }
    let output: SharedOutput = Arc::new(Mutex::new(Box::new(Output)));
    let (ready, seen) = std::sync::mpsc::channel();
    let input = Input::new(
        Arc::new(event()),
        event(),
        output.clone(),
        Some(Arc::new(move |value| ready.send(value).unwrap())),
        Default::default(),
        crate::budget::Budget::session(),
    );
    lock(&input.0.state)
        .records
        .resize(65536, record(0, 0, 65, 1, 0, 1));
    let held = lock(&output);
    let (done, result) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        done.send(input.write(b"x")).unwrap();
    });
    let received = result.recv_timeout(std::time::Duration::from_secs(1));
    drop(held);
    thread.join().unwrap();
    assert_eq!(
        received.unwrap().unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    assert!(!seen.recv().unwrap());
}

#[test]
fn the_first_event_into_an_empty_queue_is_marked_for_waiting_reads() {
    let mut input = state();
    decode_input(
        &mut input,
        b"\x1b[38;72;0;1;0;1_\x1b[38;72;0;0;0;1_a",
        false,
        true,
    );
    // The Up press re-encodes to three characters; its release adds none.
    assert_eq!(input.first_event, Some(3));
    decode_input(&mut input, b"b", false, true);
    assert_eq!(input.first_event, Some(3), "only an empty queue sets it");
}

#[test]
fn a_record_read_with_room_for_none_completes_at_once_while_input_is_queued() {
    let input = input();
    {
        let mut state = lock(&input.0.state);
        state.clients.insert(1);
        state.handles.insert(2);
        state.records.push_back(record(65, 30, 97, 1, 0, 1));
    }
    // ReadConsoleInput(h, records, 0, &read): the output holds no record.
    let mut request = read(true, 1);
    request.descriptor.object = 2;
    assert_eq!(input.fulfill(&mut request).unwrap(), Some(Vec::new()));
    assert_eq!(lock(&input.0.state).records.len(), 1);
    // With nothing queued it waits, as any record read does.
    lock(&input.0.state).records.clear();
    assert_eq!(input.fulfill(&mut request).unwrap(), None);
}

#[test]
fn records_written_past_the_session_budget_are_not_queued() {
    let budget = Arc::new(crate::budget::Budget::new(1000, None));
    let input = input_within(Arc::clone(&budget));
    let mut key = [0; 20];
    key[0] = 1;
    // Ten records, with room for the queue to double.
    assert_eq!(input.inject(&[key; 10], true), 10);
    assert_eq!(lock(&input.0.state).records.len(), 10);
    assert!(budget.used() <= 1000);
    // Thirty would need 1200 bytes of queue.
    assert_eq!(input.inject(&[key; 20], true), 0);
    assert_eq!(lock(&input.0.state).records.len(), 10);
    drop(input);
    assert_eq!(budget.used(), 0);
}
