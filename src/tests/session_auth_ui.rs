use super::*;

/// The dialog's choices, which every action that widens a session's reach shares.
///
/// Detaching has always taken an empty pair to mean "leave it unprotected", and
/// keeping a tab running the same. Sharing follows the same rule rather than
/// refusing the empty pair: a third dialog that behaved differently would be the
/// odd one out, whichever way it differed.
#[test]
fn an_empty_session_authentication_selects_the_unprotected_path() {
    assert_eq!(
        session_authentication_choice("", ""),
        SessionAuthenticationChoice::Unprotected
    );
}

#[test]
fn matching_non_empty_session_authentication_selects_the_protected_path() {
    assert_eq!(
        session_authentication_choice("secret", "secret"),
        SessionAuthenticationChoice::Protected
    );
}

#[test]
fn partial_or_mismatched_session_authentication_is_incomplete() {
    for (secret, confirmation) in [("secret", ""), ("", "secret"), ("one", "two")] {
        assert_eq!(
            session_authentication_choice(secret, confirmation),
            SessionAuthenticationChoice::Incomplete
        );
    }
}
