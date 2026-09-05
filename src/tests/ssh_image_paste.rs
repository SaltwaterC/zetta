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
        "-F".to_owned(),
        "config with spaces".to_owned(),
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
            "-F",
            "config with spaces",
            "-i",
            "key with spaces",
            "-p2222",
            "-o",
            "ProxyJump=bastion",
            "-v",
        ]
    );
    assert_eq!(
        invocation.batch_args("printf sentinel".to_owned()),
        [
            "-F",
            "config with spaces",
            "-i",
            "key with spaces",
            "-p2222",
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
    assert_eq!(invocation.options, ["-vvi", "key"]);
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
        fs::write(&path, script).unwrap();
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

        let output = run_ssh_process(
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
        let error = run_ssh_process(
            fake_spec(&executable.0, HashMap::new()),
            Vec::new(),
            Duration::from_millis(40),
        )
        .unwrap_err();
        assert!(error.to_string().contains("timed out"), "{error:#}");
    }
}
