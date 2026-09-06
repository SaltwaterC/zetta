use super::*;
use mosh_rs::Screen;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// Build the server patch in ../../server and point ZOSH_TEST_SERVER at the
// resulting executable. This uses loopback only and no user's shell config.
#[test]
#[ignore = "requires ZOSH_TEST_SERVER pointing at a patched Mosh server"]
fn a_remote_shell_clear_reaches_the_terminal_without_a_key_binding() {
    let server = std::env::var_os("ZOSH_TEST_SERVER").expect("set ZOSH_TEST_SERVER");
    let output = Command::new(server)
        .args(["new", "-i", "127.0.0.1", "--", "/bin/sh", "-c"])
        .arg(include_str!("fixtures/scrollback.sh"))
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
