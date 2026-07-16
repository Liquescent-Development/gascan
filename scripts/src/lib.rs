#![forbid(unsafe_code)]

pub mod bundle;

use std::{
    collections::BTreeSet,
    error::Error,
    ffi::OsString,
    fs,
    io::{Read, Write},
    path::{Component, Path},
    time::Duration,
};

use cap_primitives::fs::{
    FollowSymlinks, MetadataExt as CapMetadataExt, OpenOptions as CapOpenOptions,
};
use cap_std::{ambient_authority, fs::Dir};
use reqwest::{
    blocking::{Client, Response},
    redirect::{Action, Attempt, Policy},
    Url,
};
use sha2::{Digest, Sha256};

pub type DynError = Box<dyn Error + Send + Sync>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReviewedInputKind {
    Directory,
    RegularFile,
    Other,
}

pub const fn reviewed_input_kind_allowed(kind: ReviewedInputKind) -> bool {
    matches!(
        kind,
        ReviewedInputKind::Directory | ReviewedInputKind::RegularFile
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactClass {
    Mise,
    Chromium,
    WorkspaceBundle,
}

impl ArtifactClass {
    pub const fn maximum_bytes(self) -> u64 {
        match self {
            Self::Mise => 128 * 1024 * 1024,
            Self::Chromium => 1024 * 1024 * 1024,
            Self::WorkspaceBundle => 16 * 1024 * 1024 * 1024,
        }
    }
}

#[derive(Clone)]
pub struct RedirectRules {
    allowed: BTreeSet<String>,
    allow_http: bool,
    max_redirects: usize,
}

impl RedirectRules {
    pub fn image_artifacts() -> Self {
        Self {
            allowed: [
                "github.com",
                "objects.githubusercontent.com",
                "release-assets.githubusercontent.com",
                "cdn.playwright.dev",
                "playwright.download.prss.microsoft.com",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            allow_http: false,
            max_redirects: 5,
        }
    }

    pub fn for_artifact(class: ArtifactClass) -> Self {
        let hosts: &[&str] = match class {
            ArtifactClass::Mise => &[
                "github.com",
                "objects.githubusercontent.com",
                "release-assets.githubusercontent.com",
            ],
            ArtifactClass::Chromium => &[
                "cdn.playwright.dev",
                "playwright.download.prss.microsoft.com",
            ],
            ArtifactClass::WorkspaceBundle => &[
                "github.com",
                "objects.githubusercontent.com",
                "release-assets.githubusercontent.com",
            ],
        };
        Self {
            allowed: hosts.iter().map(|host| (*host).to_owned()).collect(),
            allow_http: false,
            max_redirects: 5,
        }
    }

    pub fn require_initial_url(&self, url: &str) -> Result<Url, DynError> {
        let initial = Url::parse(url)?;
        if !self.approves(&initial) {
            return Err(format!("artifact URL is not approved: {initial}").into());
        }
        Ok(initial)
    }

    #[doc(hidden)]
    pub fn for_test_http_origins(
        origins: impl IntoIterator<Item = String>,
        max_redirects: usize,
    ) -> Self {
        Self {
            allowed: origins.into_iter().collect(),
            allow_http: true,
            max_redirects,
        }
    }

    fn approves(&self, url: &Url) -> bool {
        let scheme_allowed = url.scheme() == "https" || (self.allow_http && url.scheme() == "http");
        let Some(host) = url.host_str() else {
            return false;
        };
        let authority = match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_owned(),
        };
        scheme_allowed && self.allowed.contains(&authority)
    }

    fn approve_redirect(&self, url: &Url, previous: usize) -> Result<(), DynError> {
        if previous >= self.max_redirects {
            return Err("artifact redirect limit exceeded".into());
        }
        if !self.approves(url) {
            return Err(format!("artifact redirect target is not approved: {url}").into());
        }
        Ok(())
    }
}

pub fn validate_cached_artifact(
    path: &Path,
    expected_sha256: &str,
    expected_size: u64,
) -> Result<(), DynError> {
    validate_expectations(expected_sha256, expected_size, u64::MAX)?;
    let parent_path = path.parent().ok_or("artifact path has no parent")?;
    let (parent, name) = open_parent(path)?;
    let parent_identity = directory_identity(&parent)?;
    let mut options = CapOpenOptions::new();
    options.read(true)._cap_fs_ext_follow(FollowSymlinks::No);
    let mut file = parent.open_with(&name, &options)?;
    if !file.metadata()?.is_file() {
        return Err("cached artifact is not a regular file".into());
    }
    let (size, sha256) = hash_reader(&mut file, expected_size)?;
    if size != expected_size {
        return Err("cached artifact size does not match lock".into());
    }
    if sha256 != expected_sha256 {
        return Err("cached artifact SHA-256 does not match lock".into());
    }
    let reopened = Dir::open_ambient_dir(parent_path, ambient_authority())?;
    if directory_identity(&reopened)? != parent_identity {
        return Err("cached artifact parent changed during validation".into());
    }
    Ok(())
}

pub fn install_verified_artifact(
    input: impl Read,
    destination: &Path,
    expected_sha256: &str,
    expected_size: u64,
    class: ArtifactClass,
) -> Result<(), DynError> {
    install_artifact(
        input,
        destination,
        expected_sha256,
        Some(expected_size),
        class,
    )
}

pub fn install_bounded_artifact(
    input: impl Read,
    destination: &Path,
    expected_sha256: &str,
    class: ArtifactClass,
) -> Result<(), DynError> {
    install_artifact(input, destination, expected_sha256, None, class)
}

fn install_artifact(
    mut input: impl Read,
    destination: &Path,
    expected_sha256: &str,
    expected_size: Option<u64>,
    class: ArtifactClass,
) -> Result<(), DynError> {
    validate_expectations(
        expected_sha256,
        expected_size.unwrap_or(1),
        class.maximum_bytes(),
    )?;
    let parent_path = destination
        .parent()
        .ok_or("artifact destination has no parent")?;
    fs::create_dir_all(parent_path)?;
    let (parent, destination_name) = open_parent(destination)?;
    let parent_identity = directory_identity(&parent)?;
    let temporary_name = OsString::from(format!(".artifact-{}", random_hex_256()?));
    let mut options = CapOpenOptions::new();
    options
        .write(true)
        .create_new(true)
        ._cap_fs_ext_follow(FollowSymlinks::No);
    let mut temporary = parent.open_with(&temporary_name, &options)?;
    let validation = (|| -> Result<(), DynError> {
        let mut hasher = Sha256::new();
        let mut size = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            size = size
                .checked_add(count as u64)
                .ok_or("artifact size overflow")?;
            if size > expected_size.unwrap_or(class.maximum_bytes()) || size > class.maximum_bytes()
            {
                return Err("artifact exceeded its exact size limit".into());
            }
            temporary.write_all(&buffer[..count])?;
            hasher.update(&buffer[..count]);
        }
        if size == 0 || expected_size.is_some_and(|expected| size != expected) {
            return Err("artifact size does not match lock".into());
        }
        if format!("{:x}", hasher.finalize()) != expected_sha256 {
            return Err("artifact SHA-256 does not match lock".into());
        }
        temporary.sync_all()?;
        Ok(())
    })();
    drop(temporary);
    if let Err(error) = validation {
        let _ignored = parent.remove_file(&temporary_name);
        return Err(error);
    }
    if let Err(error) = parent.rename(&temporary_name, &parent, &destination_name) {
        let _ignored = parent.remove_file(&temporary_name);
        return Err(error.into());
    }
    let reopened = Dir::open_ambient_dir(parent_path, ambient_authority())?;
    if directory_identity(&reopened)? != parent_identity {
        return Err("artifact destination parent changed during publication".into());
    }
    parent.into_std_file().sync_all()?;
    Ok(())
}

fn open_parent(path: &Path) -> Result<(Dir, OsString), DynError> {
    let parent = path.parent().ok_or("artifact path has no parent")?;
    let name = path
        .file_name()
        .ok_or("artifact path has no file name")?
        .to_owned();
    if Path::new(&name).components().count() != 1
        || !matches!(
            Path::new(&name).components().next(),
            Some(Component::Normal(_))
        )
    {
        return Err("artifact path does not have a safe final name".into());
    }
    Ok((Dir::open_ambient_dir(parent, ambient_authority())?, name))
}

fn directory_identity(directory: &Dir) -> Result<(u64, u64), DynError> {
    let metadata = directory.dir_metadata()?;
    Ok((metadata.dev(), metadata.ino()))
}

fn random_hex_256() -> Result<String, DynError> {
    let mut bytes = [0_u8; 32];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn validate_expectations(
    expected_sha256: &str,
    expected_size: u64,
    maximum: u64,
) -> Result<(), DynError> {
    if expected_size == 0 || expected_size > maximum {
        return Err("artifact expected size is outside the code-owned limit".into());
    }
    if expected_sha256.len() != 64
        || !expected_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("artifact SHA-256 must be 64 lowercase hexadecimal characters".into());
    }
    Ok(())
}

fn hash_reader(reader: &mut impl Read, expected_size: u64) -> Result<(u64, String), DynError> {
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        size = size
            .checked_add(count as u64)
            .ok_or("artifact size overflow")?;
        if size > expected_size {
            return Err("artifact exceeded its exact size limit".into());
        }
        hasher.update(&buffer[..count]);
    }
    Ok((size, format!("{:x}", hasher.finalize())))
}

pub fn open_with_redirect_rules(url: &str, rules: RedirectRules) -> Result<Response, DynError> {
    let initial = rules.require_initial_url(url)?;
    let redirects = rules.clone();
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .redirect(Policy::custom(move |attempt| {
            validate_redirect(attempt, &redirects)
        }))
        .build()?;
    Ok(client.get(initial).send()?.error_for_status()?)
}

fn validate_redirect(attempt: Attempt<'_>, rules: &RedirectRules) -> Action {
    if let Err(error) = rules.approve_redirect(attempt.url(), attempt.previous().len()) {
        return attempt.error(error);
    }
    attempt.follow()
}

#[doc(hidden)]
pub fn walk_redirects_with(
    initial: &str,
    rules: RedirectRules,
    mut fetch: impl FnMut(&Url) -> Result<Option<Url>, DynError>,
) -> Result<(), DynError> {
    let mut current = Url::parse(initial)?;
    if !rules.approves(&current) {
        return Err(format!("artifact URL is not approved: {current}").into());
    }
    let mut redirects = 0;
    loop {
        let Some(next) = fetch(&current)? else {
            return Ok(());
        };
        rules.approve_redirect(&next, redirects)?;
        redirects += 1;
        current = next;
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct DockerCopy {
    pub sources: Vec<String>,
    pub from_stage: bool,
    pub chmod: Option<u32>,
}

pub fn parse_dockerfile_copies(text: &str) -> Result<Vec<DockerCopy>, DynError> {
    let mut copies = Vec::new();
    for raw in text.lines() {
        if raw.contains('\t')
            || raw
                .bytes()
                .take_while(|byte| byte.is_ascii_whitespace())
                .any(|byte| byte != b' ')
        {
            return Err("unsupported Dockerfile ASCII whitespace".into());
        }
        let line = raw.trim_start_matches(' ');
        if let Some(comment) = line.strip_prefix('#') {
            let directive = comment.trim_start();
            if directive
                .split_once('=')
                .is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case("escape"))
            {
                return Err("Dockerfile escape directives are unsupported".into());
            }
            continue;
        }
        if line.is_empty() {
            continue;
        }
        let mut words = line.split_ascii_whitespace();
        let Some(instruction) = words.next() else {
            continue;
        };
        if !instruction.eq_ignore_ascii_case("COPY") {
            continue;
        }
        if line.contains('\t') || line.ends_with('\\') {
            return Err("unsupported Dockerfile whitespace or continuation".into());
        }
        if line.contains('[') || line.contains('"') || line.contains('\'') {
            return Err("unsupported Dockerfile COPY quoting or JSON form".into());
        }
        let mut from_stage = false;
        let mut chmod = None;
        let mut operands = Vec::new();
        for word in words {
            if operands.is_empty() && word.starts_with("--") {
                if let Some(value) = word.strip_prefix("--from=") {
                    if value.is_empty() {
                        return Err("empty COPY --from".into());
                    }
                    from_stage = true;
                } else if let Some(value) = word.strip_prefix("--chmod=") {
                    chmod = Some(u32::from_str_radix(value, 8)?);
                } else {
                    return Err("unsupported Dockerfile COPY flag".into());
                }
            } else {
                operands.push(word.to_owned());
            }
        }
        if operands.len() < 2 {
            return Err("COPY requires source and destination".into());
        }
        operands.pop();
        copies.push(DockerCopy {
            sources: operands,
            from_stage,
            chmod,
        });
    }
    Ok(copies)
}
