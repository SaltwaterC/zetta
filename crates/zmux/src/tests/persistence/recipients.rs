use super::*;

#[test]
fn classical_age_round_trip_is_interoperable_with_the_age_crate() {
    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public().to_string();
    let recipients = RecipientSet::parse(&[recipient]).unwrap();
    let ciphertext = recipients.encrypt(b"classical session state").unwrap();
    assert!(ciphertext.starts_with(b"age-encryption.org/v1\n"));
    let plaintext = age::decrypt(&identity, &ciphertext).unwrap();
    assert_eq!(plaintext, b"classical session state");
}

#[test]
fn ssh_ed25519_and_rsa_recipients_use_age_validation() {
    let ed25519 =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHsKLqeplhpW+uObz5dvMgjz1OxfM/XXUB+VHtZ6isGN";
    let rsa = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQDE7nIXTGNuaRBN9toI/wNALuQec8mvlt0iJ7o3OaD2UvoKHJ7S8rmIn4FiQDUed/Vac3OhUibei1k+TBmm16u2Rj3klgWZOIDgi8d4vXKI5N3YBhxr3jsQ+kz1c+iZ4z/tTtz306+4K46XViVMWwyyg9j82Jn41mOAy9vdeDIfQ5fLeaGqn5KwlT61GNkZ+ozWK/ZNlQIlNCcoXxhJULIs9XrtczWyVBAea1nlDo0WHODePxoJjmsNHrpQXn5mf9O83xs10qfTUjnRUt48jRmedFy4tcra3QGmSTQ3KZne+wXXSb0cIpXLGvZjQSPHgG1hc4r3uBpiSzvesGLv79XL";
    assert!(parse_recipient(ed25519).is_ok());
    assert!(parse_recipient(rsa).is_ok());
}

#[test]
fn encrypted_store_has_no_files_without_recipients() {
    let directory = tempfile::tempdir().unwrap();
    assert!(
        PersistenceStore::open(directory.path(), &[])
            .unwrap()
            .is_none()
    );
    assert!(!directory.path().join("persistence").exists());
}

#[test]
fn github_entries_are_validated_without_logging_or_networking_in_the_parser() {
    for username in ["", "-zetta", "zetta-", "zetta--user", "zetta/user"] {
        assert!(validate_github_username(username).is_err(), "{username:?}");
    }
    assert!(validate_github_username("zetta-user").is_ok());

    let body = b"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEinvalid\n";
    assert!(parse_github_keys(body).is_err());
    let unsupported = b"ssh-dss AAAAB3NzaC1kc3MAAACBfake\n";
    assert!(parse_github_keys(unsupported).is_err());
}

#[test]
fn recipient_resolution_preserves_temporary_and_permanent_failures() {
    let temporary = resolve_recipient_strings_with(&["github:zetta-user".to_owned()], |_| {
        Err(RecipientResolutionError::Temporary(anyhow::anyhow!(
            "DNS lookup failed"
        )))
    })
    .unwrap_err();
    assert!(temporary.is_temporary());
    assert!(temporary.to_string().contains("DNS lookup failed"));

    let permanent = resolve_recipient_strings_with(&["github:zetta-user".to_owned()], |_| {
        Err(RecipientResolutionError::Permanent(anyhow::anyhow!(
            "malformed GitHub SSH key"
        )))
    })
    .unwrap_err();
    assert!(!permanent.is_temporary());
    assert!(permanent.to_string().contains("malformed GitHub SSH key"));
}

#[test]
fn invalid_direct_recipients_are_rejected_before_a_github_lookup() {
    let error = resolve_recipient_strings_with(
        &[
            "github:zetta-user".to_owned(),
            "not-an-age-recipient".to_owned(),
        ],
        |_| panic!("a permanent local configuration error must not fetch GitHub"),
    )
    .unwrap_err();
    assert!(!error.is_temporary());
    assert!(error.to_string().contains("invalid age recipient"));
}

#[test]
fn github_retryable_statuses_are_distinguished_from_configuration_responses() {
    for status in [
        reqwest::StatusCode::REQUEST_TIMEOUT,
        reqwest::StatusCode::TOO_EARLY,
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        reqwest::StatusCode::BAD_GATEWAY,
    ] {
        assert!(is_retryable_github_status(status), "{status}");
    }
    for status in [
        reqwest::StatusCode::BAD_REQUEST,
        reqwest::StatusCode::UNAUTHORIZED,
        reqwest::StatusCode::NOT_FOUND,
    ] {
        assert!(!is_retryable_github_status(status), "{status}");
    }
}
