use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context as _, Result, bail};
use async_compression::futures::bufread::GzipDecoder;
use async_tar::Archive;
use futures::{AsyncReadExt as _, io::BufReader};
use gpui::BackgroundExecutor;
use gpui::http_client::{AsyncBody, HttpClient, Url};
use serde::Deserialize;

const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
/// Ceiling on what one extension archive may expand to. `MAX_RESPONSE_BYTES`
/// bounds the compressed download only; gzip can expand far past it, so without
/// this a hostile or compromised extension host could fill the disk through the
/// staging directory.
const MAX_UNPACKED_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize)]
pub struct ThemeExtension {
    pub id: Arc<str>,
    pub name: String,
    pub version: Arc<str>,
    pub description: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    pub download_count: u64,
}
#[derive(Clone, Debug)]
pub struct InstalledThemeExtension {
    pub id: String,
    pub theme_names: Vec<String>,
    pub file_count: usize,
}

#[derive(Deserialize)]
struct ExtensionResponse {
    data: Vec<ThemeExtension>,
}

#[derive(Deserialize)]
struct Manifest {
    #[serde(default)]
    themes: Vec<PathBuf>,
}

#[derive(Deserialize)]
struct OldManifest {
    #[serde(default)]
    themes: BTreeMap<String, PathBuf>,
}

pub async fn fetch(http: Arc<dyn HttpClient>, query: &str) -> Result<Vec<ThemeExtension>> {
    let mut parameters = vec![("max_schema_version", "1"), ("provides", "themes")];
    if !query.trim().is_empty() {
        parameters.push(("filter", query.trim()));
    }
    let url = Url::parse_with_params("https://api.zed.dev/extensions", &parameters)?;
    let bytes = get(http, url.as_ref()).await?;
    let mut extensions: ExtensionResponse =
        serde_json::from_slice(&bytes).context("parsing Zed extension response")?;
    extensions
        .data
        .sort_by_key(|extension| extension.name.to_lowercase());
    Ok(extensions.data)
}

pub async fn download(
    http: Arc<dyn HttpClient>,
    extension: &ThemeExtension,
    themes_dir: &Path,
    executor: BackgroundExecutor,
) -> Result<usize> {
    // The id and version come from the extension index, not from us. Reject
    // anything that could add or escape a path segment rather than trusting the
    // index to be well behaved.
    let id = url_path_segment(&extension.id).context("extension has an unusable id")?;
    let version =
        url_path_segment(&extension.version).context("extension has an unusable version")?;
    let url = Url::parse(&format!(
        "https://api.zed.dev/extensions/{id}/{version}/download"
    ))?;
    let archive = get(http, url.as_ref()).await?;
    let extension_id = extension.id.to_string();
    let themes_dir = themes_dir.to_owned();
    executor
        .spawn(async move { install_archive(&archive, &extension_id, &themes_dir).await })
        .await
}

async fn get(http: Arc<dyn HttpClient>, url: &str) -> Result<Vec<u8>> {
    let mut response = http
        .get(url, AsyncBody::empty(), true)
        .await
        .with_context(|| format!("requesting {url}"))?;
    if !response.status().is_success() {
        bail!("Zed extensions site returned {}", response.status());
    }
    if response
        .headers()
        .get(gpui::http_client::http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok()?.parse::<usize>().ok())
        .is_some_and(|length| length > MAX_RESPONSE_BYTES)
    {
        bail!("extension download is larger than 64 MiB");
    }
    let mut bytes = Vec::new();
    response
        .body_mut()
        .take((MAX_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        bail!("extension download is larger than 64 MiB");
    }
    Ok(bytes)
}

/// Fails the read once the wrapped stream has produced more than `limit` bytes.
///
/// Applied to the decompressed side of the gzip stream, so the bound is on what
/// actually reaches the disk rather than on the compressed download.
struct LimitedReader<R> {
    inner: R,
    produced: u64,
    limit: u64,
}

impl<R> LimitedReader<R> {
    fn new(inner: R, limit: u64) -> Self {
        Self {
            inner,
            produced: 0,
            limit,
        }
    }
}

impl<R: futures::AsyncRead + Unpin> futures::AsyncRead for LimitedReader<R> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let this = &mut *self;
        let polled = std::pin::Pin::new(&mut this.inner).poll_read(cx, buf);
        if let std::task::Poll::Ready(Ok(count)) = &polled {
            this.produced += *count as u64;
            if this.produced > this.limit {
                return std::task::Poll::Ready(Err(std::io::Error::other(format!(
                    "extension archive expands past {} bytes",
                    this.limit
                ))));
            }
        }
        polled
    }
}

async fn install_archive(archive: &[u8], extension_id: &str, themes_dir: &Path) -> Result<usize> {
    let unpacked = tempfile::tempdir().context("creating extension staging directory")?;
    let decompressed = LimitedReader::new(
        GzipDecoder::new(BufReader::new(archive)),
        MAX_UNPACKED_BYTES,
    );
    Archive::new(decompressed)
        .unpack(unpacked.path())
        .await
        .context("unpacking extension")?;

    let theme_paths = manifest_theme_paths(unpacked.path())?;
    if theme_paths.is_empty() {
        bail!("extension does not declare any themes");
    }
    fs::create_dir_all(themes_dir).with_context(|| format!("creating {}", themes_dir.display()))?;
    let root = unpacked.path().canonicalize()?;
    let safe_id = safe_file_component(extension_id);
    for (index, relative) in theme_paths.iter().enumerate() {
        validate_relative_theme_path(relative)?;
        let source = unpacked
            .path()
            .join(relative)
            .canonicalize()
            .with_context(|| format!("locating theme {}", relative.display()))?;
        if !source.starts_with(&root) || !source.is_file() {
            bail!("theme path escapes the extension archive");
        }
        let bytes =
            fs::read(&source).with_context(|| format!("reading theme {}", relative.display()))?;
        serde_json::from_slice::<serde_json::Value>(&bytes)
            .with_context(|| format!("theme {} is not valid JSON", relative.display()))?;
        let file_name = relative
            .file_stem()
            .and_then(|name| name.to_str())
            .map_or_else(|| "theme".to_owned(), safe_file_component);
        let destination = themes_dir.join(format!("{safe_id}--{index}--{file_name}.json"));
        let temporary = themes_dir.join(format!(".{safe_id}--{index}.tmp"));
        fs::write(&temporary, bytes).with_context(|| format!("writing {}", temporary.display()))?;
        if destination.is_file() {
            fs::remove_file(&destination)
                .with_context(|| format!("replacing {}", destination.display()))?;
        }
        fs::rename(&temporary, &destination)
            .with_context(|| format!("installing {}", destination.display()))?;
    }
    Ok(theme_paths.len())
}

pub fn installed(themes_dir: &Path) -> Result<Vec<InstalledThemeExtension>> {
    if !themes_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut extensions = BTreeMap::<String, InstalledThemeExtension>::new();
    for entry in
        fs::read_dir(themes_dir).with_context(|| format!("reading {}", themes_dir.display()))?
    {
        let entry = entry?;
        let Some(id) = managed_extension_id(&entry.path()) else {
            continue;
        };
        let bytes = fs::read(entry.path())
            .with_context(|| format!("reading {}", entry.path().display()))?;
        let family: ThemeNames = serde_json::from_slice(&bytes).with_context(|| {
            format!(
                "reading installed theme names from {}",
                entry.path().display()
            )
        })?;
        let extension = extensions
            .entry(id.clone())
            .or_insert_with(|| InstalledThemeExtension {
                id,
                theme_names: Vec::new(),
                file_count: 0,
            });
        extension.file_count += 1;
        extension
            .theme_names
            .extend(family.themes.into_iter().map(|theme| theme.name));
    }
    for extension in extensions.values_mut() {
        extension.theme_names.sort();
        extension.theme_names.dedup();
    }
    Ok(extensions.into_values().collect())
}

pub fn remove(extension_id: &str, themes_dir: &Path) -> Result<usize> {
    if !themes_dir.is_dir() {
        return Ok(0);
    }
    let safe_id = safe_file_component(extension_id);
    let mut removed = 0;
    for entry in
        fs::read_dir(themes_dir).with_context(|| format!("reading {}", themes_dir.display()))?
    {
        let entry = entry?;
        if managed_extension_id(&entry.path()).as_deref() != Some(safe_id.as_str()) {
            continue;
        }
        fs::remove_file(entry.path())
            .with_context(|| format!("removing {}", entry.path().display()))?;
        removed += 1;
    }
    Ok(removed)
}

#[derive(Deserialize)]
struct ThemeNames {
    themes: Vec<ThemeName>,
}

#[derive(Deserialize)]
struct ThemeName {
    name: String,
}

fn managed_extension_id(path: &Path) -> Option<String> {
    let file_name = path.file_name()?.to_str()?;
    let (id, remainder) = file_name.split_once("--")?;
    let (index, theme_file) = remainder.split_once("--")?;
    (!id.is_empty() && index.parse::<usize>().is_ok() && theme_file.ends_with(".json"))
        .then(|| id.to_owned())
}
fn manifest_theme_paths(root: &Path) -> Result<Vec<PathBuf>> {
    let toml_path = root.join("extension.toml");
    if toml_path.is_file() {
        let manifest: Manifest =
            toml::from_str(&fs::read_to_string(&toml_path)?).context("parsing extension.toml")?;
        return Ok(manifest.themes);
    }
    let json_path = root.join("extension.json");
    if json_path.is_file() {
        let manifest: OldManifest =
            serde_json::from_slice(&fs::read(&json_path)?).context("parsing extension.json")?;
        return Ok(manifest.themes.into_values().collect());
    }
    bail!("extension archive has no manifest")
}

fn validate_relative_theme_path(path: &Path) -> Result<()> {
    if path.extension().and_then(|extension| extension.to_str()) != Some("json")
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("invalid theme path {}", path.display());
    }
    Ok(())
}

/// Accepts a value only if it is already safe to splice into a URL path: no
/// separators, no percent escapes, no dot segments, nothing non-ASCII.
/// Sanitising instead of rejecting would silently request a different extension
/// than the one the user chose.
fn url_path_segment(value: &str) -> Option<&str> {
    let usable = !value.is_empty()
        && value != "."
        && value != ".."
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        });
    usable.then_some(value)
}

fn safe_file_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "tests/theme_extensions.rs"]
mod tests;
