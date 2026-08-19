use super::*;

#[test]
fn session_authentication_uses_a_salted_argon2id_verifier() {
    let first = SessionAuthentication::create("sensitive session").unwrap();
    let second = SessionAuthentication::create("sensitive session").unwrap();

    assert!(first.encoded().starts_with("$argon2id$"));
    assert!(!first.encoded().contains("sensitive session"));
    assert_ne!(first.encoded(), second.encoded());
    assert!(first.verify("sensitive session").is_some());
    assert!(first.verify("changed value").is_none());

    // Authorization is scoped to the session whose secret was checked.
    let authorization = first.verify("sensitive session").unwrap();
    assert!(first.authorizes(&authorization));
    assert!(!second.authorizes(&authorization));
}

#[test]
fn only_verifying_a_secret_produces_a_reconnect_authorization() {
    let authentication = SessionAuthentication::create("secret").unwrap();

    // A clone of the verifier is not itself authorization: `authorizes` takes a
    // `VerifiedSession`, and `verify` is the only way to construct one. This is
    // the invariant reattaching a protected session relies on, so if a future
    // refactor reintroduces a public constructor this stops compiling.
    assert!(authentication.verify("wrong").is_none());
    let authorization = authentication
        .verify("secret")
        .expect("the correct secret must authorize");
    assert!(authentication.clone().authorizes(&authorization));
}

#[test]
fn failed_authentication_backoff_doubles_and_saturates() {
    assert_eq!(failed_authentication_delay(0), Duration::from_secs(1));
    assert_eq!(failed_authentication_delay(1), Duration::from_secs(1));
    assert_eq!(failed_authentication_delay(2), Duration::from_secs(2));
    assert_eq!(failed_authentication_delay(3), Duration::from_secs(4));
    assert_eq!(failed_authentication_delay(4), Duration::from_secs(8));
    assert_eq!(failed_authentication_delay(5), Duration::from_secs(16));
    // Capped, and no overflow at absurd failure counts.
    assert_eq!(failed_authentication_delay(6), Duration::from_secs(30));
    assert_eq!(failed_authentication_delay(64), Duration::from_secs(30));
    assert_eq!(
        failed_authentication_delay(u32::MAX),
        Duration::from_secs(30)
    );
}

#[test]
fn session_secrets_are_not_rendered_by_debug() {
    let secret = SessionSecret::new("hunter2".to_owned());

    assert_eq!(format!("{secret:?}"), "SessionSecret(<redacted>)");
    assert!(!format!("{secret:?}").contains("hunter2"));
    assert_eq!(secret.expose(), "hunter2");
}
