//! Resolving the age recipients a stored session is encrypted to.
//!
//! A configured recipient is either an age key, an SSH key, or a GitHub
//! account whose keys are fetched once at startup. The fetch is deliberately
//! not part of parsing: a temporary failure must not look like a permanently
//! invalid configuration, which is what `RecipientResolutionError` separates.

use super::*;

/// A parsed collection of age recipients.
pub struct RecipientSet {
    recipients: Vec<Box<dyn age::Recipient + Send + Sync>>,
    post_quantum: bool,
}

impl fmt::Debug for RecipientSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RecipientSet")
            .field("count", &self.recipients.len())
            .field("post_quantum", &self.post_quantum)
            .finish()
    }
}

impl RecipientSet {
    /// Parses configured recipients without doing network I/O.
    pub fn parse(values: &[String]) -> Result<Self> {
        let mut recipients = Vec::new();
        let mut post_quantum = None;
        for value in values {
            let value = value.trim();
            anyhow::ensure!(!value.is_empty(), "age recipient entries must not be empty");
            anyhow::ensure!(
                !value.starts_with("github:"),
                "github recipients must be resolved before parsing the daemon options"
            );
            let is_pq = value.starts_with("age1pq1");
            match post_quantum {
                Some(previous) if previous != is_pq => anyhow::bail!(
                    "post-quantum age recipients cannot be mixed with classical or SSH recipients"
                ),
                None => post_quantum = Some(is_pq),
                _ => {}
            }
            let recipient = parse_recipient(value)?;
            recipients.push(recipient);
        }
        Ok(Self {
            recipients,
            post_quantum: post_quantum.unwrap_or(false),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.recipients.is_empty()
    }

    pub fn is_post_quantum(&self) -> bool {
        self.post_quantum
    }

    pub(crate) fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        anyhow::ensure!(
            !self.recipients.is_empty(),
            "no persistence recipients configured"
        );
        // age v1's Encryptor provides both the recipient envelope and the
        // standard ChaCha20-Poly1305 STREAM payload construction.
        let encryptor = age::Encryptor::with_recipients(
            self.recipients
                .iter()
                .map(|recipient| recipient.as_ref() as &dyn age::Recipient),
        )
        .context("creating age encryptor")?;
        let mut ciphertext = Vec::with_capacity(plaintext.len() + 4096);
        let mut writer = encryptor
            .wrap_output(&mut ciphertext)
            .context("creating age output stream")?;
        writer
            .write_all(plaintext)
            .context("writing age plaintext")?;
        writer.finish().context("finishing age output stream")?;
        Ok(ciphertext)
    }
}

/// Parses one configured recipient. GitHub entries intentionally do not reach
/// this function: resolving them is an explicit startup operation.
pub fn parse_recipient(value: &str) -> Result<Box<dyn age::Recipient + Send + Sync>> {
    if value.starts_with("age1pq1") {
        return Ok(Box::new(MlKem768X25519Recipient::from_str(value)?));
    }
    if value.starts_with("age1") {
        return Ok(Box::new(
            age::x25519::Recipient::from_str(value)
                .map_err(|_| anyhow::anyhow!("invalid age X25519 recipient"))?,
        ));
    }
    if value.starts_with("ssh-") {
        return Ok(Box::new(age::ssh::Recipient::from_str(value).map_err(
            |error| anyhow::anyhow!("invalid SSH recipient: {error:?}"),
        )?));
    }
    anyhow::bail!("invalid age recipient")
}

/// Resolves `github:USER` entries and then applies the same parser and PQ
/// mixing rule as direct recipients. Configuration parsing never calls this.
pub fn resolve_recipients(values: &[String]) -> Result<RecipientSet> {
    let resolved = resolve_recipient_strings_for_startup(values)
        .map_err(RecipientResolutionError::into_anyhow)?;
    RecipientSet::parse(&resolved)
}

/// Resolves aliases and validates the complete recipient set, returning only
/// the public strings the daemon needs to encrypt future records.
pub fn resolve_recipient_strings(values: &[String]) -> Result<Vec<String>> {
    resolve_recipient_strings_for_startup(values).map_err(RecipientResolutionError::into_anyhow)
}

/// Resolves aliases for an application startup path while preserving whether
/// a failure is temporary network trouble or invalid configuration.
///
/// The direct recipient entries and all GitHub usernames are validated before
/// the first network request.  That ordering matters: a bad recipient must not
/// be mistaken for a temporary GitHub outage merely because it appeared after a
/// `github:` entry in the configuration.
pub fn resolve_recipient_strings_for_startup(
    values: &[String],
) -> std::result::Result<Vec<String>, RecipientResolutionError> {
    resolve_recipient_strings_with(values, fetch_github_recipients)
}

fn resolve_recipient_strings_with(
    values: &[String],
    mut fetch_github: impl FnMut(&str) -> std::result::Result<Vec<String>, RecipientResolutionError>,
) -> std::result::Result<Vec<String>, RecipientResolutionError> {
    let mut resolved = Vec::new();
    let mut usernames = Vec::new();
    for value in values {
        let value = value.trim();
        if let Some(username) = value.strip_prefix("github:") {
            validate_github_username(username).map_err(permanent_recipient_error)?;
            usernames.push(username.to_owned());
        } else {
            if value.is_empty() {
                return Err(permanent_recipient_error(anyhow::anyhow!(
                    "age recipient entries must not be empty"
                )));
            }
            parse_recipient(value).map_err(permanent_recipient_error)?;
            resolved.push(value.to_owned());
        }
    }

    for username in usernames {
        resolved.extend(fetch_github(&username)?);
    }
    RecipientSet::parse(&resolved).map_err(permanent_recipient_error)?;
    Ok(resolved)
}

fn fetch_github_recipients(
    username: &str,
) -> std::result::Result<Vec<String>, RecipientResolutionError> {
    validate_github_username(username).map_err(permanent_recipient_error)?;
    let url = format!("https://github.com/{username}.keys");
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(GITHUB_TIMEOUT)
        .timeout(GITHUB_TIMEOUT)
        .user_agent("zetta-zmux")
        .build()
        .map_err(|error| permanent_recipient_error(anyhow::Error::new(error)))?;
    let response = client.get(url).send().map_err(|error| {
        let error = anyhow::Error::new(error).context("fetching GitHub SSH keys");
        if error.chain().any(|cause| {
            cause
                .downcast_ref::<reqwest::Error>()
                .is_some_and(|error| error.is_connect() || error.is_timeout())
        }) {
            temporary_recipient_error(error)
        } else {
            permanent_recipient_error(error)
        }
    })?;
    let status = response.status();
    if !status.is_success() {
        let error = anyhow::anyhow!("GitHub did not return SSH keys (HTTP {status})");
        return if is_retryable_github_status(status) {
            Err(temporary_recipient_error(error))
        } else {
            Err(permanent_recipient_error(error))
        };
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_GITHUB_RESPONSE_BYTES)
    {
        return Err(permanent_recipient_error(anyhow::anyhow!(
            "GitHub SSH key response is too large"
        )));
    }
    let mut body = Vec::new();
    response
        .take(MAX_GITHUB_RESPONSE_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|error| {
            temporary_recipient_error(anyhow::Error::new(error).context("reading GitHub SSH keys"))
        })?;
    if body.len() as u64 > MAX_GITHUB_RESPONSE_BYTES {
        return Err(permanent_recipient_error(anyhow::anyhow!(
            "GitHub SSH key response is too large"
        )));
    }
    parse_github_keys(&body).map_err(permanent_recipient_error)
}

fn is_retryable_github_status(status: reqwest::StatusCode) -> bool {
    matches!(
        status,
        reqwest::StatusCode::REQUEST_TIMEOUT
            | reqwest::StatusCode::TOO_EARLY
            | reqwest::StatusCode::TOO_MANY_REQUESTS
    ) || status.is_server_error()
}

fn validate_github_username(username: &str) -> Result<()> {
    anyhow::ensure!(
        (1..=39).contains(&username.len())
            && username
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && !username.starts_with('-')
            && !username.ends_with('-')
            && !username.contains("--"),
        "invalid GitHub username"
    );
    Ok(())
}

fn parse_github_keys(body: &[u8]) -> Result<Vec<String>> {
    let text = std::str::from_utf8(body).context("GitHub SSH key response was not UTF-8")?;
    let mut keys = Vec::new();
    for line in text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let mut fields = line.split_whitespace();
        let key_type = fields.next().context("malformed GitHub SSH key")?;
        let encoded = fields.next().context("malformed GitHub SSH key")?;
        let valid_blob = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .is_ok();
        if !valid_blob {
            anyhow::bail!("malformed GitHub SSH key");
        }
        let canonical = format!("{key_type} {encoded}");
        match key_type {
            "ssh-ed25519" | "ssh-rsa" => {
                parse_recipient(&canonical)?;
                keys.push(canonical);
            }
            _ => match age::ssh::Recipient::from_str(&canonical) {
                Ok(_) => unreachable!("age returned a supported SSH key type as unsupported"),
                Err(
                    age::ssh::ParseRecipientKeyError::Unsupported(_)
                    | age::ssh::ParseRecipientKeyError::Ignore,
                ) => {
                    // Valid but unsupported SSH key types are ignored, matching
                    // age's recipient-file behavior.
                }
                Err(error) => anyhow::bail!("malformed GitHub SSH key: {error:?}"),
            },
        }
    }
    anyhow::ensure!(!keys.is_empty(), "GitHub account has no supported SSH keys");
    Ok(keys)
}

#[cfg(test)]
#[path = "../tests/persistence/recipients.rs"]
mod tests;
