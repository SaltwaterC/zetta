use super::*;
use std::io::Cursor;

#[test]
fn announcements_are_recognised_only_with_the_prefix() {
    assert_eq!(
        parse_line("@wslx send x86_64\n"),
        Some(Announcement::Send("x86_64"))
    );
    assert_eq!(
        parse_line("@wslx ready /run/user/1000/wslx-1-a/agent.sock\r\n"),
        Some(Announcement::Ready("/run/user/1000/wslx-1-a/agent.sock"))
    );
    assert_eq!(
        parse_line("@wslx error no relay is built for riscv64"),
        Some(Announcement::Error("no relay is built for riscv64"))
    );
    assert_eq!(parse_line("wsl: Failed to translate 'C:\\x'\n"), None);
    assert_eq!(parse_line("@wslx ready\n"), None);
    assert_eq!(parse_line("@wslx later today\n"), None);
}

#[test]
fn the_content_hash_is_fnv1a() {
    assert_eq!(content_hash(b""), "cbf29ce484222325");
    assert_eq!(content_hash(b"a"), "af63dc4c8601ec8c");
}

fn images<'a>(x86_64: &'a [u8], aarch64: &'a [u8]) -> RelayImages<'a> {
    RelayImages {
        x86_64: RelayImage::new(x86_64),
        aarch64: RelayImage::new(aarch64),
    }
}

#[test]
fn the_handshake_sends_the_requested_image_and_skips_noise() {
    let script = "wsl: noise before the script ran\n@wslx send aarch64\n@wslx ready /tmp/s\nleft for the bridge";
    let mut input = Cursor::new(script.as_bytes());
    let mut output = Vec::new();
    let socket = handshake(&mut input, &mut output, &images(b"intel", b"arm")).unwrap();
    assert_eq!(socket, "/tmp/s");
    assert_eq!(output, b"3\narm");
    let mut rest = String::new();
    input.read_to_string(&mut rest).unwrap();
    assert_eq!(rest, "left for the bridge");
}

#[test]
fn the_handshake_reports_why_the_relay_did_not_start() {
    let images = images(b"intel", b"arm");
    let failed = handshake(
        &mut Cursor::new("@wslx error no relay is built for riscv64\n"),
        &mut Vec::new(),
        &images,
    );
    assert_eq!(
        failed.unwrap_err().to_string(),
        "no relay is built for riscv64"
    );

    let exited = handshake(&mut Cursor::new(""), &mut Vec::new(), &images);
    assert_eq!(exited.unwrap_err().kind(), io::ErrorKind::UnexpectedEof);

    let unknown = handshake(
        &mut Cursor::new("@wslx send mips\n"),
        &mut Vec::new(),
        &images,
    );
    assert!(unknown.is_err());

    let noise = "noise\n".repeat(MAX_NOISE_LINES + 1);
    let silent = handshake(&mut Cursor::new(noise), &mut Vec::new(), &images);
    assert_eq!(silent.unwrap_err().kind(), io::ErrorKind::InvalidData);
}

#[cfg(unix)]
mod script {
    //! Runs the real script under `/bin/sh` against the real handshake, with a
    //! shell script standing in for the relay binary.

    use super::*;
    use crate::test_support::ScratchDir;
    use std::{
        fs,
        io::BufReader,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        process::{Command, Stdio},
    };

    /// Prints what the real relay prints once it is listening, naming its
    /// argument and the image so a test can tell which one ran.
    fn fake_relay(name: &str) -> Vec<u8> {
        format!("#!/bin/sh\necho \"@wslx ready $1-{name}\"\n").into_bytes()
    }

    /// Runs the script once with `home` as `$HOME`, returning the socket it
    /// announced and whether it asked for an image.
    fn bootstrap(home: &Path, images: &RelayImages<'_>) -> io::Result<(String, bool)> {
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg(SCRIPT)
            .args(images.script_arguments())
            .env("HOME", home)
            .env_remove("XDG_CACHE_HOME")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let mut input = BufReader::new(child.stdout.take().unwrap());
        let mut sent = Vec::new();
        let socket = {
            let mut stdin = child.stdin.take().unwrap();
            let mut recorder = Recorder(&mut stdin, &mut sent);
            handshake(&mut input, &mut recorder, images)
        };
        child.wait()?;
        Ok((socket?, !sent.is_empty()))
    }

    struct Recorder<'a, W>(&'a mut W, &'a mut Vec<u8>);

    impl<W: Write> Write for Recorder<'_, W> {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.1.extend_from_slice(bytes);
            self.0.write_all(bytes)?;
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }

    fn host_arch() -> &'static str {
        match std::env::consts::ARCH {
            "x86_64" => "x86_64",
            "aarch64" => "aarch64",
            other => panic!("no relay architecture for a {other} test host"),
        }
    }

    fn cache(home: &Path) -> PathBuf {
        home.join(".cache/zetta/wslx")
    }

    fn cached_relays(home: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(cache(home))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_first_run_copies_the_relay_and_later_runs_reuse_it() {
        let home = ScratchDir::new("bootstrap-reuse");
        let (x86_64, aarch64) = (fake_relay("x86_64"), fake_relay("aarch64"));
        let images = images(&x86_64, &aarch64);

        let (socket, sent) = bootstrap(home.path(), &images).unwrap();
        assert_eq!(socket, format!("serve-{}", host_arch()));
        assert!(sent, "the first run must ask for the relay");

        let (socket, sent) = bootstrap(home.path(), &images).unwrap();
        assert_eq!(socket, format!("serve-{}", host_arch()));
        assert!(!sent, "a cached relay must not be sent again");

        let hash = match host_arch() {
            "x86_64" => &images.x86_64.hash,
            _ => &images.aarch64.hash,
        };
        assert_eq!(
            cached_relays(home.path()),
            [format!("wslx-relay-{hash}-{}", host_arch())]
        );
        let mode = fs::metadata(cache(home.path()))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn a_new_build_replaces_the_previous_relay_but_not_a_transfer_in_progress() {
        let home = ScratchDir::new("bootstrap-replace");
        let (old_x86_64, old_aarch64) = (fake_relay("old"), fake_relay("old"));
        bootstrap(home.path(), &images(&old_x86_64, &old_aarch64)).unwrap();
        let in_progress =
            cache(home.path()).join(format!("wslx-relay-0-{}.partial.1", host_arch()));
        fs::write(&in_progress, b"").unwrap();

        let (new_x86_64, new_aarch64) = (fake_relay("new"), fake_relay("new"));
        let new = images(&new_x86_64, &new_aarch64);
        let (socket, sent) = bootstrap(home.path(), &new).unwrap();
        assert_eq!(socket, "serve-new");
        assert!(sent);
        let relays = cached_relays(home.path());
        assert_eq!(relays.len(), 2, "{relays:?}");
        assert!(in_progress.exists());
    }

    #[test]
    fn a_cut_transfer_is_reported_and_never_cached() {
        let home = ScratchDir::new("bootstrap-cut");
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg(SCRIPT)
            .args(["wslx", "a", "a"])
            .env("HOME", home.path())
            .env_remove("XDG_CACHE_HOME")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        input.read_line(&mut line).unwrap();
        assert!(
            matches!(parse_line(&line), Some(Announcement::Send(_))),
            "{line}"
        );
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(b"100\nshort").unwrap();
        drop(stdin);
        line.clear();
        input.read_line(&mut line).unwrap();
        assert_eq!(
            parse_line(&line),
            Some(Announcement::Error(
                "the relay transfer ended after 5 of 100 bytes"
            ))
        );
        child.wait().unwrap();
        assert!(cached_relays(home.path()).is_empty());
    }
}
