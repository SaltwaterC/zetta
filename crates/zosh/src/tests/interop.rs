use super::*;
use mosh_rs::Screen;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// Build the bundled server in ../../server, or a patched upstream server, and
// point ZOSH_TEST_SERVER at the resulting executable. This uses loopback only
// and no user's shell config.
#[test]
#[ignore = "requires ZOSH_TEST_SERVER pointing at a scrollback-aware Mosh server"]
fn a_remote_shell_clear_reaches_the_terminal_without_a_key_binding() {
    let mut session = connect_test_session(&[
        "new",
        "-i",
        "127.0.0.1",
        "--",
        "/bin/sh",
        "-c",
        include_str!("fixtures/scrollback.sh"),
    ]);
    session.send_resize(80, 24);
    let initial = pump_until(&mut session, |session, _| {
        session.screen_text().contains("READY")
    });
    assert!(!has_clear(&initial));

    session.send_input(b"\r");
    let cleared = pump_until(&mut session, |session, _| {
        session.screen_text().contains("new prompt")
    });
    assert!(has_clear(&cleared), "remote CSI 3 J was lost");
    let mut physical = display::DisplayScreen::new(24, 80);
    physical.feed(&initial);
    physical.feed(&cleared);
    assert!(!physical.text().contains("old TUI"));

    session.send_input(b"\r");
    let redrawn = pump_until(&mut session, |session, _| {
        session.screen_text().contains("redrawn prompt")
    });
    assert!(!has_clear(&redrawn), "ordinary redraw cleared scrollback");

    session.send_input(b"\r");
    pump_until(&mut session, |_, output| has_clear(output));
    session.send_input(b"\r");
    session.shutdown();
    pump_until(&mut session, |session, _| session.finished());
}

// Point ZOSH_TEST_SERVER at either the bundled `zosh-server` or a stock
// one: the whole claim of `PROTOCOL.md` is that both answer a keep-alive,
// so this test is meant to be run against each in turn.
#[test]
#[ignore = "requires ZOSH_TEST_SERVER pointing at any Mosh server"]
fn an_idle_session_with_keep_alive_is_answered_several_times_a_second() {
    let mut session = connect_test_session(&["new", "-i", "127.0.0.1", "--", "/bin/cat"]);
    session.send_resize(80, 24);
    // Settle the connection first, so what is measured below is an idle
    // session rather than the initial exchange.
    let established = Instant::now() + Duration::from_millis(500);
    while Instant::now() < established {
        session.pump_ready().unwrap();
        std::thread::sleep(Duration::from_millis(10));
    }

    session.set_keep_alive(Some(mosh_rs::sender::KEEP_ALIVE_DEFAULT_MS));
    // Nothing is typed from here on. Without a keep-alive the server
    // answers at its own three-second heartbeat, so an acknowledgement
    // inside 1.5 s is only possible because the keep-alive drew one.
    let deadline = Instant::now() + Duration::from_millis(1_500);
    let mut answered = 0;
    let mut previous = u64::MAX;
    while Instant::now() < deadline {
        session.pump_ready().unwrap();
        let since_ack = session.link_health().since_ack_ms;
        if since_ack < previous {
            answered += 1;
        }
        previous = since_ack;
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        answered >= 2,
        "an idle keep-alive session was acknowledged {answered} times in 1.5 s"
    );
    // The remote command is `cat` over a PTY that echoes, so a keep-alive
    // that reached the shell as input would be on the screen twice over.
    assert!(
        session.screen_text().trim().is_empty(),
        "a keep-alive reached the remote shell: {:?}",
        session.screen_text()
    );

    session.shutdown();
    pump_until(&mut session, |session, _| session.finished());
}

// The command emits both supported query spellings. The normal Mosh client
// path observes the decoded host events here; the terminal frontend's byte
// proxy is covered by the focused client tests because this test has no real
// outer terminal to answer the queries.
#[test]
#[ignore = "requires ZOSH_TEST_SERVER pointing at the bundled Mosh server"]
fn bundled_server_forwards_osc_color_queries() {
    let mut session = connect_test_session(&[
        "new",
        "-i",
        "127.0.0.1",
        "--",
        "/bin/sh",
        "-c",
        "printf '\\033]10;?\\007\\033]11;?\\033\\\\'; printf READY",
    ]);
    session.send_resize(80, 24);

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut queries = Vec::new();
    while Instant::now() < deadline && queries.len() < 2 {
        for event in session.pump_ready().unwrap() {
            if let mosh_rs::HostEvent::TerminalQuery { bytes, .. } = event {
                queries.push(bytes);
            }
        }
        let _ = session.render();
        std::thread::sleep(Duration::from_millis(10));
    }

    assert_eq!(
        queries,
        vec![b"\x1b]10;?\x07".to_vec(), b"\x1b]11;?\x1b\\".to_vec(),]
    );
    session.shutdown();
    pump_until(&mut session, |session, _| session.finished());
}

fn connect_test_session(arguments: &[&str]) -> mosh_rs::MoshSession<display::DisplayScreen> {
    let server = std::env::var_os("ZOSH_TEST_SERVER").expect("set ZOSH_TEST_SERVER");
    let output = Command::new(server)
        .args(arguments)
        .env("LANG", "en_US.UTF-8")
        .env("MOSH_SERVER_NETWORK_TMOUT", "10")
        .stdin(Stdio::null())
        .output()
        .expect("start test server");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bootstrap = String::from_utf8(output.stdout).unwrap();
    let mut endpoint = bootstrap
        .lines()
        .find_map(|line| line.strip_prefix("MOSH CONNECT "))
        .expect("server endpoint")
        .split_whitespace();
    let port = endpoint.next().unwrap().parse().unwrap();
    let key = mosh_rs::Base64Key::from_printable(endpoint.next().unwrap()).unwrap();
    let mut session = mosh_rs::MoshSession::connect_with_screen(
        "127.0.0.1",
        port,
        &key,
        display::DisplayScreen::new(24, 80),
    )
    .unwrap();
    session
        .prediction_mut()
        .set_display_preference(mosh_rs::DisplayPreference::Never);
    session
}

fn has_clear(output: &[u8]) -> bool {
    output.windows(4).any(|bytes| bytes == b"\x1b[3J")
}

fn pump_until(
    session: &mut mosh_rs::MoshSession<display::DisplayScreen>,
    done: impl Fn(&mosh_rs::MoshSession<display::DisplayScreen>, &[u8]) -> bool,
) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    loop {
        session.pump_ready().unwrap();
        output.extend(session.render());
        if done(session, &output) {
            return output;
        }
        assert!(
            Instant::now() < deadline,
            "timed out: {:?}",
            session.screen_text()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
