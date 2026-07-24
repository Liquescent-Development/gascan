use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs,
    io::{Cursor, Read, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use flate2::read::GzDecoder;
use reqwest::{
    Url,
    blocking::{Client, Response},
    header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT},
    redirect::{Action, Attempt, Policy},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};

type DynError = Box<dyn Error + Send + Sync>;

#[derive(Deserialize)]
struct Inputs {
    ubuntu: String,
    ubuntu_snapshot: String,
    mise: String,
    playwright_chromium_channel: String,
    gascamp_revision: String,
    tools: BTreeMap<String, String>,
    workstation: BTreeMap<String, String>,
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
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

#[derive(Deserialize)]
struct NpmPackage {
    version: String,
    dist: NpmDist,
}

#[derive(Deserialize)]
struct NpmDist {
    tarball: String,
    integrity: String,
}

#[derive(Deserialize)]
struct GitlabRelease {
    tag_name: String,
    assets: GitlabAssets,
}

#[derive(Deserialize)]
struct GitlabAssets {
    links: Vec<GitlabAsset>,
}

#[derive(Deserialize)]
struct GitlabAsset {
    name: String,
    direct_asset_url: String,
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct VersionedArtifact {
    version: String,
    url: String,
    sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct WorkstationArtifact {
    version: String,
    url: String,
    sha256: String,
    platform: String,
    kind: String,
    size: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct NpmLifecycleException {
    package: String,
    version: String,
    command: String,
    integrity: String,
    manifest_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    script_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    script_sha256: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ClaudeNative {
    package: String,
    version: String,
    url: String,
    integrity: String,
    sha256: String,
    size: u64,
    binary_path: String,
    binary_sha256: String,
    binary_size: u64,
    platform: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct WorkstationNpm {
    scripts: String,
    npm_version: String,
    package_manifest_sha256: String,
    package_lock_sha256: String,
    lifecycle_exceptions: BTreeMap<String, NpmLifecycleException>,
    claude_native: ClaudeNative,
}

#[derive(Deserialize)]
struct WorkstationValidationLock {
    workstation_artifacts: BTreeMap<String, WorkstationArtifact>,
    workstation_npm: WorkstationNpm,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct GascampLock {
    revision: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct WorkspaceBundles {
    media_type: String,
    platform: String,
    publication: String,
}

#[derive(Deserialize)]
struct ExistingImageLock {
    base_image: String,
    workspace_build_mode: String,
    ubuntu_snapshot: String,
    workspace_bundles: WorkspaceBundles,
    mise: VersionedArtifact,
    playwright_chromium: VersionedArtifact,
    gascamp: GascampLock,
    tools: BTreeMap<String, String>,
}

#[derive(Serialize)]
struct ImageLock {
    base_image: String,
    workspace_build_mode: String,
    ubuntu_snapshot: String,
    workspace_tag: String,
    workspace_bundles: WorkspaceBundles,
    mise: VersionedArtifact,
    playwright_chromium: VersionedArtifact,
    gascamp: GascampLock,
    tools: BTreeMap<String, String>,
    workstation_artifacts: BTreeMap<String, WorkstationArtifact>,
    workstation_npm: WorkstationNpm,
}

fn main() -> Result<(), DynError> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if let [mode, path] = arguments.as_slice() {
        if mode == "--validate-workstation-lock" {
            return validate_workstation_lock_file(Path::new(path));
        }
    }
    if let [mode, npm_manifest, npm_lock, image_lock] = arguments.as_slice() {
        if mode == "--validate-workstation-package-lock" {
            return validate_workstation_package_lock_files(
                Path::new(npm_manifest),
                Path::new(npm_lock),
                Path::new(image_lock),
            );
        }
    }
    if !arguments.is_empty() {
        return Err("usage: update-image-lock [--validate-workstation-lock PATH | --validate-workstation-package-lock NPM_MANIFEST NPM_LOCK IMAGE_LOCK]".into());
    }

    let root = repository_root()?;
    let input_path = root.join("images/workspace/versions.toml");
    let inputs: Inputs = toml::from_str(&fs::read_to_string(input_path)?)?;
    validate_inputs(&inputs)?;
    let lock_path = root.join("images/workspace/versions.lock");
    let existing_text = fs::read_to_string(&lock_path)?;
    let existing: ExistingImageLock = toml::from_str(&existing_text)?;
    let existing_value: toml::Value = toml::from_str(&existing_text)?;

    let client = http_client()?;
    let resolved_base_image = resolve_ubuntu(&client, &inputs.ubuntu)?;
    let (resolved_mise, resolver) = resolve_mise(&client, &inputs.mise)?;
    let resolved_tools = resolve_tools(&client, &resolver, &inputs.tools)?;
    let resolved_chromium = resolve_chromium(&client, &inputs.playwright_chromium_channel)?;
    report_preserved_drift("base_image", &resolved_base_image, &existing.base_image);
    report_preserved_drift("mise", &resolved_mise, &existing.mise);
    report_preserved_drift("tools", &resolved_tools, &existing.tools);
    report_preserved_drift(
        "playwright_chromium",
        &resolved_chromium,
        &existing.playwright_chromium,
    );
    let workstation_artifacts = resolve_workstation(&client, &inputs.workstation)?;
    let (workstation_manifest, workstation_npm_lock) =
        generate_workstation_npm_lock(&client, &workstation_artifacts)?;
    let workstation_npm = resolve_workstation_npm(
        &client,
        &workstation_manifest,
        &workstation_npm_lock,
        &workstation_artifacts,
    )?;

    let lock = ImageLock {
        base_image: existing.base_image,
        workspace_build_mode: existing.workspace_build_mode,
        ubuntu_snapshot: existing.ubuntu_snapshot,
        workspace_tag: String::new(),
        workspace_bundles: existing.workspace_bundles,
        mise: existing.mise,
        playwright_chromium: existing.playwright_chromium,
        gascamp: existing.gascamp,
        tools: existing.tools,
        workstation_artifacts,
        workstation_npm,
    };
    validate_workstation_artifacts(&lock.workstation_artifacts)?;
    validate_workstation_npm(&lock.workstation_npm)?;
    let mut output = merge_workstation_sections(
        existing_value,
        toml::Value::try_from(&lock.workstation_artifacts)?,
        toml::Value::try_from(&lock.workstation_npm)?,
    )?;
    output
        .as_table_mut()
        .ok_or("image lock root is not a TOML table")?
        .insert(
            "workspace_tag".to_owned(),
            toml::Value::String(String::new()),
        );
    let identity = toml::to_string(&output)?;
    let workspace_tag = format!("gascan-workspace:{}", &sha256(identity.as_bytes())[..16]);
    output
        .as_table_mut()
        .ok_or("image lock root is not a TOML table")?
        .insert(
            "workspace_tag".to_owned(),
            toml::Value::String(workspace_tag),
        );
    eprintln!("image-lock: writing {}", lock_path.display());
    write_atomic(
        &root.join("images/workspace/workstation-package.json"),
        &workstation_manifest,
    )?;
    write_atomic(
        &root.join("images/workspace/workstation-package-lock.json"),
        &workstation_npm_lock,
    )?;
    write_atomic(&lock_path, toml::to_string_pretty(&output)?.as_bytes())?;
    eprintln!("image-lock: wrote {}", lock_path.display());
    Ok(())
}

fn merge_workstation_sections(
    mut existing: toml::Value,
    workstation_artifacts: toml::Value,
    workstation_npm: toml::Value,
) -> Result<toml::Value, DynError> {
    let table = existing
        .as_table_mut()
        .ok_or("image lock root is not a TOML table")?;
    table.insert("workstation_artifacts".to_owned(), workstation_artifacts);
    table.insert("workstation_npm".to_owned(), workstation_npm);
    Ok(existing)
}

fn report_preserved_drift<T: std::fmt::Debug + PartialEq>(name: &str, resolved: &T, existing: &T) {
    if resolved != existing {
        eprintln!(
            "image-lock: {name} resolver drifted to {resolved:?}; preserving reviewed lock {existing:?}"
        );
    }
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
    let expected = BTreeMap::from([
        ("claude".to_owned(), "latest".to_owned()),
        ("codex".to_owned(), "latest".to_owned()),
        ("glab".to_owned(), "latest".to_owned()),
        ("herdr".to_owned(), "latest".to_owned()),
        ("neovim".to_owned(), "0.11".to_owned()),
        ("pi".to_owned(), "latest".to_owned()),
    ]);
    if inputs.workstation != expected {
        return Err("workstation resolver intent differs from the reviewed inputs".into());
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
                | "registry.npmjs.org"
                | "gitlab.com"
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
            let resolved = rust_version_from_channel(&manifest)?;
            tools.insert(tool.clone(), resolved);
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

fn resolve_workstation(
    client: &Client,
    intent: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, WorkstationArtifact>, DynError> {
    let mut artifacts = BTreeMap::new();
    for (tool, package) in [
        ("claude", "@anthropic-ai/claude-code"),
        ("codex", "@openai/codex"),
        ("pi", "@earendil-works/pi-coding-agent"),
    ] {
        let alias = intent
            .get(tool)
            .ok_or_else(|| format!("workstation intent omitted {tool}"))?;
        artifacts.insert(
            tool.to_owned(),
            resolve_npm_artifact(client, package, alias)?,
        );
    }
    artifacts.insert(
        "herdr".to_owned(),
        resolve_github_artifact(
            client,
            "ogulcancelik/herdr",
            intent
                .get("herdr")
                .ok_or("workstation intent omitted herdr")?,
            |name| name == "herdr-linux-aarch64",
        )?,
    );
    artifacts.insert(
        "glab".to_owned(),
        resolve_glab(
            client,
            intent
                .get("glab")
                .ok_or("workstation intent omitted glab")?,
        )?,
    );
    artifacts.insert(
        "neovim".to_owned(),
        resolve_github_artifact(
            client,
            "neovim/neovim",
            intent
                .get("neovim")
                .ok_or("workstation intent omitted neovim")?,
            |name| name == "nvim-linux-arm64.tar.gz",
        )?,
    );
    validate_workstation_artifacts(&artifacts)?;
    Ok(artifacts)
}

fn resolve_npm_artifact(
    client: &Client,
    package: &str,
    alias: &str,
) -> Result<WorkstationArtifact, DynError> {
    if alias != "latest" {
        return Err(format!("unsupported npm resolver intent for {package}: {alias}").into());
    }
    let encoded = package.replace('@', "%40").replace('/', "%2F");
    let metadata: NpmPackage = get(
        client,
        &format!("https://registry.npmjs.org/{encoded}/latest"),
    )?
    .json()?;
    let bytes = get_bounded(client, &metadata.dist.tarball, 64 * 1024 * 1024)?;
    let size = u64::try_from(bytes.len())?;
    Ok(WorkstationArtifact {
        version: metadata.version,
        url: metadata.dist.tarball,
        sha256: sha256(&bytes),
        platform: "linux-arm64".to_owned(),
        kind: "npm_tgz".to_owned(),
        size,
    })
}

fn resolve_github_artifact(
    client: &Client,
    repository: &str,
    intent: &str,
    matches_asset: impl Fn(&str) -> bool,
) -> Result<WorkstationArtifact, DynError> {
    let release = if intent == "latest" {
        get(
            client,
            &format!("https://api.github.com/repos/{repository}/releases/latest"),
        )?
        .json::<GithubRelease>()?
    } else {
        let releases = get(
            client,
            &format!("https://api.github.com/repos/{repository}/releases?per_page=100"),
        )?
        .json::<Vec<GithubRelease>>()?;
        releases
            .into_iter()
            .find(|release| {
                !release.draft
                    && !release.prerelease
                    && release
                        .tag_name
                        .strip_prefix('v')
                        .is_some_and(|version| version.starts_with(&format!("{intent}.")))
            })
            .ok_or_else(|| format!("{repository} has no stable v{intent}.x release"))?
    };
    if release.draft || release.prerelease {
        return Err(format!("{repository} resolved to a non-stable release").into());
    }
    let mut candidates = release
        .assets
        .iter()
        .filter(|asset| matches_asset(&asset.name));
    let asset = candidates
        .next()
        .ok_or_else(|| format!("Linux ARM64 release asset is missing for {repository}"))?;
    if candidates.next().is_some() {
        return Err(format!("Linux ARM64 release asset is ambiguous for {repository}").into());
    }
    let version = release
        .tag_name
        .strip_prefix('v')
        .ok_or_else(|| format!("{repository} release tag is malformed"))?
        .to_owned();
    let maximum = if repository == "ogulcancelik/herdr" {
        64 * 1024 * 1024
    } else {
        128 * 1024 * 1024
    };
    let bytes = get_bounded(client, &asset.browser_download_url, maximum)?;
    let size = u64::try_from(bytes.len())?;
    if size != asset.size {
        return Err(format!("{repository} asset size differs from release metadata").into());
    }
    let digest = sha256(&bytes);
    if let Some(published) = &asset.digest
        && published != &format!("sha256:{digest}")
    {
        return Err(format!("{repository} asset digest differs from release metadata").into());
    }
    Ok(WorkstationArtifact {
        version,
        url: asset.browser_download_url.clone(),
        sha256: digest,
        platform: "linux-arm64".to_owned(),
        kind: if repository == "ogulcancelik/herdr" {
            "raw_binary"
        } else {
            "tar_gz"
        }
        .to_owned(),
        size,
    })
}

fn resolve_glab(client: &Client, intent: &str) -> Result<WorkstationArtifact, DynError> {
    if intent != "latest" {
        return Err(format!("unsupported glab resolver intent: {intent}").into());
    }
    let release: GitlabRelease = get(
        client,
        "https://gitlab.com/api/v4/projects/gitlab-org%2Fcli/releases/permalink/latest",
    )?
    .json()?;
    let version = release
        .tag_name
        .strip_prefix('v')
        .ok_or("glab release tag is malformed")?
        .to_owned();
    let expected_name = format!("glab_{version}_linux_arm64.tar.gz");
    let asset = release
        .assets
        .links
        .into_iter()
        .find(|asset| asset.name == expected_name)
        .ok_or("glab release omitted its Linux ARM64 tarball")?;
    let bytes = get_bounded(client, &asset.direct_asset_url, 128 * 1024 * 1024)?;
    let size = u64::try_from(bytes.len())?;
    Ok(WorkstationArtifact {
        version,
        url: asset.direct_asset_url,
        sha256: sha256(&bytes),
        platform: "linux-arm64".to_owned(),
        kind: "tar_gz".to_owned(),
        size,
    })
}

fn generate_workstation_npm_lock(
    client: &Client,
    artifacts: &BTreeMap<String, WorkstationArtifact>,
) -> Result<(Vec<u8>, Vec<u8>), DynError> {
    let dependencies = serde_json::Map::from_iter([
        (
            "@anthropic-ai/claude-code".to_owned(),
            serde_json::Value::String(artifacts["claude"].version.clone()),
        ),
        (
            "@openai/codex".to_owned(),
            serde_json::Value::String(artifacts["codex"].version.clone()),
        ),
        (
            "@earendil-works/pi-coding-agent".to_owned(),
            serde_json::Value::String(artifacts["pi"].version.clone()),
        ),
    ]);
    let manifest = serde_json::json!({
        "name": "gascan-workstation-tools",
        "version": "0.0.0",
        "private": true,
        "dependencies": dependencies,
    });
    let mut manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    manifest_bytes.push(b'\n');

    let temporary = tempfile::tempdir()?;
    fs::write(temporary.path().join("package.json"), &manifest_bytes)?;
    fs::write(temporary.path().join("empty-user.npmrc"), b"")?;
    fs::write(temporary.path().join("empty-global.npmrc"), b"")?;
    eprintln!("image-lock: resolving complete workstation npm dependency closure");
    let version = configured_npm_command(temporary.path())
        .arg("--version")
        .output()?;
    if !version.status.success() || String::from_utf8(version.stdout)?.trim() != "11.12.1" {
        return Err("workstation lock generation requires reviewed npm 11.12.1".into());
    }
    let output = configured_npm_command(temporary.path())
        .args([
            "install",
            "--package-lock-only",
            "--ignore-scripts",
            "--omit=dev",
            "--audit=false",
            "--fund=false",
            "--registry=https://registry.npmjs.org/",
            "--install-strategy=hoisted",
            "--include=optional",
            "--legacy-peer-deps=false",
        ])
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "npm failed to resolve workstation package lock: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let lock_bytes = fill_missing_npm_integrities(
        client,
        &fs::read(temporary.path().join("package-lock.json"))?,
    )?;
    validate_npm_lock(&lock_bytes, artifacts)?;
    Ok((manifest_bytes, lock_bytes))
}

fn configured_npm_command(directory: &Path) -> Command {
    let mut command = Command::new("npm");
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_lowercase()
            .starts_with("npm_config_")
        {
            command.env_remove(name);
        }
    }
    command
        .current_dir(directory)
        .arg(format!(
            "--userconfig={}",
            directory.join("empty-user.npmrc").display()
        ))
        .arg(format!(
            "--globalconfig={}",
            directory.join("empty-global.npmrc").display()
        ))
        .env("HOME", directory)
        .env("npm_config_cache", directory.join("npm-cache"))
        .env("npm_config_ignore_scripts", "true")
        .env_remove("NODE_OPTIONS")
        .env_remove("NPM_TOKEN")
        .env_remove("NODE_AUTH_TOKEN");
    command
}

fn fill_missing_npm_integrities(client: &Client, bytes: &[u8]) -> Result<Vec<u8>, DynError> {
    let mut lock: serde_json::Value = serde_json::from_slice(bytes)?;
    let packages = lock["packages"]
        .as_object_mut()
        .ok_or("workstation npm lock omitted packages")?;
    let missing = packages
        .iter()
        .filter_map(|(path, package)| {
            (package.get("resolved").is_some() && package.get("integrity").is_none())
                .then_some(path.clone())
        })
        .collect::<Vec<_>>();
    for path in missing {
        let package = path
            .rsplit("node_modules/")
            .next()
            .ok_or("npm lock path omitted package name")?;
        let entry = packages
            .get(&path)
            .ok_or("npm lock package disappeared during validation")?;
        let version = entry["version"]
            .as_str()
            .ok_or("npm lock package omitted version")?;
        let resolved = entry["resolved"]
            .as_str()
            .ok_or("npm lock package omitted resolved URL")?;
        let encoded = package.replace('@', "%40").replace('/', "%2F");
        let metadata: NpmPackage = get(
            client,
            &format!("https://registry.npmjs.org/{encoded}/{version}"),
        )?
        .json()?;
        if metadata.version != version || metadata.dist.tarball != resolved {
            return Err(format!("{package} registry metadata disagrees with npm lock").into());
        }
        let tarball = get_bounded(client, resolved, 64 * 1024 * 1024)?;
        verify_npm_integrity(&tarball, &metadata.dist.integrity, package)?;
        packages
            .get_mut(&path)
            .and_then(serde_json::Value::as_object_mut)
            .ok_or("npm lock package is not an object")?
            .insert(
                "integrity".to_owned(),
                serde_json::Value::String(metadata.dist.integrity),
            );
    }
    let mut completed = serde_json::to_vec_pretty(&lock)?;
    completed.push(b'\n');
    Ok(completed)
}

fn validate_npm_lock(
    bytes: &[u8],
    artifacts: &BTreeMap<String, WorkstationArtifact>,
) -> Result<(), DynError> {
    let lock: serde_json::Value = serde_json::from_slice(bytes)?;
    if lock["lockfileVersion"].as_u64() != Some(3) {
        return Err("workstation npm lock must use lockfileVersion 3".into());
    }
    let packages = lock["packages"]
        .as_object()
        .ok_or("workstation npm lock omitted packages")?;
    if packages.is_empty() {
        return Err("workstation npm lock has no dependency closure".into());
    }
    for (path, package) in packages.iter().filter(|(path, _)| !path.is_empty()) {
        if package["link"].as_bool() == Some(true) {
            return Err(format!("workstation npm lock contains unsupported link: {path}").into());
        }
        let path_name = path
            .rsplit("node_modules/")
            .next()
            .ok_or("npm lock path omitted package name")?;
        let name = package["name"].as_str().unwrap_or(path_name);
        let version = package["version"]
            .as_str()
            .ok_or_else(|| format!("{path} omitted exact version"))?;
        let resolved = package["resolved"]
            .as_str()
            .ok_or_else(|| format!("{path} omitted resolved URL"))?;
        let integrity = package["integrity"]
            .as_str()
            .ok_or_else(|| format!("{path} omitted integrity"))?;
        validate_npm_tarball_url(name, version, resolved)?;
        validate_sha512_sri(integrity, name)?;
    }
    let root_dependencies = packages[""]["dependencies"]
        .as_object()
        .ok_or("workstation npm lock omitted top-level dependencies")?;
    if root_dependencies.len() != 3 {
        return Err("workstation npm lock must contain exactly three top-level packages".into());
    }
    for (tool, package) in [
        ("claude", "@anthropic-ai/claude-code"),
        ("codex", "@openai/codex"),
        ("pi", "@earendil-works/pi-coding-agent"),
    ] {
        if root_dependencies
            .get(package)
            .and_then(serde_json::Value::as_str)
            != Some(artifacts[tool].version.as_str())
        {
            return Err(
                format!("{package} lock version disagrees with workstation artifact").into(),
            );
        }
    }
    Ok(())
}

fn validate_sha512_sri(integrity: &str, package: &str) -> Result<(), DynError> {
    let encoded = integrity
        .strip_prefix("sha512-")
        .ok_or_else(|| format!("{package} npm integrity is not SHA-512"))?;
    let decoded = BASE64
        .decode(encoded)
        .map_err(|_| format!("{package} npm integrity is malformed"))?;
    if decoded.len() != 64 {
        return Err(format!("{package} npm integrity is not 64 bytes").into());
    }
    Ok(())
}

fn resolve_workstation_npm(
    client: &Client,
    manifest_bytes: &[u8],
    lock_bytes: &[u8],
    artifacts: &BTreeMap<String, WorkstationArtifact>,
) -> Result<WorkstationNpm, DynError> {
    validate_npm_lock(lock_bytes, artifacts)?;
    let lock: serde_json::Value = serde_json::from_slice(lock_bytes)?;
    let packages = lock["packages"]
        .as_object()
        .ok_or("workstation npm lock omitted packages")?;

    let (claude, claude_manifest, claude_path) = resolve_lifecycle_exception(
        client,
        packages,
        "@anthropic-ai/claude-code",
        "postinstall",
        Some("package/install.cjs"),
    )?;
    let (google_genai, _, google_path) =
        resolve_lifecycle_exception(client, packages, "@google/genai", "preinstall", None)?;
    let (protobufjs, _, protobuf_path) = resolve_lifecycle_exception(
        client,
        packages,
        "protobufjs",
        "postinstall",
        Some("package/scripts/postinstall.js"),
    )?;

    let actual_script_packages = packages
        .iter()
        .filter_map(|(path, package)| {
            if package
                .get("hasInstallScript")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
            {
                Some(path.clone())
            } else {
                None
            }
        })
        .collect::<BTreeSet<_>>();
    let expected_script_packages = BTreeSet::from([claude_path, google_path, protobuf_path]);
    if actual_script_packages != expected_script_packages {
        return Err(format!(
            "unreviewed npm lifecycle packages: {:?}",
            actual_script_packages
                .difference(&expected_script_packages)
                .collect::<Vec<_>>()
        )
        .into());
    }

    let native_package = "@anthropic-ai/claude-code-linux-arm64";
    let native_version = claude_manifest["optionalDependencies"][native_package]
        .as_str()
        .ok_or("Claude manifest omitted exact Linux ARM64 optional dependency")?;
    if native_version != claude.version {
        return Err("Claude native package version differs from wrapper version".into());
    }
    let claude_native = resolve_claude_native(client, native_package, native_version)?;
    let workstation_npm = WorkstationNpm {
        scripts: "disabled".to_owned(),
        npm_version: "11.12.1".to_owned(),
        package_manifest_sha256: sha256(manifest_bytes),
        package_lock_sha256: sha256(lock_bytes),
        lifecycle_exceptions: BTreeMap::from([
            ("claude".to_owned(), claude),
            ("google_genai".to_owned(), google_genai),
            ("protobufjs".to_owned(), protobufjs),
        ]),
        claude_native,
    };
    validate_workstation_npm(&workstation_npm)?;
    Ok(workstation_npm)
}

fn resolve_lifecycle_exception(
    client: &Client,
    packages: &serde_json::Map<String, serde_json::Value>,
    package: &str,
    script_name: &str,
    script_path: Option<&str>,
) -> Result<(NpmLifecycleException, serde_json::Value, String), DynError> {
    let suffix = format!("node_modules/{package}");
    let mut matches = packages
        .iter()
        .filter(|(path, _)| path.as_str() == suffix || path.ends_with(&format!("/{suffix}")));
    let (lock_path, locked) = matches
        .next()
        .ok_or_else(|| format!("npm lock omitted lifecycle package {package}"))?;
    if matches.next().is_some() {
        return Err(format!("npm lock contains ambiguous lifecycle package {package}").into());
    }
    if locked["hasInstallScript"].as_bool() != Some(true) {
        return Err(format!("{package} no longer declares an install lifecycle").into());
    }
    let version = locked["version"]
        .as_str()
        .ok_or_else(|| format!("{package} lock omitted version"))?;
    let url = locked["resolved"]
        .as_str()
        .ok_or_else(|| format!("{package} lock omitted resolved URL"))?;
    let integrity = locked["integrity"]
        .as_str()
        .ok_or_else(|| format!("{package} lock omitted integrity"))?;
    validate_npm_tarball_url(package, version, url)?;
    let bytes = get_bounded(client, url, 64 * 1024 * 1024)?;
    verify_npm_integrity(&bytes, integrity, package)?;
    let manifest_bytes = tgz_regular_file(&bytes, "package/package.json", 1024 * 1024)?;
    let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes)?;
    if manifest["name"].as_str() != Some(package) || manifest["version"].as_str() != Some(version) {
        return Err(format!("{package} tarball manifest disagrees with npm lock").into());
    }
    let command = manifest["scripts"][script_name]
        .as_str()
        .ok_or_else(|| format!("{package} omitted reviewed {script_name} command"))?;
    let script_bytes = script_path
        .map(|path| tgz_regular_file(&bytes, path, 1024 * 1024))
        .transpose()?;
    Ok((
        NpmLifecycleException {
            package: package.to_owned(),
            version: version.to_owned(),
            command: command.to_owned(),
            integrity: integrity.to_owned(),
            manifest_sha256: sha256(&manifest_bytes),
            script_path: script_path.map(str::to_owned),
            script_sha256: script_bytes.as_deref().map(sha256),
        },
        manifest,
        lock_path.clone(),
    ))
}

fn resolve_claude_native(
    client: &Client,
    package: &str,
    version: &str,
) -> Result<ClaudeNative, DynError> {
    let encoded = package.replace('@', "%40").replace('/', "%2F");
    let metadata: NpmPackage = get(
        client,
        &format!("https://registry.npmjs.org/{encoded}/{version}"),
    )?
    .json()?;
    if metadata.version != version {
        return Err("Claude native registry version differs from wrapper".into());
    }
    validate_npm_tarball_url(package, version, &metadata.dist.tarball)?;
    let bytes = get_bounded(client, &metadata.dist.tarball, 200 * 1024 * 1024)?;
    verify_npm_integrity(&bytes, &metadata.dist.integrity, package)?;
    let binary_path = "package/claude";
    let binary = tgz_regular_file(&bytes, binary_path, 512 * 1024 * 1024)?;
    if binary.get(..4) != Some(b"\x7fELF")
        || binary.get(4) != Some(&2)
        || binary.get(5) != Some(&1)
        || binary.get(18..20) != Some(&[0xb7, 0x00])
    {
        return Err("Claude native binary is not ELF64 little-endian AArch64".into());
    }
    Ok(ClaudeNative {
        package: package.to_owned(),
        version: version.to_owned(),
        url: metadata.dist.tarball,
        integrity: metadata.dist.integrity,
        sha256: sha256(&bytes),
        size: u64::try_from(bytes.len())?,
        binary_path: binary_path.to_owned(),
        binary_sha256: sha256(&binary),
        binary_size: u64::try_from(binary.len())?,
        platform: "linux-arm64".to_owned(),
    })
}

fn validate_npm_tarball_url(package: &str, version: &str, value: &str) -> Result<(), DynError> {
    let url = Url::parse(value)?;
    let base = package
        .rsplit('/')
        .next()
        .ok_or("npm package name is empty")?;
    let expected = format!("/{package}/-/{base}-{version}.tgz");
    if url.scheme() != "https"
        || url.host_str() != Some("registry.npmjs.org")
        || url.path() != expected
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(format!("{package} has an unapproved npm tarball URL").into());
    }
    Ok(())
}

fn get_bounded(client: &Client, url: &str, maximum: usize) -> Result<Vec<u8>, DynError> {
    let response = get(client, url)?;
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Err(format!("artifact exceeds {maximum}-byte download bound").into());
    }
    let mut bytes =
        Vec::with_capacity(response.content_length().unwrap_or(0).min(maximum as u64) as usize);
    response
        .take(u64::try_from(maximum)? + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        return Err(format!("artifact exceeds {maximum}-byte download bound").into());
    }
    Ok(bytes)
}

fn verify_npm_integrity(bytes: &[u8], integrity: &str, package: &str) -> Result<(), DynError> {
    let expected = integrity
        .strip_prefix("sha512-")
        .ok_or_else(|| format!("{package} npm integrity is not SHA-512"))?;
    let actual = BASE64.encode(Sha512::digest(bytes));
    if actual != expected {
        return Err(format!("{package} npm integrity mismatch").into());
    }
    Ok(())
}

fn tgz_regular_file(bytes: &[u8], path: &str, maximum: usize) -> Result<Vec<u8>, DynError> {
    let decoder = GzDecoder::new(Cursor::new(bytes));
    let mut archive = tar::Archive::new(decoder);
    let mut found = None;
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.path()?.as_ref() != Path::new(path) {
            continue;
        }
        if found.is_some() || !entry.header().entry_type().is_file() {
            return Err(format!("tarball has invalid or duplicate {path}").into());
        }
        if entry.size() > maximum as u64 {
            return Err(format!("tarball member {path} exceeds size bound").into());
        }
        let mut contents = Vec::with_capacity(usize::try_from(entry.size())?);
        entry.read_to_end(&mut contents)?;
        found = Some(contents);
    }
    found.ok_or_else(|| format!("tarball omitted {path}").into())
}

fn reviewed_workstation_npm() -> WorkstationNpm {
    WorkstationNpm {
        scripts: "disabled".to_owned(),
        npm_version: "11.12.1".to_owned(),
        package_manifest_sha256:
            "5942f8228c2e250dddea8688fc8fea2776042baafd40da0d2ac70920a1e165bb".to_owned(),
        package_lock_sha256:
            "22521261a3bee269f2b992011e417eb4c47a94bcde34c92f29b4504f77454d5c".to_owned(),
        lifecycle_exceptions: BTreeMap::from([
            (
                "claude".to_owned(),
                NpmLifecycleException {
                    package: "@anthropic-ai/claude-code".to_owned(),
                    version: "2.1.218".to_owned(),
                    command: "node install.cjs".to_owned(),
                    integrity: "sha512-BHV951ruIa6QXaZFDF1wRhwxAOkAiafB2AOWG6wGRUJ4apaJ9mlzp1BFLAhGfG0SknwAyqBenqeT6nit6at4uQ==".to_owned(),
                    manifest_sha256: "e3ea99daaf9c111af49f7ceb367f0274745565c44762befb03656adbd62b19b0".to_owned(),
                    script_path: Some("package/install.cjs".to_owned()),
                    script_sha256: Some("5cbab1670597f492cd4eeb946f3c344ebcb1fbd43c623ba192c9b33744461b85".to_owned()),
                },
            ),
            (
                "google_genai".to_owned(),
                NpmLifecycleException {
                    package: "@google/genai".to_owned(),
                    version: "1.52.0".to_owned(),
                    command: "echo 'preinstall: no-op'".to_owned(),
                    integrity: "sha512-gwSvbpiN/17O9TbsqSsE/OzZcpv5Fo4RQjdngGgogtuB9RsyJ8ZHhX5KjHj1bp5N9snN2eK8LDGXSaWW2hof8Q==".to_owned(),
                    manifest_sha256: "ec761756421ea5502c23dbebfb4bc2b74c3ff842597199f2f330afc49cbdedc7".to_owned(),
                    script_path: None,
                    script_sha256: None,
                },
            ),
            (
                "protobufjs".to_owned(),
                NpmLifecycleException {
                    package: "protobufjs".to_owned(),
                    version: "7.6.4".to_owned(),
                    command: "node scripts/postinstall".to_owned(),
                    integrity: "sha512-RJJPTTpvFfHcWLkIa2JFWK4XvtSzS0yEWDmunqHXli1h3JlkbcQZXDZdcWxv+JK3Xsl5/UFDPZ0iGm7DAengYw==".to_owned(),
                    manifest_sha256: "f904144a0cf7ac4a5728abaeaf10413dd86ca282dcd387d8008bd9563740e5b1".to_owned(),
                    script_path: Some("package/scripts/postinstall.js".to_owned()),
                    script_sha256: Some("5af8463b97ee8e309b4a2111f9479bacdf0c180de0ca0155527679b1fc6d9e6c".to_owned()),
                },
            ),
        ]),
        claude_native: ClaudeNative {
            package: "@anthropic-ai/claude-code-linux-arm64".to_owned(),
            version: "2.1.218".to_owned(),
            url: "https://registry.npmjs.org/@anthropic-ai/claude-code-linux-arm64/-/claude-code-linux-arm64-2.1.218.tgz".to_owned(),
            integrity: "sha512-CcbVQCzXd9EnlktCEPrkElhdBZuqIWhkeinRGxUuZa6aal4h6J+8Dbo+OnfchBEzd1mahRDQK8BckGBAYozv2g==".to_owned(),
            sha256: "1d3cb5e12f0b653929e34ba046a7ba0a4f5c01eb25ea57b478dbac27e4af9619".to_owned(),
            size: 84_159_749,
            binary_path: "package/claude".to_owned(),
            binary_sha256: "295fd30481bd03b38450fdec2a6e25bb6472c2074f04b0c4a566cd5988f230bf".to_owned(),
            binary_size: 269_990_816,
            platform: "linux-arm64".to_owned(),
        },
    }
}

fn validate_workstation_npm(workstation_npm: &WorkstationNpm) -> Result<(), DynError> {
    if workstation_npm != &reviewed_workstation_npm() {
        return Err("workstation npm lifecycle/native evidence differs from review".into());
    }
    Ok(())
}

fn validate_workstation_lock_file(path: &Path) -> Result<(), DynError> {
    let text = fs::read_to_string(path)?;
    let lock: WorkstationValidationLock = toml::from_str(&text)?;
    validate_workstation_artifacts(&lock.workstation_artifacts)?;
    validate_workstation_npm(&lock.workstation_npm)
}

fn validate_workstation_package_lock_files(
    npm_manifest_path: &Path,
    npm_lock_path: &Path,
    image_lock_path: &Path,
) -> Result<(), DynError> {
    let npm_lock_bytes = fs::read(npm_lock_path)?;
    let image_lock: WorkstationValidationLock =
        toml::from_str(&fs::read_to_string(image_lock_path)?)?;
    if sha256(&fs::read(npm_manifest_path)?) != image_lock.workstation_npm.package_manifest_sha256
        || sha256(&npm_lock_bytes) != image_lock.workstation_npm.package_lock_sha256
    {
        return Err("workstation npm generated input hash differs from image lock".into());
    }
    validate_npm_lock(&npm_lock_bytes, &image_lock.workstation_artifacts)?;
    validate_workstation_npm(&image_lock.workstation_npm)?;
    let npm_lock: serde_json::Value = serde_json::from_slice(&npm_lock_bytes)?;
    let packages = npm_lock["packages"]
        .as_object()
        .ok_or("workstation npm lock omitted packages")?;
    let actual = packages
        .iter()
        .filter(|(_, package)| package["hasInstallScript"].as_bool() == Some(true))
        .collect::<Vec<_>>();
    let reviewed_paths = BTreeMap::from([
        ("node_modules/@anthropic-ai/claude-code", "claude"),
        (
            "node_modules/@earendil-works/pi-coding-agent/node_modules/@google/genai",
            "google_genai",
        ),
        (
            "node_modules/@earendil-works/pi-coding-agent/node_modules/protobufjs",
            "protobufjs",
        ),
    ]);
    if actual
        .iter()
        .map(|(path, _)| path.as_str())
        .collect::<BTreeSet<_>>()
        != reviewed_paths.keys().copied().collect::<BTreeSet<_>>()
    {
        return Err("npm lifecycle package set differs from reviewed evidence".into());
    }
    for (path, package) in actual {
        let evidence_key = reviewed_paths
            .get(path.as_str())
            .ok_or_else(|| format!("unreviewed npm lifecycle package: {path}"))?;
        let evidence = image_lock
            .workstation_npm
            .lifecycle_exceptions
            .get(*evidence_key)
            .ok_or_else(|| format!("lifecycle evidence omitted {evidence_key}"))?;
        if package["version"].as_str() != Some(evidence.version.as_str())
            || package["integrity"].as_str() != Some(evidence.integrity.as_str())
        {
            return Err(format!(
                "npm lifecycle lock evidence drifted for {}",
                evidence.package
            )
            .into());
        }
    }
    Ok(())
}

fn validate_workstation_artifacts(
    artifacts: &BTreeMap<String, WorkstationArtifact>,
) -> Result<(), DynError> {
    let required = ["claude", "codex", "pi", "herdr", "glab", "neovim"];
    if artifacts.len() != required.len()
        || required.iter().any(|name| !artifacts.contains_key(*name))
    {
        return Err("workstation lock must contain each reviewed tool exactly once".into());
    }
    for (name, artifact) in artifacts {
        if artifact.version.is_empty()
            || matches!(artifact.version.as_str(), "latest" | "stable" | "lts")
            || artifact.version.contains('*')
            || artifact
                .version
                .split('.')
                .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return Err(format!("{name} has a mutable workstation version").into());
        }
        if !lower_hex(&artifact.sha256, 64) {
            return Err(format!("{name} has a malformed SHA-256").into());
        }
        if artifact.platform != "linux-arm64" {
            return Err(format!("{name} is not locked for Linux ARM64").into());
        }
        if artifact.size == 0 {
            return Err(format!("{name} has an empty artifact").into());
        }
        let maximum = match name.as_str() {
            "claude" | "codex" | "pi" | "herdr" => 64 * 1024 * 1024,
            "glab" | "neovim" => 128 * 1024 * 1024,
            _ => 0,
        };
        if artifact.size > maximum {
            return Err(format!("{name} artifact exceeds reviewed size bound").into());
        }
        if !matches!(artifact.kind.as_str(), "npm_tgz" | "tar_gz" | "raw_binary") {
            return Err(format!("{name} has an unsupported artifact kind").into());
        }
        validate_workstation_url(name, artifact)?;
    }
    Ok(())
}

fn validate_workstation_url(name: &str, artifact: &WorkstationArtifact) -> Result<(), DynError> {
    let url = Url::parse(&artifact.url)?;
    if url.scheme() != "https" || url.query().is_some() || url.fragment().is_some() {
        return Err(format!("{name} artifact URL is not immutable HTTPS").into());
    }
    let version = &artifact.version;
    let valid = match name {
        "claude" => {
            artifact.kind == "npm_tgz"
                && url.host_str() == Some("registry.npmjs.org")
                && url.path() == format!("/@anthropic-ai/claude-code/-/claude-code-{version}.tgz")
        }
        "codex" => {
            artifact.kind == "npm_tgz"
                && url.host_str() == Some("registry.npmjs.org")
                && url.path() == format!("/@openai/codex/-/codex-{version}.tgz")
        }
        "pi" => {
            artifact.kind == "npm_tgz"
                && url.host_str() == Some("registry.npmjs.org")
                && url.path()
                    == format!("/@earendil-works/pi-coding-agent/-/pi-coding-agent-{version}.tgz")
        }
        "herdr" => {
            artifact.kind == "raw_binary"
                && artifact.size <= 64 * 1024 * 1024
                && url.host_str() == Some("github.com")
                && url.path()
                    == format!(
                        "/ogulcancelik/herdr/releases/download/v{version}/herdr-linux-aarch64"
                    )
        }
        "glab" => {
            artifact.kind == "tar_gz"
                && url.host_str() == Some("gitlab.com")
                && url.path()
                    == format!(
                        "/gitlab-org/cli/-/releases/v{version}/downloads/glab_{version}_linux_arm64.tar.gz"
                    )
        }
        "neovim" => {
            artifact.kind == "tar_gz"
                && url.host_str() == Some("github.com")
                && url.path()
                    == format!(
                        "/neovim/neovim/releases/download/v{version}/nvim-linux-arm64.tar.gz"
                    )
        }
        _ => false,
    };
    if !valid {
        return Err(
            format!("{name} artifact URL is outside its exact official release path").into(),
        );
    }
    Ok(())
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
    use super::{
        BrowserManifest, checksum_for, chromium_from_manifest, configured_npm_command,
        merge_workstation_sections, rust_version_from_channel,
    };

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

    #[test]
    fn workstation_merge_preserves_unknown_top_level_and_nested_lock_records() {
        let existing: toml::Value = toml::from_str(
            r#"workspace_tag = "old"
[future_metadata]
sentinel = "top-level"
[workspace_bundles.future_bundle]
sentinel = "nested"
[workstation_artifacts.old]
version = "old"
[workstation_npm]
scripts = "old"
"#,
        )
        .unwrap();
        let workstation_artifacts: toml::Value =
            toml::from_str("[new]\nversion = \"exact\"\n").unwrap();
        let workstation_npm: toml::Value = toml::from_str("scripts = \"disabled\"\n").unwrap();
        let merged =
            merge_workstation_sections(existing, workstation_artifacts, workstation_npm).unwrap();
        assert_eq!(
            merged["future_metadata"]["sentinel"].as_str(),
            Some("top-level")
        );
        assert_eq!(
            merged["workspace_bundles"]["future_bundle"]["sentinel"].as_str(),
            Some("nested")
        );
        assert!(merged["workstation_artifacts"].get("old").is_none());
        assert_eq!(
            merged["workstation_artifacts"]["new"]["version"].as_str(),
            Some("exact")
        );
        assert_eq!(
            merged["workstation_npm"]["scripts"].as_str(),
            Some("disabled")
        );
    }

    #[test]
    fn npm_command_ignores_hostile_global_config_location() {
        let temporary = tempfile::tempdir().unwrap();
        let expected = temporary.path().join("empty-global.npmrc");
        std::fs::write(temporary.path().join("empty-user.npmrc"), b"").unwrap();
        std::fs::write(&expected, b"").unwrap();
        let hostile_prefix = temporary.path().join("hostile-prefix");
        std::fs::create_dir_all(hostile_prefix.join("etc")).unwrap();
        std::fs::write(hostile_prefix.join("etc/npmrc"), b"legacy-peer-deps=true\n").unwrap();
        let output = configured_npm_command(temporary.path())
            .args(["config", "get", "globalconfig"])
            .env("PREFIX", &hostile_prefix)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().trim(),
            expected.to_string_lossy()
        );
    }
}
