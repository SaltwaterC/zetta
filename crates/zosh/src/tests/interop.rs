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
    session_from_bootstrap(&bootstrap)
}

fn session_from_bootstrap(bootstrap: &str) -> mosh_rs::MoshSession<display::DisplayScreen> {
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

// Own the foreground server directly: even a failed assertion kills and reaps
// only this test's child, never a user's session or an unrelated process.
struct ColourFixture {
    server: Option<std::process::Child>,
    session: Option<mosh_rs::MoshSession<display::DisplayScreen>>,
    directory: std::path::PathBuf,
}

impl Drop for ColourFixture {
    fn drop(&mut self) {
        // Give the server a chance to tear down its PTY child on assertion
        // failures too, then reap our foreground process regardless.
        if let Some(session) = self.session.as_mut() {
            session.shutdown();
            let deadline = Instant::now() + Duration::from_secs(1);
            while !session.finished() && Instant::now() < deadline {
                if session.pump_ready().is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        if let Some(server) = self.server.as_mut() {
            let _ = server.kill();
            let _ = server.wait();
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn fixture_environment(command: &mut Command, locale: &str) {
    command.env_clear().envs([
        ("PATH", "/usr/bin:/bin"),
        ("LANG", locale),
        ("LANGUAGE", ""),
        ("LC_CTYPE", locale),
        ("LC_NUMERIC", ""),
        ("LC_ZOSH_TEST", "remote-extension"),
        ("TERM", "xterm-256color"),
        ("COLORTERM", "truecolor"),
        ("PYTHONCOERCECLOCALE", "0"),
        ("PYTHONUTF8", "0"),
        ("MOSH_SERVER_NETWORK_TMOUT", "10"),
    ]);
}

#[test]
#[ignore = "requires freshly built ZOSH_TEST_SERVER and Python 3; use make test-zosh-interop"]
fn colours_and_remote_locales_survive_a_complete_round_trip() {
    use std::io::BufRead;
    let locales = Command::new("locale")
        .arg("-a")
        .env_clear()
        .output()
        .unwrap();
    assert!(locales.status.success());
    let locales = String::from_utf8(locales.stdout).unwrap();
    let locale = locales
        .lines()
        .find(|name| name.to_ascii_lowercase().replace('-', "").contains("utf8"))
        .expect("install a UTF-8 locale for the loopback fixture");
    let python = Command::new("python3")
        .args(["-c", "import sys; print(sys.executable)"])
        .output()
        .expect("Python 3 is required for the terminal fixture");
    assert!(python.status.success());
    let python = String::from_utf8(python.stdout).unwrap();
    let directory = std::env::temp_dir().join(format!("zosh-colour-locale-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let mut fixture = ColourFixture {
        server: None,
        session: None,
        directory,
    };
    let report = fixture.directory.join("remote.json");
    let script = include_str!("fixtures/colour_locale.py");
    let mut direct = Command::new(python.trim());
    fixture_environment(&mut direct, locale);
    let direct = direct
        .args(["-c", script, "direct", "unused", locale])
        .output()
        .unwrap();
    assert!(
        direct.status.success(),
        "{}",
        String::from_utf8_lossy(&direct.stderr)
    );

    let mut command =
        Command::new(std::env::var_os("ZOSH_TEST_SERVER").expect("set ZOSH_TEST_SERVER"));
    fixture_environment(&mut command, locale);
    // These must be supplied by the server's colour configuration, not inherited.
    command.env_remove("TERM").env_remove("COLORTERM");
    command
        .args([
            "new",
            "--foreground",
            "-i",
            "127.0.0.1",
            "-p",
            "0",
            "-c",
            "32768",
            "--",
            python.trim(),
            "-c",
            script,
            "remote",
        ])
        .arg(&report)
        .arg(locale)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    fixture.server = Some(command.spawn().unwrap());
    let stdout = fixture.server.as_mut().unwrap().stdout.take().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            let line = line.unwrap();
            if line.starts_with("MOSH CONNECT ") {
                let _ = sender.send(line);
                break;
            }
        }
    });
    let bootstrap = receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("server bootstrap deadline");
    reader.join().unwrap();
    fixture.session = Some(session_from_bootstrap(&bootstrap));
    let session = fixture.session.as_mut().unwrap();
    session.send_resize(80, 24);
    let mut proxy = client::TerminalQueryProxy::default();
    let mut physical = vt100::Parser::new(24, 80, 0);
    let queries: [&[u8]; 4] = [
        b"\x1b]10;?\x07",
        b"\x1b]11;?\x1b\\",
        b"\x1b]10;?\x1b\\",
        b"\x1b]11;?\x07",
    ];
    let responses: [&[u8]; 4] = [
        b"\x1b]10;rgb:1212/3434/5656\x07",
        b"\x1b]11;rgb:abab/cdcd/efef\x1b\\",
        b"\x1b]10;rgb:7878/9a9a/bcbc\x1b\\",
        b"\x1b]11;rgb:dede/f0f0/1212\x07",
    ];
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut answered = 0;
    loop {
        let events = session.pump_ready().unwrap();
        let mut outer = Vec::new();
        client::forward_terminal_queries(&events, &mut proxy, &mut outer).unwrap();
        if !outer.is_empty() {
            assert!(answered < queries.len(), "unexpected repeated query");
            assert_eq!(outer, queries[answered]);
            // One-byte fragments split the introducer, RGB payload and ST.
            // The fixture will not issue its next query until this reply arrives.
            let mut complete = 0;
            for fragment in responses[answered].chunks(1) {
                for input in proxy.filter(fragment) {
                    let client::ProxiedInput::TerminalResponse(bytes) = input else {
                        panic!("terminal reply became keyboard input");
                    };
                    session.send_terminal_response(&bytes);
                    complete += 1;
                }
            }
            assert_eq!(complete, 1);
            answered += 1;
        }
        physical.process(&session.render());
        if physical.screen().contents().contains("COLOUR-LOCALE-DONE") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "round-trip deadline after {answered} replies: {}",
            physical.screen().contents()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(answered, 4);
    for (row, foreground, background) in [
        (
            0,
            vt100::Color::Rgb(0x12, 0x34, 0x56),
            vt100::Color::Rgb(0xab, 0xcd, 0xef),
        ),
        (
            1,
            vt100::Color::Rgb(0x78, 0x9a, 0xbc),
            vt100::Color::Rgb(0xde, 0xf0, 0x12),
        ),
    ] {
        let cell = physical.screen().cell(row, 0).unwrap();
        assert_eq!(cell.contents(), "X");
        assert_eq!(cell.fgcolor(), foreground);
        assert_eq!(cell.bgcolor(), background);
    }
    assert_eq!(
        std::fs::read(report).unwrap(),
        direct.stdout,
        "remote locale/Perl diagnostics differ from direct execution"
    );
    session.send_input(b"q");
    session.shutdown();
    pump_until(session, |session, _| session.finished());
}
