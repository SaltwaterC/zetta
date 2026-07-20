use super::*;

#[test]
fn empty_detach_authentication_selects_the_unprotected_path() {
    assert_eq!(
        detach_authentication_choice("", ""),
        DetachAuthenticationChoice::Unprotected
    );
}

#[test]
fn matching_non_empty_detach_authentication_selects_the_protected_path() {
    assert_eq!(
        detach_authentication_choice("secret", "secret"),
        DetachAuthenticationChoice::Protected
    );
}

#[test]
fn partial_or_mismatched_detach_authentication_is_incomplete() {
    for (secret, confirmation) in [("secret", ""), ("", "secret"), ("one", "two")] {
        assert_eq!(
            detach_authentication_choice(secret, confirmation),
            DetachAuthenticationChoice::Incomplete
        );
    }
}
