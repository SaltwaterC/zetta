use super::*;

#[test]
fn proxy_forwards_the_original_arguments_without_rewriting_them() {
    let command = MoshCommand {
        raw_arguments: vec![
            "--family=prefer-inet6".into(),
            "pi@adsb".into(),
            "--".into(),
            "zsh".into(),
            "-l".into(),
        ],
        ..MoshCommand::default()
    };
    assert_eq!(
        forwarded_arguments(&command),
        command.raw_arguments,
        "zetta mosh must not reinterpret arguments before zosh sees them"
    );
}

#[test]
fn proxy_reconstructs_help_and_version_for_directly_built_commands() {
    let help = MoshCommand {
        help: true,
        ..MoshCommand::default()
    };
    assert_eq!(forwarded_arguments(&help), vec![OsString::from("--help")]);

    let version = MoshCommand {
        version: true,
        ..MoshCommand::default()
    };
    assert_eq!(
        forwarded_arguments(&version),
        vec![OsString::from("--version")]
    );
}
