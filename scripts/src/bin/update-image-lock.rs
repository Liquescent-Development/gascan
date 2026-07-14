use std::{
    collections::BTreeMap,
    error::Error,
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use reqwest::{
    Url,
    blocking::{Client, Response},
    header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT},
    redirect::{Action, Attempt, Policy},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

type DynError = Box<dyn Error + Send + Sync>;

#[derive(Deserialize)]
struct Inputs {
    ubuntu: String,
    ubuntu_snapshot: String,
    mise: String,
    playwright_chromium_channel: String,
    gascamp_revision: String,
    tools: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct DockerToken {
    token: String,
}

#[derive(Deserialize)]
struct ImageIndex {
    manifests: Vec<ImageManifest>,
}

#[derive(Deserialize)]
struct ImageManifest {
    digest: String,
    platform: Platform,
}

#[derive(Deserialize)]
struct Platform {
    architecture: String,
    os: String,
    #[serde(default)]
    variant: String,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Deserialize)]
struct BrowserManifest {
    browsers: Vec<Browser>,
}

#[derive(Deserialize)]
struct Browser {
    name: String,
    revision: String,
    #[serde(rename = "browserVersion")]
    browser_version: Option<String>,
}

#[derive(Serialize)]
struct VersionedArtifact {
    version: String,
    url: String,
    sha256: String,
}

#[derive(Serialize)]
struct GascampLock {
    revision: String,
}

#[derive(Serialize)]
struct ImageLock {
    base_image: String,
    ubuntu_snapshot: String,
    workspace_tag: String,
    mise: VersionedArtifact,
    playwright_chromium: VersionedArtifact,
    gascamp: GascampLock,
    tools: BTreeMap<String, String>,
}

fn main() -> Result<(), DynError> {
    let root = repository_root()?;
    let input_path = root.join("images/workspace/versions.toml");
    let inputs: Inputs = toml::from_str(&fs::read_to_string(input_path)?)?;
    validate_inputs(&inputs)?;

    let client = http_client()?;
    let base_image = resolve_ubuntu(&client, &inputs.ubuntu)?;
    let (mise, resolver) = resolve_mise(&client, &inputs.mise)?;
    let tools = resolve_tools(&client, &resolver, &inputs.tools)?;
    let playwright_chromium = resolve_chromium(&client, &inputs.playwright_chromium_channel)?;

    let mut lock = ImageLock {
        base_image,
        ubuntu_snapshot: inputs.ubuntu_snapshot,
        workspace_tag: String::new(),
        mise,
        playwright_chromium,
        gascamp: GascampLock {
            revision: inputs.gascamp_revision,
        },
        tools,
    };
    let identity = toml::to_string(&lock)?;
    lock.workspace_tag = format!("gascan-workspace:{}", &sha256(identity.as_bytes())[..16]);
    let lock_path = root.join("images/workspace/versions.lock");
    eprintln!("image-lock: writing {}", lock_path.display());
    write_atomic(&lock_path, toml::to_string_pretty(&lock)?.as_bytes())?;
    eprintln!("image-lock: wrote {}", lock_path.display());
    Ok(())
}

fn repository_root() -> Result<PathBuf, DynError> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "scripts directory has no repository parent".into())
}

fn validate_inputs(inputs: &Inputs) -> Result<(), DynError> {
    if inputs.ubuntu != "24.04" {
        return Err("only the reviewed Ubuntu 24.04 input is accepted".into());
    }
    if inputs.ubuntu_snapshot != "2026-07-13T00:00:00Z" {
        return Err("Ubuntu snapshot timestamp differs from the reviewed input".into());
    }
    if !lower_hex(&inputs.gascamp_revision, 40) {
        return Err("Gascamp revision must be 40 lowercase hexadecimal characters".into());
    }
    if inputs.tools.is_empty() {
        return Err("at least one default tool alias is required".into());
    }
    Ok(())
}

fn http_client() -> Result<Client, DynError> {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static("gascan-image-lock/1"));
    Ok(Client::builder()
        .default_headers(headers)
        .redirect(Policy::custom(validate_redirect))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .build()?)
}

fn validate_redirect(attempt: Attempt<'_>) -> Action {
    if approved_host(attempt.url()) {
        attempt.follow()
    } else {
        attempt.error("redirect target is outside approved release hosts")
    }
}

fn approved_host(url: &Url) -> bool {
    matches!(
        url.host_str(),
        Some(
            "api.github.com"
                | "github.com"
                | "objects.githubusercontent.com"
                | "release-assets.githubusercontent.com"
                | "raw.githubusercontent.com"
                | "static.rust-lang.org"
                | "auth.docker.io"
                | "registry-1.docker.io"
                | "cdn.playwright.dev"
                | "playwright.download.prss.microsoft.com"
        )
    )
}

fn get(client: &Client, url: &str) -> Result<Response, DynError> {
    let parsed = Url::parse(url)?;
    if !approved_host(&parsed) {
        return Err(format!("unapproved release host: {parsed}").into());
    }
    eprintln!("image-lock: GET {parsed}");
    Ok(client.get(parsed).send()?.error_for_status()?)
}

fn resolve_ubuntu(client: &Client, version: &str) -> Result<String, DynError> {
    eprintln!("image-lock: resolving ubuntu:{version} Linux ARM64 digest");
    let token: DockerToken = get(
        client,
        "https://auth.docker.io/token?service=registry.docker.io&scope=repository:library/ubuntu:pull",
    )?
    .json()?;
    let manifest_url =
        format!("https://registry-1.docker.io/v2/library/ubuntu/manifests/{version}");
    eprintln!("image-lock: GET {manifest_url}");
    let index: ImageIndex = client
        .get(manifest_url)
        .header(AUTHORIZATION, format!("Bearer {}", token.token))
        .header(
            ACCEPT,
            "application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json",
        )
        .send()?
        .error_for_status()?
        .json()?;
    let digest = index
        .manifests
        .into_iter()
        .find(|manifest| {
            manifest.platform.os == "linux"
                && manifest.platform.architecture == "arm64"
                && (manifest.platform.variant.is_empty() || manifest.platform.variant == "v8")
        })
        .ok_or("Ubuntu index has no Linux ARM64 manifest")?
        .digest;
    if !digest.starts_with("sha256:") || !lower_hex(&digest[7..], 64) {
        return Err("Ubuntu ARM64 manifest digest is malformed".into());
    }
    Ok(format!("ubuntu@{digest}"))
}

fn resolve_mise(
    client: &Client,
    version: &str,
) -> Result<(VersionedArtifact, tempfile::TempDir), DynError> {
    eprintln!("image-lock: resolving mise {version}");
    let release: GithubRelease = get(
        client,
        &format!("https://api.github.com/repos/jdx/mise/releases/tags/v{version}"),
    )?
    .json()?;
    if release.tag_name != format!("v{version}") {
        return Err("mise release tag does not exactly match the requested version".into());
    }
    let linux_name = format!("mise-v{version}-linux-arm64");
    let host_name = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => format!("mise-v{version}-macos-arm64"),
        ("linux", "aarch64") => linux_name.clone(),
        _ => return Err("lock updater requires an ARM64 macOS or Linux host".into()),
    };
    let checksum_url = asset_url(&release.assets, "SHASUMS256.txt")?;
    let checksums = get(client, &checksum_url)?.text()?;
    let linux_sha = checksum_for(&checksums, &linux_name)?;
    let host_sha = checksum_for(&checksums, &host_name)?;
    let linux_url = asset_url(&release.assets, &linux_name)?;
    let host_url = asset_url(&release.assets, &host_name)?;
    let linux_bytes = get(client, &linux_url)?.bytes()?;
    verify_sha(&linux_bytes, &linux_sha, &linux_name)?;
    let host_bytes = if host_name == linux_name {
        linux_bytes.clone()
    } else {
        let bytes = get(client, &host_url)?.bytes()?;
        verify_sha(&bytes, &host_sha, &host_name)?;
        bytes
    };
    let resolver = tempfile::tempdir()?;
    let resolver_path = resolver.path().join("mise");
    fs::write(&resolver_path, &host_bytes)?;
    fs::set_permissions(&resolver_path, fs::Permissions::from_mode(0o700))?;
    Ok((
        VersionedArtifact {
            version: version.to_owned(),
            url: linux_url,
            sha256: linux_sha,
        },
        resolver,
    ))
}

fn asset_url(assets: &[GithubAsset], name: &str) -> Result<String, DynError> {
    assets
        .iter()
        .find(|asset| asset.name == name)
        .map(|asset| asset.browser_download_url.clone())
        .ok_or_else(|| format!("release asset is missing: {name}").into())
}

fn checksum_for(checksums: &str, name: &str) -> Result<String, DynError> {
    checksums
        .lines()
        .filter_map(|line| line.split_once(char::is_whitespace))
        .find(|(_, candidate)| {
            candidate
                .trim_start_matches('*')
                .trim()
                .trim_start_matches("./")
                == name
        })
        .map(|(checksum, _)| checksum.to_owned())
        .filter(|checksum| lower_hex(checksum, 64))
        .ok_or_else(|| format!("valid published checksum is missing for {name}").into())
}

fn resolve_tools(
    client: &Client,
    resolver: &tempfile::TempDir,
    aliases: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, DynError> {
    let mut tools = BTreeMap::new();
    for (tool, alias) in aliases {
        if tool == "rust" && alias == "stable" {
            eprintln!("image-lock: resolving Rust stable from official channel manifest");
            let manifest = get(
                client,
                "https://static.rust-lang.org/dist/channel-rust-stable.toml",
            )?
            .text()?;
            tools.insert(tool.clone(), rust_version_from_channel(&manifest)?);
            continue;
        }
        eprintln!("image-lock: resolving tool alias {tool}@{alias} (60s deadline)");
        let output = run_mise_latest(resolver, tool, alias)?;
        if !output.status.success() {
            return Err(format!(
                "mise failed to resolve {tool}@{alias}: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        let version = String::from_utf8(output.stdout)?.trim().to_owned();
        if version.is_empty()
            || matches!(version.as_str(), "latest" | "stable" | "lts")
            || version.contains('*')
        {
            return Err(format!("mise left {tool}@{alias} unresolved as {version:?}").into());
        }
        tools.insert(tool.clone(), version);
    }
    Ok(tools)
}

fn rust_version_from_channel(manifest: &str) -> Result<String, DynError> {
    let value: toml::Value = toml::from_str(manifest)?;
    let declared = value
        .get("pkg")
        .and_then(|pkg| pkg.get("rust"))
        .and_then(|rust| rust.get("version"))
        .and_then(toml::Value::as_str)
        .ok_or("Rust stable channel manifest omitted pkg.rust.version")?;
    let version = declared
        .split_whitespace()
        .next()
        .ok_or("Rust stable channel version is empty")?;
    let parts: Vec<_> = version.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(format!("Rust stable channel version is malformed: {declared:?}").into());
    }
    Ok(version.to_owned())
}

fn run_mise_latest(
    resolver: &tempfile::TempDir,
    tool: &str,
    alias: &str,
) -> Result<std::process::Output, DynError> {
    let mut child = Command::new(resolver.path().join("mise"))
        .args(["latest", &format!("{tool}@{alias}")])
        .env("MISE_DATA_DIR", resolver.path().join("data"))
        .env("MISE_CACHE_DIR", resolver.path().join("cache"))
        .env("MISE_STATE_DIR", resolver.path().join("state"))
        .env("MISE_CONFIG_DIR", resolver.path().join("config"))
        .env("MISE_NO_CONFIG", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let started = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            return Ok(child.wait_with_output()?);
        }
        if started.elapsed() >= Duration::from_secs(60) {
            child.kill()?;
            let _ = child.wait();
            return Err(format!("mise timed out resolving {tool}@{alias} after 60 seconds").into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn resolve_chromium(client: &Client, channel: &str) -> Result<VersionedArtifact, DynError> {
    eprintln!("image-lock: resolving Playwright {channel} Linux ARM64 artifact");
    if channel != "chromium" {
        return Err("only the reviewed Playwright Chromium channel is accepted".into());
    }
    let release: GithubRelease = get(
        client,
        "https://api.github.com/repos/microsoft/playwright/releases/latest",
    )?
    .json()?;
    if !release.tag_name.starts_with('v') {
        return Err("Playwright release tag is malformed".into());
    }
    let manifest: BrowserManifest = get(
        client,
        &format!(
            "https://raw.githubusercontent.com/microsoft/playwright/{}/packages/playwright-core/browsers.json",
            release.tag_name
        ),
    )?
    .json()?;
    let (browser_version, revision) = chromium_from_manifest(manifest, channel)?;
    let url = format!(
        "https://cdn.playwright.dev/dbazure/download/playwright/builds/chromium/{}/chromium-linux-arm64.zip",
        revision
    );
    let bytes = get(client, &url)?.bytes()?;
    Ok(VersionedArtifact {
        version: format!("{browser_version}+{revision}"),
        url,
        sha256: sha256(&bytes),
    })
}

fn chromium_from_manifest(
    manifest: BrowserManifest,
    channel: &str,
) -> Result<(String, String), DynError> {
    let browser = manifest
        .browsers
        .into_iter()
        .find(|browser| browser.name == channel)
        .ok_or("tagged Playwright manifest has no Chromium channel")?;
    if !browser.revision.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Playwright Chromium revision is malformed".into());
    }
    let version = browser
        .browser_version
        .ok_or("Playwright Chromium entry omitted browserVersion")?;
    Ok((version, browser.revision))
}

fn verify_sha(bytes: &[u8], expected: &str, name: &str) -> Result<(), DynError> {
    if sha256(bytes) != expected {
        return Err(format!("published SHA-256 mismatch for {name}").into());
    }
    Ok(())
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), DynError> {
    let parent = path.parent().ok_or("lock path has no parent")?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file_mut().sync_all()?;
    temporary.persist(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{BrowserManifest, checksum_for, chromium_from_manifest, rust_version_from_channel};

    #[test]
    fn checksum_parser_accepts_release_dot_slash_names() {
        let checksum = "fba7c8a383cf3c59eb5a9995d5299fd2c78eba7eb1daace48d75fe491362f79a  ./mise-v2026.5.0-linux-arm64\n";
        assert_eq!(
            checksum_for(checksum, "mise-v2026.5.0-linux-arm64").unwrap(),
            "fba7c8a383cf3c59eb5a9995d5299fd2c78eba7eb1daace48d75fe491362f79a"
        );
    }

    #[test]
    fn rust_channel_parser_extracts_exact_package_version() {
        let manifest = r#"manifest-version = "2"
[pkg.rust]
version = "1.92.0 (ded5c06cf 2025-12-08)"
"#;
        assert_eq!(rust_version_from_channel(manifest).unwrap(), "1.92.0");
    }

    #[test]
    fn chromium_parser_ignores_versionless_unrelated_channels() {
        let manifest: BrowserManifest = serde_json::from_str(
            r#"{"browsers":[{"name":"ffmpeg","revision":"1011"},{"name":"chromium","revision":"1228","browserVersion":"149.0.7827.55"}]}"#,
        )
        .unwrap();
        assert_eq!(
            chromium_from_manifest(manifest, "chromium").unwrap(),
            ("149.0.7827.55".to_owned(), "1228".to_owned())
        );
    }
}
