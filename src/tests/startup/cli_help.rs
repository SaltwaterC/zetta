#[cfg(feature = "tftp-client")]
use super::super::arg_parsing::parse_args_from;
use super::*;

#[test]
fn version_flags_and_output_are_defined() {
    assert!(is_version_argument("-v"));
    assert!(is_version_argument("--version"));
    assert!(!is_version_argument("-V"));
    assert_eq!(
        version_text(),
        format!("Zetta {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn help_text_uses_title_case_and_lists_built_in_features() {
    let profiles = [
        Profile {
            name: "System".to_owned(),
            command: Shell::System,
            theme: None,
        },
        Profile {
            name: "Operations".to_owned(),
            command: Shell::Program("zsh".to_owned()),
            theme: None,
        },
    ];
    let help = help_text(&profiles);
    assert!(help.starts_with("Zetta Terminal\n"));
    assert!(help.contains("Built-in features:\n  Terminal emulator"));
    #[cfg(feature = "syntax-highlighting")]
    assert!(help.contains("Vi syntax highlighting"));
    #[cfg(not(feature = "syntax-highlighting"))]
    assert!(!help.contains("Vi syntax highlighting"));
    assert!(help.contains("Profiles accepted by --profile NAME (case-insensitive):"));
    assert!(help.contains("  System\n  Operations"));
    assert!(help.contains("Select one of the profiles listed above"));
    assert!(help.contains("-s, --split NAME"));
    assert!(help.contains("zetta splits"));
    assert!(
        help.contains("splits                              List configured pane split templates")
    );
    assert!(help.contains("run `zetta splits` to list available names"));
    assert!(help.contains("zetta terminal-size [--json | --resize"));
    assert!(help.contains("zetta tabicon [OPTIONS] ICON"));
    assert!(help.contains("tabicon                             Set the active tab icon"));
    assert!(help.contains("zetta overlay [OPTIONS] TEXT"));
    assert!(help.contains(
        "overlay                             Non-persistently show text over the active pane"
    ));
    assert!(help.contains("zetta vi [OPTIONS] [FILE ...]"));
    assert!(help.contains("zetta edit [OPTIONS] [--] FILE ..."));
    assert!(help.contains("edit                                Edit files with $EDITOR"));
    assert!(
        help.contains("vi                                  Edit files with Zetta's built-in vi")
    );
    assert!(
        help.contains(
            "terminal-size                       Print or resize the current terminal pane"
        )
    );
    assert!(help.contains("zetta init [SHELL]"));
    assert!(
        help.contains(
            "init                                Configure or generate shell integration"
        )
    );
    assert!(pane_splits_help().contains("Usage: zetta splits"));
    assert!(pane_splits_help().contains("--split or -s"));

    #[cfg(all(feature = "wayland", linux_like))]
    assert!(help.contains("Wayland backend"));
    #[cfg(not(all(feature = "wayland", linux_like)))]
    assert!(!help.contains("Wayland backend"));

    #[cfg(all(feature = "x11", linux_like))]
    assert!(help.contains("X11 backend"));
    #[cfg(not(all(feature = "x11", linux_like)))]
    assert!(!help.contains("X11 backend"));

    #[cfg(feature = "serial-console")]
    {
        assert!(help.contains("Serial console"));
        assert!(help.contains("zetta serial <COMMAND>"));
        assert!(
            help.contains("serial                              List or connect to serial devices")
        );
    }
    #[cfg(not(feature = "serial-console"))]
    assert!(!help.contains("Serial console"));

    #[cfg(feature = "http-server")]
    {
        assert!(help.contains("HTTP server"));
        assert!(help.contains("zetta http server [OPTIONS]"));
        assert!(help.contains("http server                         Serve static files over HTTP"));
    }
    #[cfg(not(feature = "http-server"))]
    assert!(!help.contains("HTTP server"));

    #[cfg(feature = "tftp-server")]
    {
        assert!(help.contains("TFTP server"));
        assert!(help.contains("zetta tftp <COMMAND>"));
    }
    #[cfg(not(feature = "tftp-server"))]
    assert!(!help.contains("TFTP server"));

    #[cfg(tftp_enabled)]
    {
        #[cfg(feature = "tftp-client")]
        assert!(help.contains("TFTP client"));
        assert!(help.contains("zetta tftp <COMMAND>"));
    }
    #[cfg(not(tftp_enabled))]
    {
        assert!(!help.contains("TFTP client"));
        assert!(!help.contains("zetta tftp <COMMAND>"));
    }

    #[cfg(feature = "notifications")]
    {
        assert!(help.contains("Desktop notifications"));
        assert!(help.contains("zetta notify [OPTIONS] SUMMARY [BODY]"));
        assert!(help.contains("notify                              Show a desktop notification"));
    }
    #[cfg(not(feature = "notifications"))]
    {
        assert!(!help.contains("Desktop notifications"));
        assert!(!help.contains("zetta notify"));
    }
}

#[cfg(feature = "tftp-client")]
#[test]
fn tftp_subcommand_is_parsed_without_starting_the_application() {
    let args = parse_args_from([
        OsString::from("tftp"),
        OsString::from("get"),
        OsString::from("--port"),
        OsString::from("1069"),
        OsString::from("localhost"),
        OsString::from("boot.bin"),
        OsString::from("download.bin"),
    ])
    .unwrap();

    assert_eq!(
        args.tftp_command,
        Some(TftpCommand::Get {
            host: "localhost".to_owned(),
            remote: "boot.bin".to_owned(),
            local: PathBuf::from("download.bin"),
            port: 1069,
        })
    );
}
