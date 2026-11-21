use super::*;

#[test]
fn server_options_require_explicit_upload_permission() {
    let defaults = parse_server_args([]).unwrap();
    assert_eq!(
        defaults,
        ServerCommand {
            root: PathBuf::from("."),
            port: 69,
            writable: false
        }
    );
    for flag in ["--writable", "-w"] {
        let command =
            parse_server_args(["--root", "images", "--port", "1069", flag].map(OsString::from))
                .unwrap();
        assert_eq!(
            command,
            ServerCommand {
                root: PathBuf::from("images"),
                port: 1069,
                writable: true
            }
        );
    }
    for args in [
        vec!["--port", "0"],
        vec!["--port", "65536"],
        vec!["--root"],
        vec!["--writable", "-w"],
        vec!["--port", "69", "-p", "70"],
    ] {
        assert!(parse_server_args(args.into_iter().map(OsString::from)).is_err());
    }
}
