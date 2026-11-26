//! Session authentication: the Argon2id verifier a protected session is
//! reattached with, the backoff that bounds guessing at it, and the buffers
//! secrets are built in.
//!
//! This is deliberately free of any terminal, GPUI or platform dependency so
//! the daemon and the client can share one implementation. The verifier half
//! is `verifier.rs`, which a Zetta build without the multiplexer compiles in by
//! path, as it does [`crate::private_fs`].
//!
//! `Zeroizing` erases the allocation a value ends up in, but a `Vec` or
//! `String` that grows leaves every earlier, smaller copy to the allocator
//! unerased, and a `FromUtf8Error` frees the bytes it refused the same way.
//! [`SecretBytes`], [`read_secret_to_end`], [`encode_secret_json`],
//! [`secret_utf8`] and [`push_secret_char`] close those paths for the buffers
//! this crate owns. Copies a dependency makes inside its own API — `age`'s
//! plaintext chunk on the encrypting side, `serde_json`'s scratch buffer for an
//! escaped string, the standard library's stdin buffer, a parsed key's stack
//! copy — are outside their reach, so nothing here promises that a secret
//! leaves no trace in process memory: only that the copies this crate makes are
//! erased.

use std::io;

use anyhow::Result;
use zeroize::Zeroizing;

mod verifier;
pub use verifier::*;

/// A byte buffer for secret material that erases every allocation it gives up.
///
/// `Zeroizing<Vec<u8>>` erases the allocation a vector ends up in, not the ones
/// it abandoned on the way there: growing reallocates, and the allocator gets
/// the old block back with its contents intact. This grows by copying into a
/// fresh zeroizing allocation instead, so nothing it held is freed unerased.
/// Presizing it still matters: growth copies, and erasing is the cost of that.
#[derive(Default)]
pub struct SecretBytes(Zeroizing<Vec<u8>>);

impl SecretBytes {
    pub fn with_capacity(capacity: usize) -> Self {
        Self(Zeroizing::new(Vec::with_capacity(capacity)))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    pub fn extend_from_slice(&mut self, bytes: &[u8]) {
        let needed = self
            .0
            .len()
            .checked_add(bytes.len())
            .expect("secret buffer length overflowed");
        if needed > self.0.capacity() {
            let capacity = needed.max(self.0.capacity().saturating_mul(2)).max(64);
            let mut grown = Zeroizing::new(Vec::with_capacity(capacity));
            grown.extend_from_slice(&self.0);
            // The old allocation is erased as it is dropped here.
            self.0 = grown;
        }
        self.0.extend_from_slice(bytes);
    }

    pub fn into_inner(self) -> Zeroizing<Vec<u8>> {
        self.0
    }
}

impl io::Write for SecretBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Reads `reader` to its end into zeroizing memory, refusing more than `limit`
/// bytes.
///
/// `size_hint` is the expected length — a file's size — and makes the usual
/// case a single allocation. The bytes pass through one stack buffer, which is
/// erased before this returns.
pub fn read_secret_to_end(
    reader: &mut impl io::Read,
    size_hint: usize,
    limit: usize,
) -> io::Result<Zeroizing<Vec<u8>>> {
    let mut buffer = SecretBytes::with_capacity(size_hint.min(limit));
    let mut chunk = Zeroizing::new([0_u8; 8192]);
    loop {
        let read = match reader.read(&mut chunk[..]) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if buffer.len().saturating_add(read) > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("secret input is longer than {limit} bytes"),
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    Ok(buffer.into_inner())
}

/// Serializes `value` as JSON into zeroizing memory, after `prefix` zero bytes
/// the caller fills in — a frame header, say.
///
/// The output is measured first and written into a buffer of exactly that
/// size, so serializing never reallocates: `serde_json::to_vec` grows its
/// vector as it goes and frees each smaller copy unerased. Costs a second
/// serialization, which is why this is for values carrying a secret.
pub fn encode_secret_json(
    value: &impl serde::Serialize,
    prefix: usize,
) -> serde_json::Result<Zeroizing<Vec<u8>>> {
    struct Measure(usize);

    impl io::Write for Measure {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let mut measure = Measure(0);
    serde_json::to_writer(&mut measure, value)?;
    let mut buffer = SecretBytes::with_capacity(prefix.saturating_add(measure.0));
    buffer.0.resize(prefix, 0);
    serde_json::to_writer(&mut buffer, value)?;
    Ok(buffer.into_inner())
}

/// Turns decrypted bytes into text without leaving them behind on failure.
///
/// `String::from_utf8`'s error owns the bytes it refused, and dropping it frees
/// them unerased; this takes them back out and erases them.
pub fn secret_utf8(mut bytes: Zeroizing<Vec<u8>>) -> Result<Zeroizing<String>> {
    match String::from_utf8(std::mem::take(&mut *bytes)) {
        Ok(text) => Ok(Zeroizing::new(text)),
        Err(error) => {
            let position = error.utf8_error().valid_up_to();
            drop(Zeroizing::new(error.into_bytes()));
            anyhow::bail!("secret text is not valid UTF-8 (at byte {position})")
        }
    }
}

/// Appends `character` to a secret being typed, growing as [`SecretBytes`]
/// does so no shorter copy is freed unerased.
pub fn push_secret_char(secret: &mut Zeroizing<String>, character: char) {
    let needed = secret.len().saturating_add(character.len_utf8());
    if needed > secret.capacity() {
        let capacity = needed.max(secret.capacity().saturating_mul(2)).max(64);
        let mut grown = Zeroizing::new(String::with_capacity(capacity));
        grown.push_str(secret);
        // The old allocation is erased as it is dropped here.
        *secret = grown;
    }
    secret.push(character);
}

#[cfg(test)]
#[path = "tests/auth.rs"]
mod tests;
