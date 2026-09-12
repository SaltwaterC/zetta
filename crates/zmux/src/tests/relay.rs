use super::*;

/// The secret is read before anything else, from a terminal already in raw
/// mode, so the relay has to find the end of the line itself rather than
/// relying on a line discipline that is no longer doing it.
#[test]
fn a_secret_line_ends_at_the_newline_and_keeps_what_follows_for_the_pane() {
    let (secret, rest) = split_secret_line(b"passphrase\r\nls -l\r").expect("a complete line");
    assert_eq!(secret, "passphrase");
    assert_eq!(
        rest, b"ls -l\r",
        "input typed after the secret belongs to the pane"
    );
}

/// An empty secret is still a secret the daemon can refuse; it is not the
/// absence of one, which is what would make the relay attach unauthenticated.
#[test]
fn an_empty_secret_line_is_a_secret() {
    let (secret, rest) = split_secret_line(b"\n").expect("a complete line");
    assert!(secret.is_empty());
    assert!(rest.is_empty());
}

/// Without a newline there is no secret yet, and the relay must keep waiting
/// rather than attaching with a partial one.
#[test]
fn an_unterminated_secret_line_is_not_yet_a_secret() {
    assert!(split_secret_line(b"passphras").is_none());
}

/// A peer that never sends a newline must not be able to make the relay
/// allocate without bound.
#[test]
fn a_secret_longer_than_the_bound_is_refused() {
    let overlong = vec![b'x'; MAX_SECRET_BYTES + 1];
    assert!(
        read_secret_from(&mut overlong.as_slice()).is_err(),
        "an unbounded secret must be refused rather than buffered"
    );
    let mut bounded = vec![b'x'; MAX_SECRET_BYTES];
    bounded.push(b'\n');
    assert_eq!(
        read_secret_from(&mut bounded.as_slice())
            .expect("a secret at the bound is accepted")
            .expose()
            .len(),
        MAX_SECRET_BYTES
    );
}

/// The relay's own parsing, exercised without a terminal: this is
/// [`read_secret_from`] against a fixed input, which is what the real read
/// does once its bytes have arrived.
fn split_secret_line(input: &[u8]) -> Option<(String, Vec<u8>)> {
    let mut remaining = input;
    let secret = read_secret_from(&mut remaining).ok()?;
    Some((secret.expose().to_owned(), remaining.to_vec()))
}
