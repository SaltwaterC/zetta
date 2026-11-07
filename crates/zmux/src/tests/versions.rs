use super::*;

#[test]
fn compatibility_output_lists_every_compiled_contract_once() {
    let text = compatibility_version_text();
    let mut fields = std::collections::BTreeMap::new();
    for line in text.lines() {
        let (name, value) = line.split_once('=').unwrap();
        assert_eq!(value, "1", "{name} must start at the release baseline");
        assert!(fields.insert(name, value).is_none(), "duplicate {name}");
    }
    for name in [
        "CONTROL_VERSION",
        "CATALOG_VERSION",
        "ZMUX_PROTOCOL_VERSION",
        "ZMUX_ENDPOINT_VERSION",
        "ZMUX_SHARED_SESSION_STATE_VERSION",
        "ZMUX_HANDOVER_VERSION",
    ] {
        assert!(fields.contains_key(name), "missing {name}");
    }
    for name in [
        "ZMUX_MANIFEST_VERSION",
        "ZMUX_PERSISTENCE_BINDING_VERSION",
        "ZMUX_SEALED_KEY_VERSION",
    ] {
        assert_eq!(
            fields.contains_key(name),
            cfg!(feature = "session-persistence"),
            "{name}"
        );
    }
    for name in [
        "ZMUX_HOST_PROTOCOL_VERSION",
        "ZMUX_MINIMUM_HOST_PROTOCOL_VERSION",
        "ZMUX_CONSOLE_SERVER_HOST_PROTOCOL_VERSION",
        "ZMUX_HOST_INSTALL_REVISION",
    ] {
        assert_eq!(fields.contains_key(name), cfg!(windows), "{name}");
    }
}
