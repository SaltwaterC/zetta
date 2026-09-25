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
    use std::os::unix::fs::PermissionsExt as _;
    let directory = tempfile::tempdir().unwrap();
    let helper = directory.path().join("zcopy");
    let args_file = directory.path().join("args");
    let script = format!(
        r#"#!/bin/sh
printf '%s\n' "$@" > '{}'
"#,
        args_file.display()
    );
    std::fs::write(&helper, script).unwrap();
    let mut permissions = std::fs::metadata(&helper).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&helper, permissions).unwrap();
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
    use std::os::unix::fs::PermissionsExt as _;
    let directory = tempfile::tempdir().unwrap();
    let helper = directory.path().join("zpaste");
    std::fs::write(&helper, "#!/bin/sh\nexit 7\n").unwrap();
    let mut permissions = std::fs::metadata(&helper).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&helper, permissions).unwrap();
    let error = run_helper_at(&helper, "zpaste", &[]).unwrap_err();
    assert!(format!("{error:#}").contains("zpaste failed with exit status: 7"));
}
