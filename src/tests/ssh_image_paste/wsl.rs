use super::*;

fn profile(arguments: &[&str]) -> Shell {
    Shell::WithArguments {
        program: r"C:\Windows\System32\wsl.exe".to_owned(),
        args: arguments
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect(),
        title_override: None,
    }
}

#[test]
fn staging_preserves_distribution_user_and_directory_but_replaces_the_command() {
    for exec in ["--exec", "-e"] {
        let shell = profile(&[
            "--distribution",
            "Ubuntu 测试",
            "--user",
            "a user",
            "--cd",
            "/home/a user",
            exec,
            "/bin/sh",
            "-c",
            "original tracking wrapper",
        ]);
        let environment = HashMap::from([("ZETTA_THEME".to_owned(), "Dark".to_owned())]);
        let spec = launch_spec(&environment, &shell, vec!["replacement".to_owned()]);
        assert_eq!(spec.program, r"C:\Windows\System32\wsl.exe");
        assert_eq!(
            spec.args,
            [
                "--distribution",
                "Ubuntu 测试",
                "--user",
                "a user",
                "--cd",
                "/home/a user",
                "--exec",
                "replacement",
            ]
        );
        assert_eq!(spec.environment, environment);
        assert_eq!(spec.working_directory, None);
    }
}

#[test]
fn default_and_option_only_profiles_can_run_auxiliary_commands() {
    for shell in [
        Shell::Program("wsl.exe".to_owned()),
        profile(&["-d", "Debian", "-u", "root"]),
    ] {
        let spec = launch_spec(&HashMap::new(), &shell, vec!["/bin/sh".to_owned()]);
        let (_, original_args) = shell.program_and_args();
        let expected = original_args
            .iter()
            .cloned()
            .chain(["--exec".to_owned(), "/bin/sh".to_owned()])
            .collect::<Vec<_>>();
        assert_eq!(spec.args, expected);
    }
}

#[cfg(unix)]
mod process_tests {
    use super::*;
    use std::{fs, io::Cursor, os::unix::fs::PermissionsExt};

    const LAUNCHER: &str = include_str!("../../ssh_image_paste/wsl/test-launcher.sh");
    const MKTEMP: &str = include_str!("../../ssh_image_paste/wsl/test-mktemp.sh");
    const SSH: &str = include_str!("../../ssh_image_paste/wsl/test-ssh.sh");

    fn executable(directory: &std::path::Path, name: &str, script: &str) -> PathBuf {
        let path = directory.join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    fn image() -> Image {
        let pixels = image::RgbaImage::from_pixel(2, 3, image::Rgba([10, 20, 30, 255]));
        let mut bytes = Cursor::new(Vec::new());
        pixels
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        Image::from_bytes(gpui::ImageFormat::Png, bytes.into_inner())
    }

    fn handler(
        directory: &std::path::Path,
        environment: HashMap<String, String>,
    ) -> SshImagePasteHandler {
        let launcher = executable(directory, "wsl.exe", LAUNCHER);
        SshImagePasteHandler::new(
            Shell::WithArguments {
                program: launcher.to_string_lossy().into_owned(),
                args: vec![
                    "-d".to_owned(),
                    "Ubuntu 测试".to_owned(),
                    "--exec".to_owned(),
                    "/bin/sh".to_owned(),
                    "-c".to_owned(),
                    "tracking wrapper".to_owned(),
                ],
                title_override: None,
            },
            environment,
            Some(directory.join("nonexistent Windows cwd")),
        )
    }

    #[test]
    fn codex_and_unknown_foreground_processes_receive_private_png_paths() {
        let directory = tempfile::tempdir().unwrap();
        let handler = handler(directory.path(), HashMap::new());
        let image = image();
        let mut staged = Vec::new();
        for foreground in [Some(vec!["codex".to_owned()]), None] {
            let result = handler.paste_image(&image, foreground.as_deref()).unwrap();
            let ImagePasteResult::ResolvedPath(path) = result else {
                panic!("WSL must stage the image rather than send the native shortcut");
            };
            let path = PathBuf::from(path);
            assert!(path.is_absolute());
            assert_eq!(path.file_name().unwrap(), "image.png");
            let bytes = fs::read(&path).unwrap();
            let decoded = image::load_from_memory(&bytes).unwrap();
            assert_eq!((decoded.width(), decoded.height()), (2, 3));
            assert_eq!(bytes, normalize_image(&image).unwrap());
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            staged.push(path);
        }
        assert_ne!(staged[0], staged[1]);
        assert!(staged.iter().all(|path| path.exists()));
        drop(handler);
        let deadline = Instant::now() + Duration::from_secs(5);
        while staged.iter().any(|path| path.parent().unwrap().exists()) && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(staged.iter().all(|path| !path.parent().unwrap().exists()));
    }

    #[test]
    fn failed_wsl_staging_returns_an_error_instead_of_a_native_shortcut() {
        let directory = tempfile::tempdir().unwrap();
        let handler = handler(
            directory.path(),
            HashMap::from([("ZETTA_TEST_WSL_MODE".to_owned(), "fail".to_owned())]),
        );
        let error = handler
            .paste_image(&image(), Some(&["codex".to_owned()]))
            .unwrap_err();
        assert!(format!("{error:#}").contains("staging clipboard image in WSL"));
    }

    #[test]
    fn an_ssh_foreground_process_in_a_wsl_profile_keeps_the_ssh_upload_route() {
        let directory = tempfile::tempdir().unwrap();
        executable(directory.path(), "ssh", SSH);
        // The distribution's own `ssh`, found on its `PATH` — which the test
        // launcher stands in for with this process's.
        let path = std::env::join_paths(
            std::iter::once(directory.path().to_path_buf())
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let handler = handler(
            directory.path(),
            HashMap::from([
                (
                    "TMPDIR".to_owned(),
                    directory.path().to_string_lossy().into_owned(),
                ),
                ("PATH".to_owned(), path.to_string_lossy().into_owned()),
            ]),
        );
        let result = handler
            .paste_image(&image(), Some(&["ssh host.example".to_owned()]))
            .unwrap();
        let ImagePasteResult::ResolvedPath(path) = result else {
            panic!("the SSH upload must return its remote image path");
        };
        let path = PathBuf::from(path);
        // The SSH store honors TMPDIR; the WSL store uses its own /tmp. This
        // proves the fallback did not intercept a recognized SSH foreground.
        assert!(path.starts_with(directory.path()));
        assert_eq!(fs::read(&path).unwrap(), normalize_image(&image()).unwrap());
        drop(handler);
        let deadline = Instant::now() + Duration::from_secs(5);
        while path.parent().unwrap().exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!path.parent().unwrap().exists());
    }

    /// A reported `ssh` path is title text; the one it names is never run. The
    /// paste is handled as for any other foreground process instead.
    #[test]
    fn a_reported_ssh_path_in_a_wsl_profile_is_not_run() {
        let directory = tempfile::tempdir().unwrap();
        let planted = executable(
            directory.path(),
            "ssh",
            "#!/bin/sh\ntouch \"$0.ran\"\nexit 1\n",
        );
        let handler = handler(directory.path(), HashMap::new());
        let result = handler
            .paste_image(
                &image(),
                Some(&[format!("{} host.example", planted.display())]),
            )
            .unwrap();
        let ImagePasteResult::ResolvedPath(path) = result else {
            panic!("WSL must stage the image rather than send the native shortcut");
        };
        assert!(!PathBuf::from(path).starts_with(directory.path()));
        assert!(!planted.with_extension("ran").exists());
    }

    #[test]
    fn invalid_images_fail_before_starting_wsl() {
        let handler =
            SshImagePasteHandler::new(Shell::Program("wsl.exe".to_owned()), HashMap::new(), None);
        for image in [
            Image::from_bytes(gpui::ImageFormat::Png, Vec::new()),
            Image::from_bytes(gpui::ImageFormat::Svg, b"<svg/>".to_vec()),
        ] {
            let error = handler.paste_image(&image, None).unwrap_err();
            assert!(format!("{error:#}").contains("clipboard image"));
            assert!(!format!("{error:#}").contains("starting image-paste process"));
        }
    }

    #[test]
    fn truncated_staging_removes_the_incomplete_directory() {
        let directory = tempfile::tempdir().unwrap();
        executable(directory.path(), "mktemp", MKTEMP);
        let staging_directory = directory.path().join("staged");
        let path = std::env::join_paths(
            std::iter::once(directory.path().to_path_buf())
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let spec = LaunchSpec {
            program: "/bin/sh".to_owned(),
            args: vec![
                "-c".to_owned(),
                STAGE_IMAGE.to_owned(),
                "zetta-image-paste".to_owned(),
                "100".to_owned(),
                "sentinel".to_owned(),
            ],
            environment: HashMap::from([
                ("PATH".to_owned(), path.to_string_lossy().into_owned()),
                (
                    "ZETTA_TEST_DIRECTORY".to_owned(),
                    staging_directory.to_string_lossy().into_owned(),
                ),
            ]),
            working_directory: None,
        };
        assert!(
            run_image_paste_process(spec, b"truncated".to_vec(), Duration::from_secs(5)).is_err()
        );
        assert!(staging_directory.with_extension("created").is_file());
        assert!(!staging_directory.exists());
    }

    #[test]
    fn auxiliary_wsl_processes_have_a_hard_timeout() {
        let directory = tempfile::tempdir().unwrap();
        let launcher = executable(directory.path(), "wsl.exe", LAUNCHER);
        let spec = launch_spec(
            &HashMap::from([("ZETTA_TEST_WSL_MODE".to_owned(), "timeout".to_owned())]),
            &Shell::Program(launcher.to_string_lossy().into_owned()),
            vec!["/bin/sh".to_owned()],
        );
        let error =
            run_image_paste_process(spec, Vec::new(), Duration::from_millis(40)).unwrap_err();
        assert!(format!("{error:#}").contains("timed out"));
    }
}
