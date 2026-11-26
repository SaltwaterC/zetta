//! A pane's agent pipe and its host image, both kept across an upgrade or a
//! replacement of the running host.

use super::*;

fn zmux_agent_pipes() -> Vec<String> {
    std::fs::read_dir(r"\\.\pipe\")
        .expect("listing named pipes")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("zmux-agent-"))
        .collect()
}

/// Whether another process could create `name` now — which is to say whether
/// the name is free for another account to claim.
fn pipe_name_is_free(name: &str) -> bool {
    use windows::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX},
        System::Pipes::{CreateNamedPipeW, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES},
    };

    let wide = format!(r"\\.\pipe\{name}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: `wide` is a NUL-terminated UTF-16 name that outlives the call;
    // no security attributes are passed.
    let handle = unsafe {
        CreateNamedPipeW(
            windows::core::PCWSTR(wide.as_ptr()),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE,
            PIPE_UNLIMITED_INSTANCES,
            1024,
            1024,
            0,
            None,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return false;
    }
    // SAFETY: `handle` is the valid pipe just created here, closed once.
    let _ = unsafe { CloseHandle(handle) };
    true
}

/// A pane's agent pipe is a name another account could create first and then
/// serve to the pane. It must be unpredictable, and held by the daemon for the
/// pane's whole life — across an upgrade, whose handover must also leave no
/// plaintext behind.
#[test]
fn a_panes_agent_pipe_is_unpredictable_and_stays_held_across_an_upgrade() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let before = zmux_agent_pipes();
    let created = client
        .create_shared(CreateSharedRequest {
            operation_id: client.next_shared_operation_id(),
            title: "agent pipe".to_owned(),
            replacement: SharedDraftLayout::Draft { draft_id: 1 },
            panes: vec![SharedPaneDraft {
                draft_id: 1,
                profile: "Command Prompt".to_owned(),
                command: Some(zetta_profiles::ProfileCommand::with_args(
                    "cmd.exe",
                    vec![
                        "/D".to_owned(),
                        "/C".to_owned(),
                        "ping 127.0.0.1 -n 60 >NUL".to_owned(),
                    ],
                )),
                env: HashMap::new(),
                working_directory: None,
                inherit_working_directory_from: None,
                load_shell_integration: false,
                size: spawn_request().size,
                console_palette: ConsolePalette::default(),
                metadata: BackgroundPaneSummary {
                    id: 0,
                    label: "pane".to_owned(),
                    profile: "Command Prompt".to_owned(),
                    configured_command: String::new(),
                    application: "cmd.exe".to_owned(),
                    foreground_command: None,
                    terminal_title: None,
                    working_directory: None,
                    state: BackgroundPaneState::Starting,
                    exit: None,
                },
            }],
            active_pane: Some(SharedPaneRef::Draft { draft_id: 1 }),
            verifier: None,
        })
        .expect("creating a shared pseudoconsole session");
    let session_id = created.session_id;
    let pipe = zmux_agent_pipes()
        .into_iter()
        .find(|name| !before.contains(name))
        .expect("the pane's agent pipe is served");
    let nonce = pipe.rsplit_once('-').map(|(_, nonce)| nonce).unwrap();
    assert!(
        nonce.len() == 32 && nonce.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "the agent pipe name {pipe:?} carries no random component"
    );
    assert!(
        !pipe_name_is_free(&pipe),
        "the daemon does not hold {pipe:?}"
    );

    let upgrade = Command::new(daemon_binary())
        .arg("--upgrade")
        .env("APPDATA", &daemon.config)
        .output()
        .expect("running zmux --upgrade");
    assert!(
        upgrade.status.success(),
        "zmux --upgrade failed: {}",
        String::from_utf8_lossy(&upgrade.stderr)
    );
    let replacement = daemon.client();
    assert!(
        !pipe_name_is_free(&pipe),
        "the replacement does not hold {pipe:?}"
    );
    let leftovers = std::fs::read_dir(daemon.sessions_dir())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("zmux-handover-"))
        .collect::<Vec<_>>();
    assert!(
        leftovers.is_empty(),
        "the upgrade left handover files behind: {leftovers:?}"
    );

    let _ = replacement.kill(session_id);
    let _ = replacement.shutdown();
}

#[test]
fn replacing_a_running_host_image_keeps_panes_and_uses_the_active_bootstrap() {
    let built_daemon = daemon_binary();
    let built_directory = built_daemon.parent().unwrap();
    let binaries = tempfile::Builder::new()
        .prefix("host-upgrade-")
        .tempdir_in(built_directory)
        .unwrap();
    for name in ["zmux.exe", "zmux-pty.exe", "conpty.dll", "OpenConsole.exe"] {
        let source = built_directory.join(name);
        if source.is_file() {
            std::fs::copy(source, binaries.path().join(name)).unwrap();
        }
    }
    let installed_daemon = binaries.path().join("zmux.exe");
    let installed_host = binaries.path().join("zmux-pty.exe");
    let daemon = TestDaemon::start_binary(&installed_daemon);
    let client = daemon.client();
    let pane = client.spawn(spawn_request()).unwrap();
    let host_endpoint = zmux::transport::Endpoint::read_host(&zmux::pty_host::endpoint_path(
        &daemon.sessions_dir(),
    ))
    .unwrap();

    // Archive the image while the host owns a live HPCON, just as installation
    // does. New bootstraps must come from the active installed path.
    let archive = binaries.path().join("zmux-pty.old.exe");
    std::fs::rename(&installed_host, &archive).unwrap();
    std::fs::copy(built_directory.join("zmux-pty.exe"), &installed_host).unwrap();
    assert!(process_is_alive(host_endpoint.process_id));
    assert!(process_is_alive(pane.child_pid));

    let upgrade = Command::new(&installed_daemon)
        .arg("--upgrade")
        .env("APPDATA", &daemon.config)
        .output()
        .unwrap();
    assert!(
        upgrade.status.success(),
        "{}",
        String::from_utf8_lossy(&upgrade.stderr)
    );
    let replacement = daemon.client();
    assert_ne!(replacement.process_id(), client.process_id());
    replacement
        .resize(pane.session_id, pane.pane_id, 100, 30)
        .unwrap();
    assert!(process_is_alive(pane.child_pid));
    ctrl_c::assert_returns_to_prompt(&replacement);
    let after = zmux::transport::Endpoint::read_host(&zmux::pty_host::endpoint_path(
        &daemon.sessions_dir(),
    ))
    .unwrap();
    assert_eq!(after.process_id, host_endpoint.process_id);
    replacement.kill(pane.session_id).unwrap();
    replacement.shutdown().unwrap();
}
