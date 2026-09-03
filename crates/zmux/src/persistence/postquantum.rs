//! The ML-KEM-768/X25519 age recipient and identity, and the Bech32 coding
//! their string forms use.
//!
//! age 1.3.1 defines this stanza; it is implemented here rather than taken
//! from the `age` crate because that crate does not expose it.

use super::*;

/// The ML-KEM-768/X25519 age recipient used by age 1.3.1.
#[derive(Clone, PartialEq, Eq)]
pub struct MlKem768X25519Recipient {
    key: <XWing as hpke::kem::Kem>::PublicKey,
}

impl fmt::Debug for MlKem768X25519Recipient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_public(formatter)
    }
}

impl fmt::Display for MlKem768X25519Recipient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_public(formatter)
    }
}

impl MlKem768X25519Recipient {
    fn fmt_public(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}",
            bech32_encode(PQ_RECIPIENT_HRP, &self.key.to_bytes(), false)
        )
    }
}

impl FromStr for MlKem768X25519Recipient {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let bytes = bech32_decode(value, PQ_RECIPIENT_HRP, false)?;
        anyhow::ensure!(
            bytes.len() == PQ_PUBLIC_KEY_BYTES,
            "invalid ML-KEM-768/X25519 recipient length"
        );
        let key = <XWing as hpke::kem::Kem>::PublicKey::from_bytes(&bytes)
            .map_err(|_| anyhow::anyhow!("invalid ML-KEM-768/X25519 recipient key"))?;
        Ok(Self { key })
    }
}

impl age::Recipient for MlKem768X25519Recipient {
    fn wrap_file_key(
        &self,
        file_key: &FileKey,
    ) -> std::result::Result<(Vec<Stanza>, HashSet<String>), age::EncryptError> {
        let (enc, mut context) = hpke::setup_sender::<ChaCha20Poly1305, HkdfSha256, XWing>(
            &OpModeS::Base,
            &self.key,
            PQ_LABEL,
        )
        .map_err(|error| age::EncryptError::Io(io::Error::other(error.to_string())))?;
        let body = context
            .seal(file_key.expose_secret(), &[])
            .map_err(|error| age::EncryptError::Io(io::Error::other(error.to_string())))?;
        Ok((
            vec![Stanza {
                tag: PQ_STANZA.to_owned(),
                args: vec![STANDARD_NO_PAD.encode(enc.to_bytes())],
                body,
            }],
            [PQ_LABEL_NAME.to_owned()].into_iter().collect(),
        ))
    }
}

/// The ML-KEM-768/X25519 age identity used by age 1.3.1.
#[derive(Clone)]
pub struct MlKem768X25519Identity {
    key: <XWing as hpke::kem::Kem>::PrivateKey,
}

impl fmt::Debug for MlKem768X25519Identity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MlKem768X25519Identity(REDACTED)")
    }
}

impl MlKem768X25519Identity {
    pub fn generate() -> Self {
        let (key, _) = XWing::gen_keypair();
        Self { key }
    }

    pub fn to_recipient(&self) -> MlKem768X25519Recipient {
        MlKem768X25519Recipient {
            key: XWing::sk_to_pk(&self.key),
        }
    }

    fn encoded(&self) -> String {
        bech32_encode(PQ_IDENTITY_HRP, &self.key.to_bytes(), true)
    }
}

impl fmt::Display for MlKem768X25519Identity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.encoded())
    }
}

impl FromStr for MlKem768X25519Identity {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let bytes = bech32_decode(value, PQ_IDENTITY_HRP, true)?;
        anyhow::ensure!(
            bytes.len() == PQ_PRIVATE_KEY_BYTES,
            "invalid ML-KEM-768/X25519 identity length"
        );
        let key = <XWing as hpke::kem::Kem>::PrivateKey::from_bytes(&bytes)
            .map_err(|_| anyhow::anyhow!("invalid ML-KEM-768/X25519 identity key"))?;
        Ok(Self { key })
    }
}

impl age::Identity for MlKem768X25519Identity {
    fn unwrap_stanza(
        &self,
        stanza: &Stanza,
    ) -> Option<std::result::Result<FileKey, age::DecryptError>> {
        if stanza.tag != PQ_STANZA {
            return None;
        }
        let encoded = match stanza.args.as_slice() {
            [encoded] => encoded,
            _ => return Some(Err(age::DecryptError::InvalidHeader)),
        };
        let encoded = match STANDARD_NO_PAD.decode(encoded) {
            Ok(encoded) if encoded.len() == PQ_ENCAPSULATED_KEY_BYTES => encoded,
            _ => return Some(Err(age::DecryptError::InvalidHeader)),
        };
        if stanza.body.len() != PQ_CIPHERTEXT_BYTES {
            return Some(Err(age::DecryptError::InvalidHeader));
        }
        let enc = match <XWing as hpke::kem::Kem>::EncappedKey::from_bytes(&encoded) {
            Ok(enc) => enc,
            Err(_) => return Some(Err(age::DecryptError::InvalidHeader)),
        };
        let mut context = match hpke::setup_receiver::<ChaCha20Poly1305, HkdfSha256, XWing>(
            &OpModeR::Base,
            &self.key,
            &enc,
            PQ_LABEL,
        ) {
            Ok(context) => context,
            Err(_) => return Some(Err(age::DecryptError::InvalidHeader)),
        };
        match context.open(&stanza.body, &[]) {
            Ok(plaintext) if plaintext.len() == FILE_KEY_BYTES => {
                let mut plaintext = plaintext;
                Some(Ok(FileKey::init_with_mut(|file_key| {
                    file_key.copy_from_slice(&plaintext);
                    plaintext.fill(0);
                })))
            }
            // X-Wing is anonymous. A failed open means this stanza was not
            // addressed to this identity, so age should try the next one.
            _ => None,
        }
    }
}

fn bech32_encode(hrp: &str, bytes: &[u8], uppercase: bool) -> String {
    const CHARSET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";
    let data = convert_bits(bytes, 8, 5, true).expect("valid 8-to-5 conversion");
    let mut values = data.clone();
    values.extend([0; 6]);
    let checksum_hrp = hrp.to_ascii_lowercase();
    let checksum = bech32_polymod(&bech32_hrp_expand(&checksum_hrp), &values) ^ 1;
    let mut output = String::with_capacity(hrp.len() + 1 + data.len() + 6);
    output.push_str(hrp);
    output.push('1');
    for value in data
        .into_iter()
        .chain((0..6).map(|index| ((checksum >> (5 * (5 - index))) & 31) as u8))
    {
        output.push(CHARSET[value as usize] as char);
    }
    if uppercase {
        output.to_ascii_uppercase()
    } else {
        output
    }
}

fn bech32_decode(value: &str, expected_hrp: &str, uppercase: bool) -> Result<Vec<u8>> {
    const CHARSET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";
    const MAX_BECH32_LENGTH: usize = 4096;
    anyhow::ensure!(!value.is_empty(), "invalid Bech32 encoding");
    anyhow::ensure!(
        value.len() <= MAX_BECH32_LENGTH,
        "Bech32 encoding is too long"
    );
    anyhow::ensure!(value.is_ascii(), "invalid Bech32 encoding");
    anyhow::ensure!(
        !(value
            .chars()
            .any(|character| character.is_ascii_lowercase())
            && value
                .chars()
                .any(|character| character.is_ascii_uppercase())),
        "mixed-case Bech32 encoding"
    );
    anyhow::ensure!(
        value
            .chars()
            .all(|character| !character.is_ascii() || character.is_ascii_graphic()),
        "invalid Bech32 encoding"
    );
    anyhow::ensure!(
        (uppercase && value == value.to_ascii_uppercase())
            || (!uppercase && value == value.to_ascii_lowercase()),
        "non-canonical Bech32 case"
    );
    let separator = value.rfind('1').context("invalid Bech32 encoding")?;
    let (hrp, encoded) = value.split_at(separator);
    anyhow::ensure!(hrp == expected_hrp, "incorrect Bech32 HRP");
    let encoded = &encoded[1..];
    anyhow::ensure!(encoded.len() >= 6, "invalid Bech32 checksum");
    let values = encoded
        .bytes()
        .map(|byte| {
            CHARSET
                .iter()
                .position(|candidate| *candidate == byte.to_ascii_lowercase())
                .map(|value| value as u8)
                .context("invalid Bech32 character")
        })
        .collect::<Result<Vec<_>>>()?;
    let checksum_hrp = hrp.to_ascii_lowercase();
    anyhow::ensure!(
        bech32_polymod(&bech32_hrp_expand(&checksum_hrp), &values) == 1,
        "invalid Bech32 checksum"
    );
    convert_bits(&values[..values.len() - 6], 5, 8, false)
}

fn convert_bits(data: &[u8], from: u32, to: u32, pad: bool) -> Result<Vec<u8>> {
    let mut accumulator = 0u32;
    let mut bits = 0u32;
    let max_value = (1u32 << to) - 1;
    let max_input = (1u32 << from) - 1;
    let max_accumulator = (1u32 << (from + to - 1)) - 1;
    let mut result = Vec::new();
    for &value in data {
        anyhow::ensure!(u32::from(value) <= max_input, "invalid Bech32 data");
        accumulator = ((accumulator << from) | u32::from(value)) & max_accumulator;
        bits += from;
        while bits >= to {
            bits -= to;
            result.push(((accumulator >> bits) & max_value) as u8);
        }
    }
    if pad {
        if bits > 0 {
            result.push(((accumulator << (to - bits)) & max_value) as u8);
        }
    } else {
        anyhow::ensure!(bits < from && ((accumulator << (to - bits)) & max_value) == 0);
    }
    Ok(result)
}

fn bech32_hrp_expand(hrp: &str) -> Vec<u8> {
    hrp.bytes()
        .map(|byte| byte >> 5)
        .chain(std::iter::once(0))
        .chain(hrp.bytes().map(|byte| byte & 31))
        .collect()
}

fn bech32_polymod(hrp: &[u8], values: &[u8]) -> u32 {
    const GENERATORS: [u32; 5] = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3];
    hrp.iter().chain(values).fold(1u32, |checksum, value| {
        let top = checksum >> 25;
        let mut checksum = (checksum & 0x1ffffff) << 5 ^ u32::from(*value);
        for (index, generator) in GENERATORS.iter().enumerate() {
            if (top >> index) & 1 == 1 {
                checksum ^= generator;
            }
        }
        checksum
    })
}

#[cfg(test)]
#[path = "../tests/persistence/postquantum.rs"]
mod tests;
