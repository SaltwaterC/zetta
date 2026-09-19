use super::*;

#[cfg(unix)]
#[test]
fn a_relay_publishes_its_agent_at_the_daemons_stable_path() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("private-agent.sock");
    let stable = directory.path().join("forwarded-agent.sock");
    let fallback = directory.path().join("ssh-agent.sock");

    let link =
        ForwardedAgentLink::publish(target.clone(), stable.clone(), fallback.clone()).unwrap();
    assert_eq!(fs::read_link(&stable).unwrap(), target);

    drop(link);
    assert_eq!(fs::read_link(&stable).unwrap(), fallback);
}

#[cfg(unix)]
#[test]
fn an_older_relay_does_not_remove_a_newer_agent_link() {
    let directory = tempfile::tempdir().unwrap();
    let first_target = directory.path().join("first-agent.sock");
    let second_target = directory.path().join("second-agent.sock");
    let stable = directory.path().join("forwarded-agent.sock");
    let fallback = directory.path().join("ssh-agent.sock");

    let first =
        ForwardedAgentLink::publish(first_target, stable.clone(), fallback.clone()).unwrap();
    let second =
        ForwardedAgentLink::publish(second_target.clone(), stable.clone(), fallback.clone())
            .unwrap();
    drop(first);
    assert_eq!(fs::read_link(&stable).unwrap(), second_target);

    drop(second);
    assert_eq!(fs::read_link(&stable).unwrap(), fallback);
}

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

/// The prelude is positional, because neither line is self-describing: the
/// secret first, then the viewer's client ID, then the pane's own input. A
/// relay that read them the other way round would attach with the secret as an
/// identity and the identity as a secret.
#[test]
fn the_viewer_follows_the_secret_and_the_pane_keeps_what_follows_both() {
    let mut remaining: &[u8] = b"passphrase\r\n9f0a1b2c\r\nls -l\r";
    let secret = read_secret_from(&mut remaining).expect("a complete secret line");
    let viewer = read_viewer_from(&mut remaining).expect("a complete viewer line");

    assert_eq!(secret.expose(), "passphrase");
    assert_eq!(viewer.as_str(), "9f0a1b2c");
    assert_eq!(
        remaining, b"ls -l\r",
        "input typed after the prelude belongs to the pane"
    );
}

/// An empty line is not an identity. Accepting it would put a client ID that
/// cannot exist into the pane's shared set, where it would be indistinguishable
/// from a relay that had declared a real one.
#[test]
fn an_empty_viewer_line_is_refused() {
    assert!(read_viewer_from(&mut b"\n".as_slice()).is_err());
    assert!(read_viewer_from(&mut b"   \n".as_slice()).is_err());
}

/// The relay's own parsing, exercised without a terminal: this is
/// [`read_secret_from`] against a fixed input, which is what the real read
/// does once its bytes have arrived.
fn split_secret_line(input: &[u8]) -> Option<(String, Vec<u8>)> {
    let mut remaining = input;
    let secret = read_secret_from(&mut remaining).ok()?;
    Some((secret.expose().to_owned(), remaining.to_vec()))
}
