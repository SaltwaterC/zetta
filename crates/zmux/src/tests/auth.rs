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

/// A verifier with every field of an ordinary one, but the given costs. The
/// hash is not a real one: these are refused before anything is hashed.
fn verifier_with_costs(memory_kib: u32, passes: u32, lanes: u32) -> String {
    format!(
        "$argon2id$v=19$m={memory_kib},t={passes},p={lanes}$c29tZXNhbHRzb21lc2FsdA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
    )
}

#[test]
fn a_created_verifier_is_within_the_import_policy() {
    let created = SessionAuthentication::create("sensitive session").unwrap();
    assert!(created.encoded().contains("$v=19$m=19456,t=2,p=1$"));

    let imported = SessionAuthentication::from_verifier(created.encoded().to_owned())
        .expect("whatever `create` writes must import, or protecting a session breaks");
    assert!(imported.verify("sensitive session").is_some());
}

/// A verifier written by argon2 0.5.3's defaults before the policy existed —
/// the shape every persisted record and handed-over session carries — still
/// imports and still opens with its secret.
#[test]
fn a_verifier_from_before_the_policy_still_imports() {
    // Written by `SessionAuthentication::create("kept secret")` with argon2
    // 0.5.3, its salt fixed so the fixture is reproducible.
    let recorded = "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHRzb21lc2FsdA$8L48kLA1ei86vwDWfBzujaQRnXkgRhEZoLY+1X6H35c".to_owned();

    let imported = SessionAuthentication::from_verifier(recorded).unwrap();
    assert!(imported.verify("kept secret").is_some());
    assert!(imported.verify("another secret").is_none());
}

#[test]
fn a_verifier_asking_for_excessive_costs_is_refused_on_import() {
    for (memory_kib, passes, lanes) in [
        (4 * 1024 * 1024, 2, 1),
        (MAX_VERIFIER_MEMORY_KIB + 1, 2, 1),
        (19456, 3, 1),
        (19456, 1_000_000, 1),
        (19456, 2, 8),
    ] {
        let verifier = verifier_with_costs(memory_kib, passes, lanes);
        let error = SessionAuthentication::from_verifier(verifier.clone())
            .err()
            .unwrap_or_else(|| panic!("{verifier} must be refused"));
        assert!(error.to_string().contains("exceed"), "{error:#}");
    }
    // The ceiling itself is accepted.
    assert!(SessionAuthentication::from_verifier(verifier_with_costs(19456, 2, 1)).is_ok());
    assert!(SessionAuthentication::from_verifier(verifier_with_costs(8192, 1, 1)).is_ok());
}

#[test]
fn a_verifier_of_another_algorithm_or_shape_is_refused_on_import() {
    let salt = "c29tZXNhbHRzb21lc2FsdA";
    let hash = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    for verifier in [
        format!("$argon2i$v=19$m=19456,t=2,p=1${salt}${hash}"),
        format!("$argon2d$v=19$m=19456,t=2,p=1${salt}${hash}"),
        format!("$argon2id$v=16$m=19456,t=2,p=1${salt}${hash}"),
        format!("$argon2id$m=19456,t=2,p=1${salt}${hash}"),
        // An eight-byte salt, and a 16-byte hash.
        format!("$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ${hash}"),
        format!("$argon2id$v=19$m=19456,t=2,p=1${salt}$AAAAAAAAAAAAAAAAAAAAAA"),
        format!("$argon2id$v=19$m=19456,t=2,p=1${salt}"),
        format!("$argon2id$v=19$m=19456,t=2,p=1,keyid=AAAA${salt}${hash}"),
        format!("$scrypt$ln=16,r=8,p=1${salt}${hash}"),
        "not a verifier".to_owned(),
    ] {
        assert!(
            SessionAuthentication::from_verifier(verifier.clone()).is_err(),
            "{verifier} must be refused"
        );
    }
}

/// A handed-over verifier outside the policy keeps its session protected, and
/// is refused by `verify` without Argon2 running — even for its own secret.
#[test]
fn an_adopted_verifier_outside_the_policy_is_never_run() {
    let salt = SaltString::from_b64("c29tZXNhbHRzb21lc2FsdA").unwrap();
    let costly = Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        Params::new(MAX_VERIFIER_MEMORY_KIB, MAX_VERIFIER_PASSES + 1, 1, None).unwrap(),
    )
    .hash_password(b"handed over", &salt)
    .unwrap()
    .to_string();
    assert!(SessionAuthentication::from_verifier(costly.clone()).is_err());

    let adopted = SessionAuthentication::adopt_verifier(costly.clone())
        .expect("an over-cost verifier must not fail the handover it came in");
    let probe = verification_probe::watch(&costly, Duration::ZERO);
    assert!(adopted.verify("handed over").is_none());
    assert_eq!(probe.runs(), 0, "Argon2 must not run for it at all");

    assert!(SessionAuthentication::adopt_verifier("not a verifier".to_owned()).is_err());
}
