use super::*;

fn owner(text: &str) -> Arc<Inner> {
    let owner = Arc::new(Inner::new().unwrap());
    owner
        .write(
            vec![ClipboardData {
                bytes: text.as_bytes().to_vec(),
                format: owner.atoms.UTF8_STRING,
            }],
            ClipboardKind::Clipboard,
            WaitConfig::None,
        )
        .unwrap();
    owner.server.conn.sync().unwrap();
    owner
}

fn captured(owner: &Arc<Inner>) -> ClipboardReader {
    ClipboardReader {
        inner: owner.clone(),
        captured: CapturedSelection::new(owner.atoms.CLIPBOARD).unwrap(),
    }
}

#[test]
#[ignore = "requires an isolated X11 server; run with --test-threads=1"]
fn replacement_before_worker_start_rejects_the_captured_read() {
    let first = owner("first");
    let read = captured(&first);
    let _replacement = owner("replacement");
    assert!(read.get_any(ClipboardKind::Clipboard).is_err());
    assert!(
        read.captured.invalidated.get(),
        "must reject ownership, not time out"
    );
}

#[test]
#[ignore = "requires an isolated X11 server; run with --test-threads=1"]
fn same_window_replacement_invalidates_the_capture() {
    let first = owner("first");
    let read = captured(&first);
    first
        .write(
            vec![ClipboardData {
                bytes: b"replacement".to_vec(),
                format: first.atoms.UTF8_STRING,
            }],
            ClipboardKind::Clipboard,
            WaitConfig::None,
        )
        .unwrap();
    first.server.conn.sync().unwrap();
    assert!(read.get_any(ClipboardKind::Clipboard).is_err());
    assert!(
        read.captured.invalidated.get(),
        "must reject ownership, not time out"
    );
}

fn serve_until_targets(owner: Arc<Inner>, replace: bool) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(Event::SelectionRequest(request)) =
                owner.server.conn.poll_for_event().unwrap()
            {
                let targets = request.target == owner.atoms.TARGETS;
                if targets && replace {
                    // Replace ownership while answering TARGETS, before a
                    // client can issue its subsequent data conversion.
                    let _replacement = self::owner("replacement");
                    owner.handle_selection_request(request).unwrap();
                    std::thread::sleep(Duration::from_millis(100));
                    return;
                }
                owner.handle_selection_request(request).unwrap();
                if !targets {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("the owner never received the conversion");
    })
}

#[test]
#[ignore = "requires an isolated X11 server; run with --test-threads=1"]
fn replacement_between_targets_and_data_never_pastes_the_successor() {
    let first = owner("first");
    let read = captured(&first);
    let serving = serve_until_targets(first, true);
    assert!(read.get_any(ClipboardKind::Clipboard).is_err());
    assert!(
        read.captured.invalidated.get(),
        "must reject ownership, not time out"
    );
    serving.join().unwrap();
}

#[test]
#[ignore = "requires an isolated X11 server; run with --test-threads=1"]
fn unchanged_owner_completes_the_captured_conversion() {
    let first = owner("first");
    let read = captured(&first);
    let serving = serve_until_targets(first, false);
    assert_eq!(
        read.get_any(ClipboardKind::Clipboard)
            .unwrap()
            .text()
            .as_deref(),
        Some("first")
    );
    serving.join().unwrap();
}
