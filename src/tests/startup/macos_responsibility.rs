use super::*;

#[test]
fn reexec_preserves_raw_environment_and_replaces_only_its_pid_marker() {
    use std::os::unix::ffi::OsStringExt as _;
    let environment = launch_environment(
        [
            ("PATH".into(), "/custom/bin:/usr/bin".into()),
            ("RAW".into(), OsString::from_vec(vec![0xff, b'=', b'x'])),
            (INITIALIZED_PID.into(), "123".into()),
        ],
        456,
    )
    .unwrap();
    let bytes = environment
        .iter()
        .map(|value| value.as_bytes())
        .collect::<Vec<_>>();
    assert_eq!(
        bytes,
        [
            b"PATH=/custom/bin:/usr/bin".as_slice(),
            b"RAW=\xff=x",
            b"ZETTA_MACOS_RESPONSIBLE_PID=456"
        ]
    );
}
