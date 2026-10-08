use super::*;

fn ssh(arguments: &[&str]) -> Vec<String> {
    std::iter::once("ssh")
        .chain(arguments.iter().copied())
        .map(str::to_owned)
        .collect()
}

#[test]
fn parses_and_preserves_open_ssh_connection_options() {
    let argv = [
        r"C:\Windows\System32\OpenSSH\ssh.exe".to_owned(),
        "-i".to_owned(),
        "key with spaces".to_owned(),
        "-p2222".to_owned(),
        "-o".to_owned(),
        "ProxyJump=bastion".to_owned(),
        "-v".to_owned(),
        "-tt".to_owned(),
        "-o".to_owned(),
        "RequestTTY=force".to_owned(),
        "-o".to_owned(),
        "RemoteCommand=ignored".to_owned(),
        "alice@example.test".to_owned(),
        "claude --resume".to_owned(),
    ];
    let invocation = foreground_ssh_argv(&argv).expect("the foreground process is ssh");

    assert_eq!(invocation.executable, argv[0]);
    assert_eq!(invocation.target, "alice@example.test");
    assert_eq!(
        invocation.options,
        [
            "-i",
            "key with spaces",
            "-p",
            "2222",
            "-o",
            "ProxyJump=bastion",
            "-v",
        ]
    );
    assert_eq!(
        invocation.batch_args("printf sentinel".to_owned()),
        [
            "-i",
            "key with spaces",
            "-p",
            "2222",
            "-o",
            "ProxyJump=bastion",
            "-v",
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=15",
            "-o",
            "RemoteCommand=none",
            "-o",
            "SessionType=default",
            "-o",
            "StdinNull=no",
            "alice@example.test",
            "printf sentinel",
        ]
    );
}

#[test]
fn extracts_a_target_after_the_end_of_options_marker() {
    let invocation = foreground_ssh_argv(&ssh(&["-i", "identity", "--", "-host", "old command"]))
        .expect("the foreground process is ssh");

    assert_eq!(invocation.target, "-host");
    assert!(invocation.end_options);
    assert_eq!(
        invocation.batch_args("upload".to_owned()),
        [
            "-i",
            "identity",
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=15",
            "-o",
            "RemoteCommand=none",
            "-o",
            "SessionType=default",
            "-o",
            "StdinNull=no",
            "--",
            "-host",
            "upload",
        ]
    );
}

#[test]
fn preserves_bundled_flags_before_a_value_taking_option() {
    let invocation = foreground_ssh_argv(&ssh(&["-vvi", "key", "host", "old command"]))
        .expect("the foreground process is ssh");

    assert_eq!(invocation.target, "host");
    assert_eq!(invocation.options, ["-v", "-v", "-i", "key"]);
}

/// Each of these runs a program, loads code, reads a configuration the user
/// did not choose for this connection, or writes a file. A reported command
/// line carrying any of them is not used at all, however it spells the option.
#[test]
fn options_that_run_load_or_write_anything_reject_the_invocation() {
    for arguments in [
        ["-F", "config", "host"].as_slice(),
        ["-Fconfig", "host"].as_slice(),
        ["-I", "pkcs11.so", "host"].as_slice(),
        ["-E", "log", "host"].as_slice(),
        ["-w", "0:0", "host"].as_slice(),
        ["-P", "tag", "host"].as_slice(),
        ["-o", "ProxyCommand=calc", "host"].as_slice(),
        ["-oProxyCommand=calc", "host"].as_slice(),
        ["-o", "ProxyCommand calc", "host"].as_slice(),
        ["-o", " proxycommand = calc", "host"].as_slice(),
        ["-o", "PROXYCOMMAND=calc", "host"].as_slice(),
        ["-o", "\"ProxyCommand\" calc", "host"].as_slice(),
        // A flag group ending in `o` used to pass through as written.
        ["-vo", "ProxyCommand=calc", "host"].as_slice(),
        ["-qvoProxyCommand=calc", "host"].as_slice(),
        ["-o", "LocalCommand=calc", "host"].as_slice(),
        ["-o", "PermitLocalCommand=yes", "host"].as_slice(),
        ["-o", "KnownHostsCommand=calc", "host"].as_slice(),
        ["-o", "Include=/tmp/evil", "host"].as_slice(),
        ["-o", "PKCS11Provider=/tmp/evil.so", "host"].as_slice(),
        ["-o", "SecurityKeyProvider=/tmp/evil.so", "host"].as_slice(),
        ["-o", "ProxyUseFdpass=yes", "host"].as_slice(),
        ["-o", "Tunnel=yes", "host"].as_slice(),
        // Whatever a future OpenSSH adds is refused until it is reviewed.
        ["-o", "SomeFutureOption=yes", "host"].as_slice(),
        ["-o", "=calc", "host"].as_slice(),
        ["-o", "Port=", "host"].as_slice(),
        ["--proxy", "host"].as_slice(),
        ["-Z", "host"].as_slice(),
    ] {
        assert!(
            foreground_ssh_argv(&ssh(arguments)).is_none(),
            "ssh {arguments:?} must not be used for an image upload"
        );
    }
}

#[test]
fn session_only_options_are_dropped_and_kept_ones_are_normalized() {
    let invocation = foreground_ssh_argv(&ssh(&[
        "-A",
        "-X",
        "-L",
        "8080:localhost:80",
        "-R8081:localhost:81",
        "-D",
        "1080",
        "-o",
        "ForwardAgent=yes",
        "-o",
        "LocalForward 9090 localhost:90",
        "-o",
        "ControlMaster=auto",
        "-o",
        "ControlPersist=10m",
        "-o",
        "ControlPath=~/.ssh/cm-%r@%h:%p",
        "-o",
        "Port 2222",
        "-oUser=alice",
        "-o",
        " IdentityFile = key with spaces ",
        "-J",
        "bastion",
        "-4C",
        "host",
    ]))
    .expect("every option here is allowed or dropped");

    assert_eq!(
        invocation.options,
        [
            "-o",
            "ControlPath=~/.ssh/cm-%r@%h:%p",
            "-o",
            "Port=2222",
            "-o",
            "User=alice",
            "-o",
            "IdentityFile=key with spaces",
            "-J",
            "bastion",
            "-4",
            "-C",
        ]
    );
    assert_eq!(invocation.target, "host");
}

/// What the shell integration reports is one quoted string. Quoting a value
/// with spaces is ordinary and has to keep working.
#[test]
fn quoted_values_in_a_reported_command_line_remain_usable() {
    let command = "ssh -i 'key with spaces' -o 'UserKnownHostsFile=/tmp/known hosts' \
                   -o \"HostKeyAlias=my host\" -l 'a user' -J bastion alice@example.test";
    let invocation =
        foreground_ssh_argv(&[command.to_owned()]).expect("quoted values are not dangerous");

    assert_eq!(invocation.target, "alice@example.test");
    assert_eq!(
        invocation.options,
        [
            "-i",
            "key with spaces",
            "-o",
            "UserKnownHostsFile=/tmp/known hosts",
            "-o",
            "HostKeyAlias=my host",
            "-l",
            "a user",
            "-J",
            "bastion",
        ]
    );
}

/// A WSL, MSYS2 or Cygwin pane names its `ssh` in title text. Only that
/// environment's own `ssh` is started, never a path the text chose.
#[test]
fn a_posix_environment_runs_only_its_own_ssh() {
    for (reported, expected) in [
        ("ssh", "ssh"),
        ("ssh.exe", "ssh"),
        ("SSH.EXE", ""),
        ("/usr/bin/ssh", "/usr/bin/ssh"),
        ("/bin/ssh", "/bin/ssh"),
        ("/usr/bin/ssh.exe", "/usr/bin/ssh"),
    ] {
        assert_eq!(
            posix_environment_ssh_program(reported),
            (!expected.is_empty()).then(|| expected.to_owned()),
            "{reported:?}"
        );
    }
    for reported in [
        "/tmp/evil/ssh",
        "./ssh",
        "../ssh",
        "/home/me/project/ssh",
        r"C:\Users\me\Downloads\ssh.exe",
        "/cygdrive/c/tmp/ssh.exe",
    ] {
        assert_eq!(
            posix_environment_ssh_program(reported),
            None,
            "{reported:?}"
        );
    }
}

#[test]
fn a_wsl_pane_never_starts_a_reported_ssh_path() {
    let handler =
        SshImagePasteHandler::new(Shell::Program("wsl.exe".to_owned()), HashMap::new(), None);
    let invocation = foreground_ssh_argv(&ssh(&["host"])).unwrap();
    assert_eq!(
        handler
            .execution
            .with_trusted_program(invocation)
            .map(|invocation| invocation.executable),
        Some("ssh".to_owned())
    );
    let planted = foreground_ssh_argv(&["/tmp/evil/ssh".to_owned(), "host".to_owned()]).unwrap();
    assert!(handler.execution.with_trusted_program(planted).is_none());
}

#[test]
fn rejects_ssh_modes_that_cannot_upload_on_stdin() {
    for arguments in [
        ["-n", "host"].as_slice(),
        ["-N", "host"].as_slice(),
        ["-f", "host"].as_slice(),
        ["-W", "localhost:22", "host"].as_slice(),
        ["-O", "check", "host"].as_slice(),
        ["-Q", "cipher", "host"].as_slice(),
        ["-s", "host", "sftp"].as_slice(),
        ["-o", "StdinNull=yes", "host"].as_slice(),
        ["-o", "SessionType=none", "host"].as_slice(),
        ["-o", "SessionType=subsystem", "host"].as_slice(),
    ] {
        assert!(
            foreground_ssh_argv(&ssh(arguments)).is_none(),
            "ssh {:?} must not be used for an image upload",
            arguments
        );
    }
}

#[test]
fn quoted_shell_reported_commands_are_parsed_without_expansion() {
    let command = r#"ssh.exe -i 'key with spaces' 'alice@example.test' "#;
    let argv = vec![command.to_owned()];
    let invocation = foreground_ssh_argv(&argv).expect("quoted ssh command should be safe");

    assert_eq!(invocation.executable, "ssh.exe");
    assert_eq!(invocation.target, "alice@example.test");
    assert_eq!(invocation.options, ["-i", "key with spaces"]);
}

#[test]
fn shell_reported_commands_with_expansion_or_ambiguous_syntax_are_rejected() {
    for command in [
        "ssh $SSH_TARGET",
        "ssh \"$SSH_TARGET\"",
        "ssh host $(hostname)",
        "ssh host; cat /secret",
        "ssh host && cat /secret",
        "ssh host | cat",
        "ssh host > output",
        "ssh host [wildcard]",
        "ssh host 'unfinished",
    ] {
        assert!(
            foreground_ssh_argv(&[command.to_owned()]).is_none(),
            "shell command {command:?} must be rejected"
        );
    }
}

#[test]
fn non_ssh_foreground_processes_use_the_native_path() {
    assert!(foreground_ssh_argv(&["claude".to_owned()]).is_none());
    assert!(foreground_ssh_argv(&["/usr/bin/ssh".to_owned(), "host".to_owned()]).is_some());
    assert!(foreground_ssh_argv(&["ssh-helper".to_owned(), "host".to_owned()]).is_none());
}

#[test]
fn a_mosh_session_is_recognized_in_every_form_a_pane_reports_it() {
    let argv = |words: &[&str]| {
        words
            .iter()
            .map(|word| (*word).to_owned())
            .collect::<Vec<_>>()
    };
    for session in [
        argv(&["zosh", "host"]),
        argv(&["/usr/bin/zosh", "host"]),
        argv(&[r"C:\Program Files\Zetta\zosh.exe", "host"]),
        argv(&["mosh", "--ssh=ssh -p 2222", "host"]),
        argv(&["mosh-client", "-# host |", "192.0.2.1", "60001"]),
        // The single-string form MSYS2 and Cygwin report a command line as.
        argv(&["zosh user@host"]),
    ] {
        assert!(foreground_is_mosh_session(&session), "{session:?}");
    }
    for local in [
        argv(&["ssh", "host"]),
        argv(&["zsh"]),
        argv(&["bash", "-l"]),
        argv(&["vim", "zosh.txt"]),
        argv(&["zosh-server", "new"]),
        argv(&["vim zosh"]),
        argv(&[]),
    ] {
        assert!(!foreground_is_mosh_session(&local), "{local:?}");
    }
}

#[cfg(feature = "zosh-client")]
fn mosh(program: &str, arguments: &[&str]) -> Vec<String> {
    std::iter::once(program)
        .chain(arguments.iter().copied())
        .map(str::to_owned)
        .collect()
}

#[cfg(feature = "zosh-client")]
#[test]
fn a_mosh_launcher_uploads_through_the_ssh_command_it_bootstrapped_with() {
    let invocation = foreground_mosh_argv(&mosh("/usr/local/bin/zosh", &["pi@adsb"]))
        .expect("a zosh launcher names its target");
    assert_eq!(invocation.executable, "ssh");
    assert_eq!(invocation.target, "pi@adsb");
    assert!(invocation.options.is_empty());

    let invocation = foreground_mosh_argv(&mosh(
        "zosh",
        &[
            "--ssh=ssh -p 2222 -i 'key with spaces'",
            "alice@example.test",
        ],
    ))
    .expect("--ssh supplies the auxiliary connection");
    assert_eq!(invocation.executable, "ssh");
    assert_eq!(invocation.target, "alice@example.test");
    assert_eq!(invocation.options, ["-p", "2222", "-i", "key with spaces"]);

    // A shell that reports one command line rather than an argument vector.
    assert_eq!(
        foreground_mosh_argv(&["zosh --predict always pi@adsb".to_owned()])
            .expect("a single-string mosh command line is split")
            .target,
        "pi@adsb"
    );
}

#[cfg(feature = "zosh-client")]
#[test]
fn a_mosh_udp_port_never_reaches_the_auxiliary_ssh_connection() {
    for arguments in [
        vec!["-p", "60000:61000", "pi@adsb"],
        vec!["--port=60000", "pi@adsb"],
    ] {
        let invocation =
            foreground_mosh_argv(&mosh("mosh", &arguments)).expect("the target is still recovered");
        assert_eq!(invocation.target, "pi@adsb");
        assert!(
            invocation.options.is_empty(),
            "Mosh's UDP port is not an SSH option: {:?}",
            invocation.options
        );
        assert!(
            !invocation
                .batch_args(String::new())
                .contains(&"60000".to_owned())
        );
    }
}

/// Upstream Mosh replaces its launcher with `mosh-client`, whose second
/// argument is the `ps` display string Mosh builds from the launcher's original
/// command line. The shape here is the one measured from mosh 1.4.0:
/// `argv = [client, "-# <arguments> |", ip, port]`.
#[cfg(feature = "zosh-client")]
#[test]
fn an_upstream_mosh_client_recovers_the_launcher_command_line_it_carries() {
    let invocation = foreground_mosh_argv(&mosh(
        "/opt/homebrew/bin/mosh-client",
        &[
            "-# --experimental-remote-ip=local --ssh=/usr/bin/ssh -p 60001 --predict always \
             pi@adsb |",
            "198.51.100.7",
            "60001",
        ],
    ))
    .expect("mosh-client carries the launcher command line");

    assert_eq!(invocation.executable, "/usr/bin/ssh");
    assert_eq!(invocation.target, "pi@adsb");
    assert!(
        invocation.options.is_empty(),
        "Mosh's UDP port is not an SSH option: {:?}",
        invocation.options
    );

    // The plainest form, which is what almost every session actually looks like.
    assert_eq!(
        foreground_mosh_argv(&mosh(
            "mosh-client",
            &["-# pi@adsb |", "198.51.100.7", "60001"],
        ))
        .expect("a bare target round-trips")
        .target,
        "pi@adsb"
    );
}

/// Mosh joins the launcher's arguments with a space, so a quoted value cannot
/// be told from the argument after it. The first thing that misparse reaches is
/// the target, and uploading a clipboard image to a host nobody named is worse
/// than not uploading it.
#[cfg(feature = "zosh-client")]
#[test]
fn a_reconstructed_mosh_command_line_refuses_a_target_it_cannot_trust() {
    // `--ssh='ssh -o ProxyJump=b'` as the user typed it. Re-split, `-o` is
    // Mosh's own `--predict-overwrite` flag and the assignment lands where the
    // target belongs.
    assert!(
        foreground_mosh_argv(&mosh(
            "mosh-client",
            &[
                "-# --ssh=ssh -o ProxyJump=b pi@adsb |",
                "198.51.100.7",
                "60001"
            ],
        ))
        .is_none()
    );

    // The same shape from a launcher that is still the pane's process is not a
    // reconstruction, so it is taken at its word.
    assert_eq!(
        foreground_mosh_argv(&mosh("zosh", &["--ssh=ssh -o ProxyJump=b", "pi@adsb"]))
            .expect("an exact command line keeps its quoting")
            .target,
        "pi@adsb"
    );
}

#[cfg(feature = "zosh-client")]
#[test]
fn mosh_sessions_without_a_recoverable_ssh_connection_use_the_native_path() {
    for argv in [
        // Not the display string Mosh builds, so there is nothing to recover.
        mosh("mosh-client", &["198.51.100.7", "60001"]),
        mosh("mosh-client", &["-# |", "198.51.100.7", "60001"]),
        // Not OpenSSH, so the batch-mode option normalization does not apply.
        mosh("zosh", &["--ssh=plink -ssh", "pi@adsb"]),
        // No SSH bootstrap at all.
        mosh("zosh", &["--local", "pi@adsb"]),
        // The launcher re-entering itself as an SSH ProxyCommand.
        mosh("zosh", &["--fake-proxy", "--", "pi@adsb", "22"]),
        // No target to connect to.
        mosh("zosh", &["--help"]),
        mosh("zosh", &[]),
        // An option this launcher does not accept.
        mosh("zosh", &["--not-an-option", "pi@adsb"]),
    ] {
        assert!(
            foreground_mosh_argv(&argv).is_none(),
            "{argv:?} must keep the native shortcut"
        );
    }
}

#[cfg(feature = "zosh-client")]
#[test]
fn the_foreground_invocation_covers_both_ssh_and_mosh() {
    assert_eq!(
        foreground_invocation(&ssh(&["pi@adsb"]))
            .expect("ssh is recognized")
            .target,
        "pi@adsb"
    );
    assert_eq!(
        foreground_invocation(&mosh("zosh", &["pi@adsb"]))
            .expect("zosh is recognized")
            .target,
        "pi@adsb"
    );
    assert!(foreground_invocation(&["claude".to_owned()]).is_none());
}

#[test]
fn unsupported_foreground_processes_return_the_native_paste_action() {
    let handler = SshImagePasteHandler::new(Shell::System, HashMap::new(), None);
    let image = gpui::Image {
        format: gpui::ImageFormat::Svg,
        bytes: Vec::new(),
        id: 1,
    };

    assert_eq!(
        handler
            .paste_image(&image, Some(&["claude".to_owned()]))
            .unwrap(),
        ImagePasteResult::UseNativeShortcut
    );
}

/// Why a pane in a remote session must never be built with this handler, and
/// why getting that wrong looks like nothing happening: a byte-stream pane's
/// process belongs to the session's host, so this window has no foreground
/// process to report for it, and every image paste degrades to the native
/// chord — which reaches a program on another machine as an empty clipboard.
/// `background_session_ui::image_paste::handler_for_pane` is what keeps the
/// two apart.
#[test]
fn a_pane_with_no_foreground_process_can_only_reach_the_native_paste_action() {
    let handler = SshImagePasteHandler::new(Shell::System, HashMap::new(), None);
    let image = gpui::Image {
        format: gpui::ImageFormat::Png,
        bytes: b"\x89PNG\r\n\x1a\n".to_vec(),
        id: 1,
    };

    assert_eq!(
        handler.paste_image(&image, None).unwrap(),
        ImagePasteResult::UseNativeShortcut
    );
}

#[test]
fn generated_remote_commands_use_bounded_private_storage() {
    let sentinel = "__sentinel__";
    let probe = posix_probe_command(sentinel);
    assert!(probe.contains("command -v uname"));
    assert!(probe.contains(sentinel));

    let upload = posix_upload_command(sentinel);
    for fragment in ["umask 077", "mktemp -d", "chmod 700", "cat >", sentinel] {
        assert!(
            upload.contains(fragment),
            "upload command lacks {fragment:?}"
        );
    }
    assert!(upload.contains("image_path="));
    assert!(!upload.contains(" path="));
    assert_eq!(
        posix_cleanup_command("/tmp/zetta-image/O'Reilly"),
        "rm -rf -- '/tmp/zetta-image/O'\\''Reilly'"
    );

    let powershell = powershell_upload_script(sentinel);
    for fragment in [
        "GetTempPath",
        "SetAccessRuleProtection($true,$false)",
        "OpenStandardInput",
        "CopyTo",
        sentinel,
    ] {
        assert!(
            powershell.contains(fragment),
            "PowerShell upload lacks {fragment:?}"
        );
    }
}

#[test]
fn powershell_commands_are_utf16le_base64_encoded() {
    let script = "$input = 'clipboard'; Write-Output 'done'";
    let command = powershell_remote_command("powershell.exe", script);
    let encoded = command.rsplit_once(' ').unwrap().1;
    let bytes = BASE64.decode(encoded).unwrap();
    let words = bytes
        .chunks_exact(2)
        .map(|word| u16::from_le_bytes([word[0], word[1]]))
        .collect::<Vec<_>>();

    assert_eq!(String::from_utf16(&words).unwrap(), script);
    assert!(command.starts_with("powershell.exe -NoLogo -NoProfile -NonInteractive"));
}

#[test]
fn sentinel_and_remote_path_validation_are_strict() {
    let sentinel = "SENTINEL";
    assert_eq!(
        delimited_value(b"noiseSENTINEL/tmp/a/image.pngSENTINELtail", sentinel),
        Some("/tmp/a/image.png")
    );
    assert!(delimited_value(b"missing", sentinel).is_none());
    assert!(posix_probe_succeeded(b"SENTINELLinuxSENTINEL", sentinel));
    assert!(!posix_probe_succeeded(b"SENTINEL\nSENTINEL", sentinel));

    let posix = RemotePlatform::Posix;
    assert_eq!(
        validate_remote_path("/tmp/zetta-image/image.png", &posix).unwrap(),
        "/tmp/zetta-image/image.png"
    );
    for path in [
        "tmp/image.png",
        "/tmp/../image.png",
        "/tmp/image.jpg",
        "/tmp/image*.png",
        "/tmp/image.png\nsecond",
    ] {
        assert!(validate_remote_path(path, &posix).is_err(), "path {path:?}");
    }
    let powershell = RemotePlatform::PowerShell("powershell.exe".to_owned());
    assert!(validate_remote_path(r"C:\Users\me\zetta-image\image.png", &powershell).is_ok());
    assert!(validate_remote_path(r"relative\image.png", &powershell).is_err());
    assert_eq!(
        remote_directory(r"C:\Users\me\zetta-image\image.png"),
        Some(r"C:\Users\me\zetta-image".to_owned())
    );
}

#[cfg(unix)]
mod process_tests {
    use super::*;
    use std::{
        fs,
        fs::File,
        io::Write,
        os::unix::fs::PermissionsExt,
        path::Path,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_TEST_PATH: AtomicU64 = AtomicU64::new(1);

    struct TemporaryExecutable(PathBuf);

    impl Drop for TemporaryExecutable {
        fn drop(&mut self) {
            fs::remove_file(&self.0).ok();
        }
    }

    fn temporary_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "zetta-{label}-{}-{}",
            std::process::id(),
            NEXT_TEST_PATH.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn temporary_executable(script: &str) -> TemporaryExecutable {
        let path = temporary_path("ssh-image-paste");
        let mut file = File::create(&path).unwrap();
        file.write_all(script.as_bytes()).unwrap();
        file.sync_all().unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&path, permissions).unwrap();
        TemporaryExecutable(path)
    }

    fn fake_spec(program: &Path, environment: HashMap<String, String>) -> LaunchSpec {
        LaunchSpec {
            program: program.to_string_lossy().into_owned(),
            args: Vec::new(),
            environment,
            working_directory: None,
        }
    }

    #[test]
    fn fake_ssh_process_receives_the_png_and_returns_a_path() {
        let input_path = temporary_path("ssh-image-input");
        let sentinel = "__FAKE_SENTINEL__";
        let executable = temporary_executable(
            "#!/bin/sh\ncat > \"$ZETTA_TEST_INPUT\"\nprintf '%s%s%s\\n' \"$ZETTA_TEST_SENTINEL\" '/tmp/zetta-image-test/image.png' \"$ZETTA_TEST_SENTINEL\"\n",
        );
        let spec = fake_spec(
            &executable.0,
            HashMap::from([
                (
                    "ZETTA_TEST_INPUT".to_owned(),
                    input_path.to_string_lossy().into_owned(),
                ),
                ("ZETTA_TEST_SENTINEL".to_owned(), sentinel.to_owned()),
            ]),
        );

        let output = run_image_paste_process(
            spec,
            b"\x89PNG\r\n\x1a\nimage".to_vec(),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(
            extract_remote_path(&output, sentinel, &RemotePlatform::Posix).unwrap(),
            "/tmp/zetta-image-test/image.png"
        );
        assert_eq!(fs::read(&input_path).unwrap(), b"\x89PNG\r\n\x1a\nimage");
        fs::remove_file(input_path).unwrap();
    }

    #[test]
    fn auxiliary_ssh_processes_have_a_hard_timeout() {
        let executable = temporary_executable("#!/bin/sh\nexec sleep 2\n");
        let error = run_image_paste_process(
            fake_spec(&executable.0, HashMap::new()),
            Vec::new(),
            Duration::from_millis(40),
        )
        .unwrap_err();
        assert!(error.to_string().contains("timed out"), "{error:#}");
    }

    struct TemporaryDirectory(PathBuf);

    impl Drop for TemporaryDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    /// A stand-in for OpenSSH that runs the remote command locally. The upload
    /// path recognizes a client only by the name `ssh`, so the executable has
    /// to be called that and therefore needs a directory of its own.
    fn fake_ssh_in_its_own_directory() -> (TemporaryDirectory, PathBuf) {
        let directory = TemporaryDirectory(temporary_path("ssh-image-home"));
        fs::create_dir_all(&directory.0).unwrap();
        let path = directory.0.join("ssh");
        let mut file = File::create(&path).unwrap();
        file.write_all(
            b"#!/bin/sh\nfor argument in \"$@\"; do last=$argument; done\nexec /bin/sh -c \"$last\"\n",
        )
        .unwrap();
        file.sync_all().unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&path, permissions).unwrap();
        (directory, path)
    }

    #[cfg(feature = "zosh-client")]
    fn path_with(directory: &Path) -> String {
        std::env::join_paths(
            std::iter::once(directory.to_path_buf())
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap()
        .to_string_lossy()
        .into_owned()
    }

    /// The native `ssh` is the one an absolute `PATH` entry names. A reported
    /// path is accepted only when it is that executable; an `ssh` anywhere
    /// else, or one only the current directory would find, is not run.
    #[test]
    fn a_native_pane_runs_only_the_ssh_its_path_resolves() {
        let (trusted, trusted_ssh) = fake_ssh_in_its_own_directory();
        let (_planted, planted_ssh) = fake_ssh_in_its_own_directory();
        let environment =
            HashMap::from([("PATH".to_owned(), trusted.0.to_string_lossy().into_owned())]);
        let resolved = Some(trusted_ssh.to_string_lossy().into_owned());

        assert_eq!(native_ssh_program("ssh", &environment), resolved);
        assert_eq!(
            native_ssh_program(&trusted_ssh.to_string_lossy(), &environment),
            resolved
        );
        assert_eq!(
            native_ssh_program(&planted_ssh.to_string_lossy(), &environment),
            None
        );
        assert_eq!(native_ssh_program("./ssh", &environment), None);

        // An empty or relative `PATH` entry is the current directory.
        let current_directory_only = HashMap::from([("PATH".to_owned(), ".::bin".to_owned())]);
        assert_eq!(native_ssh_program("ssh", &current_directory_only), None);
    }

    #[test]
    fn a_planted_ssh_path_keeps_the_native_paste_action() {
        let (trusted, _) = fake_ssh_in_its_own_directory();
        let (_planted, planted_ssh) = fake_ssh_in_its_own_directory();
        let handler = SshImagePasteHandler::new(
            Shell::System,
            HashMap::from([("PATH".to_owned(), trusted.0.to_string_lossy().into_owned())]),
            None,
        );
        let image = gpui::Image {
            format: gpui::ImageFormat::Png,
            bytes: b"\x89PNG\r\n\x1a\n".to_vec(),
            id: 1,
        };
        assert_eq!(
            handler
                .paste_image(
                    &image,
                    Some(&[
                        planted_ssh.to_string_lossy().into_owned(),
                        "host".to_owned()
                    ]),
                )
                .unwrap(),
            ImagePasteResult::UseNativeShortcut
        );
    }

    #[cfg(feature = "zosh-client")]
    fn clipboard_png() -> gpui::Image {
        let mut bytes = Vec::new();
        image::DynamicImage::new_rgba8(1, 1)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        gpui::Image {
            format: gpui::ImageFormat::Png,
            bytes,
            id: 1,
        }
    }

    /// The whole Mosh path, end to end: a `zosh` foreground process, its
    /// `--ssh` command, the platform probe, the real upload command, and the
    /// path that comes back. The stand-in runs the remote command locally, so
    /// the file this asserts on is the one `posix_upload_command` wrote.
    #[cfg(feature = "zosh-client")]
    #[test]
    fn a_mosh_pane_uploads_a_clipboard_image_through_its_ssh_command() {
        let (directory, ssh) = fake_ssh_in_its_own_directory();
        // The `--ssh` path is only accepted because it is the `ssh` this
        // pane's `PATH` resolves to.
        let handler = SshImagePasteHandler::new(
            Shell::System,
            HashMap::from([("PATH".to_owned(), path_with(&directory.0))]),
            None,
        );
        let foreground = [
            "/usr/local/bin/zosh".to_owned(),
            format!("--ssh={}", ssh.display()),
            "--predict".to_owned(),
            "always".to_owned(),
            "pi@adsb".to_owned(),
        ];

        let resolved = handler
            .paste_image(&clipboard_png(), Some(&foreground))
            .expect("a Mosh pane resolves a clipboard image to a path");
        let ImagePasteResult::ResolvedPath(path) = resolved else {
            panic!("a Mosh pane must not fall back to the native shortcut: {resolved:?}");
        };

        assert!(path.ends_with("/image.png"), "unexpected path {path}");
        let written = fs::read(&path).expect("the uploaded image must exist");
        assert!(written.starts_with(b"\x89PNG\r\n\x1a\n"));

        // Dropping the handler removes what it staged.
        let directory = Path::new(&path).parent().unwrap().to_owned();
        drop(handler);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while directory.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !directory.exists(),
            "the staged image directory must be removed with the handler"
        );
    }
}
