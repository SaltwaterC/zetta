use std::ffi::OsString;

use super::*;
use crate::cli_services::CliServiceCommand;

#[test]
fn copy_parser_accepts_pboard_and_rejects_unknown_options() {
    assert_eq!(
        parse_copy_args([]).unwrap(),
        CliServiceCommand::Copy(CopyCommand)
    );
    assert_eq!(
        parse_copy_args([OsString::from("-pboard"), OsString::from("general")]).unwrap(),
        CliServiceCommand::Copy(CopyCommand)
    );
    assert_eq!(
        parse_copy_args([OsString::from("--pboard"), OsString::from("font")]).unwrap(),
        CliServiceCommand::Copy(CopyCommand)
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
fn copy_parser_recognizes_the_internal_daemon_flag() {
    assert_eq!(
        parse_copy_args([OsString::from(CLIPBOARD_DAEMON_FLAG)]).unwrap(),
        CliServiceCommand::CopyDaemon
    );
}

#[test]
fn paste_parser_accepts_pboard_and_prefer_and_rejects_unknown_options() {
    assert_eq!(
        parse_paste_args([]).unwrap(),
        CliServiceCommand::Paste(PasteCommand)
    );
    assert_eq!(
        parse_paste_args([OsString::from("-pboard"), OsString::from("ruler")]).unwrap(),
        CliServiceCommand::Paste(PasteCommand)
    );
    assert_eq!(
        parse_paste_args([OsString::from("-Prefer"), OsString::from("rtf")]).unwrap(),
        CliServiceCommand::Paste(PasteCommand)
    );
    assert_eq!(
        parse_paste_args([OsString::from("--prefer"), OsString::from("txt")]).unwrap(),
        CliServiceCommand::Paste(PasteCommand)
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
