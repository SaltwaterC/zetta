use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

use super::*;
use crate::cli_services::CliServiceCommand;

/// A `security.capability` value laid out as the kernel writes it.
fn capability_data(revision: u32, effective: bool, permitted: u32, root_id: u32) -> Vec<u8> {
    let magic = revision
        | if effective {
            VFS_CAP_FLAGS_EFFECTIVE
        } else {
            0
        };
    let mut words = vec![magic, permitted, 0];
    if revision != VFS_CAP_REVISION_1 {
        words.extend([0, 0]);
    }
    if revision == VFS_CAP_REVISION_3 {
        words.push(root_id);
    }
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}

const NET_BIND_SERVICE: u32 = 1 << CAP_NET_BIND_SERVICE;

#[test]
fn net_bind_service_with_the_effective_flag_is_granted_in_every_revision() {
    for revision in [VFS_CAP_REVISION_1, VFS_CAP_REVISION_2, VFS_CAP_REVISION_3] {
        assert!(
            grants_net_bind_service(&capability_data(revision, true, NET_BIND_SERVICE, 0)),
            "revision {revision:#x}"
        );
    }
    // What `setcap cap_net_bind_service=+ep` writes as root, byte for byte.
    assert!(grants_net_bind_service(&[
        0x01, 0x00, 0x00, 0x02, 0x00, 0x04, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ]));
}

#[test]
fn a_capability_that_would_not_let_the_server_bind_is_missing() {
    // Permitted but not effective: `+p` alone leaves the process unable to use it.
    assert!(!grants_net_bind_service(&capability_data(
        VFS_CAP_REVISION_2,
        false,
        NET_BIND_SERVICE,
        0
    )));
    // Some other capability.
    assert!(!grants_net_bind_service(&capability_data(
        VFS_CAP_REVISION_2,
        true,
        1 << 13,
        0
    )));
    // Scoped to another user namespace's root.
    assert!(!grants_net_bind_service(&capability_data(
        VFS_CAP_REVISION_3,
        true,
        NET_BIND_SERVICE,
        100_000
    )));
}

#[test]
fn malformed_capability_values_are_not_trusted() {
    let mut truncated = capability_data(VFS_CAP_REVISION_2, true, NET_BIND_SERVICE, 0);
    truncated.pop();
    assert!(!grants_net_bind_service(&truncated));
    // A revision 3 header on a revision 2 length.
    let mut mislabelled = capability_data(VFS_CAP_REVISION_2, true, NET_BIND_SERVICE, 0);
    mislabelled[3] = 0x03;
    assert!(!grants_net_bind_service(&mislabelled));
    assert!(!grants_net_bind_service(&capability_data(
        0x0400_0000,
        true,
        NET_BIND_SERVICE,
        0
    )));
    assert!(!grants_net_bind_service(&[]));
}

#[test]
fn setcap_parser_accepts_check_once_and_nothing_else() {
    let parse = |arguments: &[&str]| parse_tftp_setcap_args(arguments.iter().map(OsString::from));
    assert_eq!(
        parse(&[]).unwrap(),
        CliServiceCommand::TftpSetcap(TftpSetcapCommand { check_only: false })
    );
    for check in ["--check", "-c"] {
        assert_eq!(
            parse(&[check]).unwrap(),
            CliServiceCommand::TftpSetcap(TftpSetcapCommand { check_only: true })
        );
    }
    assert!(parse(&["--check", "-c"]).is_err());
    assert!(parse(&["--force"]).is_err());
    assert!(parse(&["/usr/bin/zetta"]).is_err());
}

#[test]
fn setcap_grants_each_binary_in_one_invocation() {
    let arguments = setcap_arguments(&[PathBuf::from("/a/zetta"), PathBuf::from("/a/ztftp")]);
    assert_eq!(
        arguments,
        [
            "cap_net_bind_service=+ep",
            "/a/zetta",
            "cap_net_bind_service=+ep",
            "/a/ztftp"
        ]
        .map(OsString::from)
    );
}

#[test]
fn setcap_is_found_on_path_before_the_sbin_fallbacks_and_must_be_executable() {
    let directory = tempfile::tempdir().unwrap();
    let not_executable = directory.path().join("plain");
    let executable = directory.path().join("bin");
    for (path, mode) in [(&not_executable, 0o644), (&executable, 0o755)] {
        fs::create_dir(path).unwrap();
        let setcap = path.join("setcap");
        fs::write(&setcap, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&setcap, fs::Permissions::from_mode(mode)).unwrap();
    }
    let path = std::env::join_paths([&not_executable, &executable]).unwrap();
    assert_eq!(find_setcap(Some(&path)), Some(executable.join("setcap")));

    // Without a usable PATH entry the answer can only come from the fallbacks.
    let fallback = find_setcap(Some(not_executable.as_os_str()));
    assert!(fallback.is_none_or(|found| {
        SETCAP_FALLBACK_DIRECTORIES
            .iter()
            .any(|directory| found == Path::new(directory).join("setcap"))
    }));
}

#[test]
fn printed_commands_quote_only_the_words_a_shell_would_split() {
    let words = [
        "sudo",
        "/usr/sbin/setcap",
        "cap_net_bind_service=+ep",
        "/home/a b/zetta",
        "it's",
    ]
    .map(OsString::from);
    assert_eq!(
        display_command(&words),
        r"sudo /usr/sbin/setcap cap_net_bind_service=+ep '/home/a b/zetta' 'it'\''s'"
    );
}

#[test]
fn unprivileged_port_start_is_read_from_the_sysctl_file() {
    assert_eq!(parse_unprivileged_port_start("1024\n"), Some(1024));
    assert_eq!(parse_unprivileged_port_start("0"), Some(0));
    assert_eq!(parse_unprivileged_port_start(""), None);
}

#[test]
fn a_file_without_the_attribute_has_no_capability() {
    let file = tempfile::NamedTempFile::new().unwrap();
    // tmpfs and the usual home filesystems support xattrs; a sandbox that
    // does not is reported as an error rather than as "missing".
    if let Ok(capability) = read_capability(file.path()) {
        assert_eq!(capability, None);
    }
}
