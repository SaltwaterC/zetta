//! The console palette of a pane's pseudoconsole: ready before the child
//! starts, reset with the prompt, and updated in the order viewers send it.

use super::*;

fn palette_probe_request(palette: ConsolePalette) -> SpawnRequest {
    let mut request = spawn_request();
    request.console_palette = palette;
    request.program = Some(
        daemon_binary()
            .with_file_name("zmux-pty.exe")
            .to_string_lossy()
            .into_owned(),
    );
    request.args.clear();
    request.env.insert(
        "ZETTA_INTERNAL_CONSOLE_PALETTE_PROBE_V1".to_owned(),
        "1".to_owned(),
    );
    request
}

fn assert_powershell_prompt_resets_after_cwd_marker(
    output: &[u8],
    program: &str,
    profile_name: &str,
) {
    let output = String::from_utf8_lossy(output);
    let mut search_from = 0;
    let mut prompt_count = 0;
    while let Some(marker_start) = output[search_from..].find("\u{1b}]2;zetta-cwd:") {
        let marker_start = search_from + marker_start;
        let marker_end = output[marker_start..]
            .find("\u{1b}\\")
            .map(|offset| marker_start + offset + 2)
            .unwrap_or_else(|| {
                panic!(
                    "{program} with profile {profile_name} emitted an unterminated CWD marker: {output:?}"
                )
            });
        assert!(
            output[marker_end..].starts_with("\u{1b}[0mZETTA_PROMPT"),
            "{program} with profile {profile_name} did not reset before the prompt: {output:?}"
        );
        prompt_count += 1;
        search_from = marker_end;
    }
    assert_eq!(
        prompt_count, 2,
        "{program} with profile {profile_name} did not report both prompt transitions: {output:?}"
    );
    assert!(
        output.contains("ZETTA_DONE"),
        "{program} with profile {profile_name} did not finish the prompt exercise: {output:?}"
    );
}

fn assert_shell_starts_quickly(client: &Client, request: SpawnRequest, marker: &[u8]) {
    let started = Instant::now();
    let pane = client
        .spawn(request)
        .expect("creating the native shell pane");
    let spawn_elapsed = started.elapsed();
    let session_id = pane.session_id;
    let mut output = std::fs::File::from(pane.conout);
    let mut input = std::fs::File::from(pane.conin);
    let bytes = read_until(&mut output, &mut input, marker, Duration::from_secs(10));
    let elapsed = started.elapsed();
    eprintln!(
        "{} spawn: {spawn_elapsed:?}; startup: {elapsed:?}",
        String::from_utf8_lossy(marker)
    );
    assert!(
        spawn_elapsed < Duration::from_secs(1),
        "palette initialization blocked the spawn request for {spawn_elapsed:?}"
    );
    assert!(
        bytes.windows(marker.len()).any(|bytes| bytes == marker),
        "native shell did not produce its startup marker within {elapsed:?}: {:?}",
        String::from_utf8_lossy(&bytes)
    );
    drop(output);
    drop(input);
    let _ = client.kill(session_id);
}

#[test]
fn palette_initialization_does_not_block_native_shell_startup() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let mut cmd = spawn_request();
    cmd.args = vec![
        "/D".to_owned(),
        "/Q".to_owned(),
        "/C".to_owned(),
        "echo ZETTA_CMD_READY".to_owned(),
    ];
    assert_shell_starts_quickly(&client, cmd, b"ZETTA_CMD_READY");

    let mut powershell = output_request();
    powershell.args = vec![
        "-NoLogo".to_owned(),
        "-NoProfile".to_owned(),
        "-Command".to_owned(),
        "Write-Output ZETTA_POWERSHELL_READY".to_owned(),
    ];
    assert_shell_starts_quickly(&client, powershell, b"ZETTA_POWERSHELL_READY");

    if let Ok(output) = Command::new("where.exe").arg("pwsh.exe").output()
        && output.status.success()
    {
        let mut pwsh = output_request();
        pwsh.program = Some("pwsh.exe".to_owned());
        pwsh.args = vec![
            "-NoLogo".to_owned(),
            "-NoProfile".to_owned(),
            "-Command".to_owned(),
            "Write-Output ZETTA_PWSH_READY".to_owned(),
        ];
        assert_shell_starts_quickly(&client, pwsh, b"ZETTA_PWSH_READY");
    }

    let msys_bash = Path::new(r"C:\msys64\usr\bin\bash.exe");
    if msys_bash.is_file() {
        let mut msys = spawn_request();
        msys.program = Some(msys_bash.to_string_lossy().into_owned());
        msys.args = vec![
            "--noprofile".to_owned(),
            "--norc".to_owned(),
            "-c".to_owned(),
            "printf ZETTA_MSYS2_READY".to_owned(),
        ];
        assert_shell_starts_quickly(&client, msys, b"ZETTA_MSYS2_READY");
    }

    let _ = client.shutdown();
}

#[test]
fn conpty_powershell_prompts_reset_native_attributes_with_or_without_profile_integration() {
    const POWERSHELL_INTEGRATION: &str =
        include_str!("../../../../src/shell_integration/powershell.ps1");
    const POWERSHELL_CWD_TRACKER: &str = zetta_profiles::POWERSHELL_CWD_TRACKER;

    let profile_tracker = POWERSHELL_INTEGRATION
        .split_once("\n\nif (-not (Test-Path Env:EDITOR))")
        .expect("the profile tracker must remain the integration preamble")
        .0;
    let daemon = TestDaemon::start();
    let client = daemon.client();

    for program in ["powershell.exe", "pwsh.exe"] {
        if Command::new(program)
            .args(["-NoLogo", "-NoProfile", "-Command", "exit"])
            .output()
            .is_err()
        {
            continue;
        }

        for (profile_name, profile) in [("absent", ""), ("present", profile_tracker)] {
            let mut request = spawn_request();
            request.program = Some(program.to_owned());
            request.args = vec![
                "-NoLogo".to_owned(),
                "-NoProfile".to_owned(),
                "-NoExit".to_owned(),
                "-Command".to_owned(),
                format!(
                    "function global:prompt {{ 'ZETTA_PROMPT' }}\n{profile}\n{POWERSHELL_CWD_TRACKER}\ncmd.exe /d /c color 4\nprompt\ncmd.exe /d /c color 2\nprompt\nWrite-Output ZETTA_DONE\nexit"
                ),
            ];
            let pane = client.spawn(request).unwrap_or_else(|error| {
                panic!("creating {program} with profile {profile_name}: {error:#}")
            });
            let session_id = pane.session_id;
            let mut output = std::fs::File::from(pane.conout);
            let mut input = std::fs::File::from(pane.conin);
            let bytes = read_until(
                &mut output,
                &mut input,
                b"ZETTA_DONE",
                Duration::from_secs(10),
            );
            assert_powershell_prompt_resets_after_cwd_marker(&bytes, program, profile_name);
            drop(output);
            drop(input);
            let _ = client.kill(session_id);
        }
    }

    let _ = client.shutdown();
}

#[test]
fn conpty_palette_is_ready_before_the_child_and_updates_while_attached() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let mut initial = ConsolePalette::default();
    initial.colors[6] = [0x01, 0x02, 0x03];
    initial.foreground_index = 3;
    initial.background_index = 4;
    let pane = client
        .spawn(palette_probe_request(initial))
        .expect("creating the palette probe pane");
    let session_id = pane.session_id;
    let pane_id = pane.pane_id;
    let mut output = std::fs::File::from(pane.conout);
    let mut input = std::fs::File::from(pane.conin);

    let first = read_until(
        &mut output,
        &mut input,
        b"PALETTE:00030201,16",
        Duration::from_secs(10),
    );
    assert!(
        first
            .windows(b"PALETTE:00030201,16".len())
            .any(|bytes| bytes == b"PALETTE:00030201,16"),
        "the child did not observe the initial palette: {:?}",
        String::from_utf8_lossy(&first)
    );

    let mut updated = initial;
    updated.colors[6] = [0xaa, 0xbb, 0xcc];
    updated.foreground_index = 5;
    updated.background_index = 6;
    client
        .set_console_palette(session_id, pane_id, updated)
        .expect("updating the attached pseudoconsole palette");
    input.write_all(b"\r\n").expect("releasing the probe");
    let second = read_until(
        &mut output,
        &mut input,
        b"PALETTE:00ccbbaa,35",
        Duration::from_secs(5),
    );
    assert!(
        second
            .windows(b"PALETTE:00ccbbaa,35".len())
            .any(|bytes| bytes == b"PALETTE:00ccbbaa,35"),
        "the child did not observe the updated palette: {:?}",
        String::from_utf8_lossy(&second)
    );

    drop(output);
    drop(input);
    let _ = client.kill(session_id);
    let _ = client.shutdown();
}

#[test]
fn conpty_palette_attributes_survive_default_color_resets() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let mut palette = ConsolePalette::default();
    palette.colors[6] = [0x01, 0x02, 0x03];
    palette.foreground_index = 3;
    palette.background_index = 4;
    let pane = client
        .spawn(palette_probe_request(palette))
        .expect("creating the palette probe pane");
    let session_id = pane.session_id;
    let mut output = std::fs::File::from(pane.conout);
    let mut input = std::fs::File::from(pane.conin);

    let marker = b"PALETTE:00030201,16";
    let first = read_until(&mut output, &mut input, marker, Duration::from_secs(10));
    assert!(first.windows(marker.len()).any(|bytes| bytes == marker));
    input
        .write_all(b"sgr\r\n")
        .expect("requesting default-color resets");
    let second = read_until(&mut output, &mut input, marker, Duration::from_secs(5));
    assert!(
        second.windows(marker.len()).any(|bytes| bytes == marker),
        "default-color resets changed the legacy attributes: {:?}",
        String::from_utf8_lossy(&second)
    );

    drop(output);
    drop(input);
    let _ = client.kill(session_id);
    let _ = client.shutdown();
}

#[test]
fn palette_viewer_subprocess() {
    let Some(payload) = std::env::var_os("ZETTA_TEST_PALETTE_VIEWER") else {
        return;
    };
    let request: PaletteViewerRequest = serde_json::from_str(&payload.to_string_lossy()).unwrap();
    let client = Client::connect_ready_at(&request.directory)
        .expect("connecting the second viewer")
        .expect("the test daemon must be running");
    let pane = match client
        .attach_as(
            request.session_id,
            request.pane_id,
            std::process::id(),
            None,
        )
        .expect("attaching the second viewer from its own process")
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _ => panic!("the second viewer must attach in shared mode"),
    };
    client
        .set_console_palette(request.session_id, request.pane_id, request.palette)
        .expect("applying the second viewer's palette");
    std::fs::write(&request.ready, b"ready").unwrap();
    // The parent closes stdin once it has checked the subsequent palette.
    let mut release = String::new();
    std::io::stdin().read_line(&mut release).unwrap();
    drop(pane);
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PaletteViewerRequest {
    directory: PathBuf,
    session_id: u64,
    pane_id: u64,
    palette: ConsolePalette,
    ready: PathBuf,
}

#[test]
fn shared_conpty_accepts_palette_updates_in_viewer_order() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().expect("subscribing to pane revokes");
    let _exit_reporters = subscription.exits.clone();
    let revokes = subscription.revokes.clone();
    let mut initial = ConsolePalette::default();
    initial.colors[6] = [0x01, 0x02, 0x03];
    let pane = client
        .spawn(palette_probe_request(initial))
        .expect("creating the shared palette probe pane");
    let session_id = pane.session_id;
    let pane_id = pane.pane_id;
    let mut output = std::fs::File::from(pane.conout);
    let mut input = std::fs::File::from(pane.conin);
    let first = read_until(
        &mut output,
        &mut input,
        b"PALETTE:00030201",
        Duration::from_secs(10),
    );
    assert!(
        first
            .windows(b"PALETTE:00030201".len())
            .any(|bytes| bytes == b"PALETTE:00030201"),
        "the child did not observe the shared pane's initial palette: {:?}",
        String::from_utf8_lossy(&first)
    );

    client
        .share(
            session_id,
            session_summary(session_id, pane_id),
            serde_json::Value::Null,
            None,
            true,
        )
        .expect("offering the session for shared attachment");
    let (revoke_tx, revoke_rx) = async_channel::unbounded();
    revokes.register(pane_id, revoke_tx);
    let holder = std::thread::spawn({
        let client = daemon.client();
        move || {
            revoke_rx
                .recv_blocking()
                .expect("the holder must receive a revoke");
            drop(output);
            drop(input);
            client
                .send_snapshot(session_id, pane_id, first, 80, 24)
                .expect("handing the probe pane to the shared relay");
            match client
                .attach_as(session_id, pane_id, std::process::id(), None)
                .expect("reattaching the original viewer")
            {
                AttachOutcome::SharedAttached { pane, .. } => pane,
                _ => panic!("the original viewer must reattach in shared mode"),
            }
        }
    });

    let mut second_palette = initial;
    second_palette.colors[6] = [0x11, 0x22, 0x33];
    let ready = daemon.config.join("palette-viewer-ready");
    let request = PaletteViewerRequest {
        directory: daemon.sessions_dir(),
        session_id,
        pane_id,
        palette: second_palette,
        ready: ready.clone(),
    };
    let mut second_process = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "palette::palette_viewer_subprocess",
            "--nocapture",
        ])
        .env(
            "ZETTA_TEST_PALETTE_VIEWER",
            serde_json::to_string(&request).unwrap(),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("starting the second viewer");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready.is_file() {
        assert!(
            second_process.try_wait().unwrap().is_none(),
            "the second viewer exited before applying its palette"
        );
        assert!(
            Instant::now() < deadline,
            "the second viewer did not become ready"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let holder = holder.join().expect("the holder handover failed");

    let mut latest = second_palette;
    latest.colors[6] = [0xaa, 0xbb, 0xcc];
    latest.foreground_index = 5;
    latest.background_index = 6;
    client
        .set_console_palette(session_id, pane_id, latest)
        .expect("applying the original viewer's newer palette");

    let mut reader = holder.reader();
    holder.send_input(b"\r\n").expect("releasing the probe");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut second_output = Vec::new();
    while Instant::now() < deadline
        && !second_output
            .windows(b"PALETTE:00ccbbaa,35".len())
            .any(|bytes| bytes == b"PALETTE:00ccbbaa,35")
    {
        let mut buffer = [0; 512];
        match reader.read(&mut buffer) {
            Ok(length) => second_output.extend_from_slice(&buffer[..length]),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) => {}
            Err(error) => panic!("reading the shared palette probe: {error}"),
        }
    }
    assert!(
        second_output
            .windows(b"PALETTE:00ccbbaa,35".len())
            .any(|bytes| bytes == b"PALETTE:00ccbbaa,35"),
        "the child did not observe the latest shared viewer's palette: {:?}",
        String::from_utf8_lossy(&second_output)
    );

    drop(holder);
    drop(second_process.stdin.take());
    assert!(
        second_process.wait().unwrap().success(),
        "the second viewer failed"
    );
    let _ = client.kill(session_id);
    let _ = client.shutdown();
}
