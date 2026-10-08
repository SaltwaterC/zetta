use super::*;

/// A host key and the fingerprint `ssh-keygen -l` prints for it.
const HOST_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMrL6c1FTc/GNuTe6119TWVfkgP8cN1fl0Ltv6o83zsA root@sealing-host";
const HOST_KEY_FINGERPRINT: &str = "SHA256:bTBKZhY1f9R2YWiSdzVWcmJworfDpk06n3w9YpZPWJU";

#[test]
fn a_public_key_fingerprint_is_the_one_openssh_prints() {
    assert_eq!(
        public_key_fingerprint(HOST_KEY).as_deref(),
        Some(HOST_KEY_FINGERPRINT)
    );
}

#[test]
fn a_key_listed_under_another_type_has_no_fingerprint() {
    let mislabelled = HOST_KEY.replacen("ssh-ed25519", "ssh-rsa", 1);
    assert_eq!(public_key_fingerprint(&mislabelled), None);
    assert_eq!(public_key_fingerprint("ssh-ed25519 not-base64"), None);
    assert_eq!(public_key_fingerprint("ssh-ed25519"), None);
}

#[test]
fn known_hosts_entries_skip_authorities_and_collect_revocations() {
    let key = HOST_KEY.split_whitespace().nth(1).unwrap();
    let output = format!(
        "# Host example.com found: line 1\n\
         example.com ssh-ed25519 {key} comment\n\
         |1|c2FsdA==|aGFzaA== ssh-ed25519 {key}\n\
         @cert-authority *.example.com ssh-ed25519 {key}\n\
         @revoked example.com ssh-ed25519 {key}\n\
         example.com ssh-ed25519 garbage\n"
    );

    let entries = parse_known_hosts_entries(&output);

    assert_eq!(entries.trusted, vec![HOST_KEY_FINGERPRINT; 2]);
    assert_eq!(entries.revoked, vec![HOST_KEY_FINGERPRINT]);
}

#[test]
fn the_lookup_name_follows_alias_then_port() {
    let plain = KnownHostsLookup::parse(
        "user alice\nhostname build.example.com\nport 22\n\
         userknownhostsfile /k/known_hosts /k/known_hosts2\n\
         globalknownhostsfile /etc/ssh/ssh_known_hosts\n",
    )
    .unwrap();
    assert_eq!(plain.name, "build.example.com");
    assert_eq!(
        plain.files,
        [
            "/k/known_hosts",
            "/k/known_hosts2",
            "/etc/ssh/ssh_known_hosts"
        ]
        .map(PathBuf::from)
    );

    let forwarded = KnownHostsLookup::parse("hostname localhost\nport 2222\n").unwrap();
    assert_eq!(forwarded.name, "[localhost]:2222");

    let aliased =
        KnownHostsLookup::parse("hostname 10.0.0.5\nport 2222\nhostkeyalias build\n").unwrap();
    assert_eq!(aliased.name, "build");

    assert!(KnownHostsLookup::parse("port 22\n").is_err());
    let ignored = KnownHostsLookup::parse(
        "hostname h\nuserknownhostsfile /dev/null\nglobalknownhostsfile none\n",
    )
    .unwrap();
    assert!(ignored.files.is_empty());
}

/// End to end against the real `ssh-keygen`, with `ssh -G` stood in for so the
/// test does not depend on this machine's SSH configuration — including a
/// hashed `known_hosts`, which only `ssh-keygen` can match.
#[cfg(unix)]
#[test]
fn known_fingerprints_come_from_the_destinations_known_hosts_entry() {
    use std::os::unix::fs::PermissionsExt as _;

    if Command::new("ssh-keygen").arg("-?").output().is_err() {
        eprintln!("skipping: no ssh-keygen");
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let known_hosts = directory.path().join("known_hosts");
    let other_key =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIIF4LqMO/Yo/8s+yfHmnKWfKl4bgBe9XtA8LnDR8k8EZ";
    let key = HOST_KEY
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    std::fs::write(
        &known_hosts,
        format!("[build.example.com]:2222 {key}\nelsewhere.example.com {other_key}\n"),
    )
    .unwrap();
    let hashed = Command::new("ssh-keygen")
        .args(["-q", "-H", "-f"])
        .arg(&known_hosts)
        .output()
        .unwrap();
    assert!(hashed.status.success());
    let ssh = directory.path().join("ssh");
    std::fs::write(
        &ssh,
        format!(
            "#!/bin/sh\nprintf 'hostname build.example.com\\nport 2222\\nuserknownhostsfile {}\\n'\n",
            known_hosts.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).unwrap();

    let target = RemoteTarget::new("build").with_port(Some(2222));
    let trusted = known_fingerprints_with(&ssh, Path::new("ssh-keygen"), &target).unwrap();

    assert_eq!(trusted, vec![HOST_KEY_FINGERPRINT.to_owned()]);
}
