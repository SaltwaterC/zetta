use super::*;

fn probes(markers: &[&str], native_backend: bool) -> bool {
    should_probe_with(|name| markers.contains(&name), native_backend)
}

#[test]
fn local_zetta_shell_uses_native_backend() {
    assert!(!probes(&["ZETTA_TERM"], true));
    assert!(probes(&["ZETTA_TERM"], false));
}

#[test]
fn ssh_and_zosh_shells_probe_with_either_build() {
    for marker in ["SSH_CONNECTION", "SSH_TTY", "ZOSH_CLIPBOARD_CHANNEL"] {
        assert!(probes(&[marker], true));
        assert!(probes(&[marker], false));
    }
}

#[test]
fn host_backend_guard_prevents_recursive_probe() {
    assert!(!probes(&["ZCLIP_HOST_BACKEND", "SSH_CONNECTION"], true));
    assert!(!probes(&["ZCLIP_HOST_BACKEND", "ZETTA_TERM"], false));
}
