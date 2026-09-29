use std::ffi::OsString;

use super::*;
use crate::cli_services::CliServiceCommand;

#[test]
fn copy_parser_accepts_pboard_and_rejects_unknown_options() {
    assert_eq!(
        parse_copy_args([]).unwrap(),
        CliServiceCommand::Copy(CopyCommand { args: vec![] })
    );
    assert_eq!(
        parse_copy_args([OsString::from("-pboard"), OsString::from("general")]).unwrap(),
        CliServiceCommand::Copy(CopyCommand {
            args: vec![OsString::from("-pboard"), OsString::from("general")]
        })
    );
    assert_eq!(
        parse_copy_args([OsString::from("--pboard"), OsString::from("font")]).unwrap(),
        CliServiceCommand::Copy(CopyCommand {
            args: vec![OsString::from("--pboard"), OsString::from("font")]
        })
    );

    assert!(parse_copy_args([OsString::from("-pboard"), OsString::from("invalid")]).is_err());
    assert!(parse_copy_args([OsString::from("--unknown")]).is_err());
    assert!(parse_copy_args([OsString::from("unexpected")]).is_err());
    assert!(
        parse_copy_args([
            OsString::from("-pboard"),
            OsString::from("general"),
            OsString::from("-pboard"),
            OsString::from("general"),
        ])
        .is_err()
    );
}

#[test]
fn paste_parser_accepts_pboard_and_prefer_and_rejects_unknown_options() {
    assert_eq!(
        parse_paste_args([]).unwrap(),
        CliServiceCommand::Paste(PasteCommand { args: vec![] })
    );
    assert_eq!(
        parse_paste_args([OsString::from("-pboard"), OsString::from("ruler")]).unwrap(),
        CliServiceCommand::Paste(PasteCommand {
            args: vec![OsString::from("-pboard"), OsString::from("ruler")]
        })
    );
    assert_eq!(
        parse_paste_args([OsString::from("-Prefer"), OsString::from("rtf")]).unwrap(),
        CliServiceCommand::Paste(PasteCommand {
            args: vec![OsString::from("-Prefer"), OsString::from("rtf")]
        })
    );
    assert_eq!(
        parse_paste_args([OsString::from("--prefer"), OsString::from("txt")]).unwrap(),
        CliServiceCommand::Paste(PasteCommand {
            args: vec![OsString::from("--prefer"), OsString::from("txt")]
        })
    );

    assert!(parse_paste_args([OsString::from("-pboard"), OsString::from("invalid")]).is_err());
    assert!(parse_paste_args([OsString::from("-Prefer"), OsString::from("invalid")]).is_err());
    assert!(parse_paste_args([OsString::from("--unknown")]).is_err());
    assert!(parse_paste_args([OsString::from("unexpected")]).is_err());
}

#[test]
fn copy_and_paste_help_mention_the_pbcopy_and_pbpaste_flags() {
    assert!(copy_help().contains("-pboard"));
    assert!(paste_help().contains("-pboard"));
    assert!(paste_help().contains("-Prefer"));
}

#[test]
fn missing_clipboard_helper_has_install_guidance() {
    let path = std::env::temp_dir().join("missing-zcopy-helper-for-test");
    let error = run_helper_at(&path, "zcopy", &[]).unwrap_err();
    assert!(format!("{error:#}").contains("install the clipboard helpers beside zetta"));
}

#[cfg(unix)]
#[test]
fn proxy_passes_arguments_to_sibling_helper() {
    let directory = tempfile::tempdir().unwrap();
    let helper = directory.path().join("zcopy");
    let args_file = directory.path().join("args");
    let script = format!(
        r#"#!/bin/sh
printf '%s\n' "$@" > '{}'
"#,
        args_file.display()
    );
    write_script(&helper, &script);
    run_helper_at(
        &helper,
        "zcopy",
        &[OsString::from("-pboard"), OsString::from("font")],
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(args_file).unwrap(),
        "-pboard\nfont\n"
    );
}

#[cfg(unix)]
#[test]
fn failing_clipboard_helper_is_reported() {
    let directory = tempfile::tempdir().unwrap();
    let helper = directory.path().join("zpaste");
    write_script(&helper, "#!/bin/sh\nexit 7\n");
    let error = format!("{:#}", run_helper_at(&helper, "zpaste", &[]).unwrap_err());
    assert!(
        error.contains("zpaste failed with exit status: 7"),
        "{error}"
    );
}

/// Writes an executable test script and waits until it can be run.
///
/// A file just written can briefly refuse to execute (`ETXTBSY`): a process
/// another test forks while the file is still open for writing inherits that
/// descriptor until it execs. So run the script once, harmlessly, until the
/// kernel lets it — after the first success no writer can appear again. The
/// guard line is what makes that run harmless.
#[cfg(unix)]
fn write_script(path: &std::path::Path, content: &str) {
    use std::os::unix::fs::PermissionsExt as _;

    const PROBE: &str = "--zetta-test-probe";
    let body = content
        .strip_prefix("#!/bin/sh\n")
        .expect("test scripts are /bin/sh scripts");
    std::fs::write(
        path,
        format!("#!/bin/sh\ntest \"$1\" = {PROBE} && exit 0\n{body}"),
    )
    .unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match Command::new(path).arg(PROBE).status() {
            Err(error)
                if error.kind() == std::io::ErrorKind::ExecutableFileBusy
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            run => {
                assert!(run.unwrap().success(), "the probe run of {path:?} failed");
                return;
            }
        }
    }
}
