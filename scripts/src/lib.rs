#![forbid(unsafe_code)]

pub mod bundle;

use std::{
    collections::BTreeSet,
    error::Error,
    fs,
    io::{Read, Write},
    path::Path,
    time::Duration,
};

use reqwest::{
    Url,
    blocking::{Client, Response},
    redirect::{Action, Attempt, Policy},
};
use sha2::{Digest, Sha256};

pub type DynError = Box<dyn Error + Send + Sync>;

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
    let mut file = fs::File::open(path)?;
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
    Ok(())
}

pub fn install_verified_artifact(
    mut input: impl Read,
    destination: &Path,
    expected_sha256: &str,
    expected_size: u64,
    class: ArtifactClass,
) -> Result<(), DynError> {
    validate_expectations(expected_sha256, expected_size, class.maximum_bytes())?;
    let parent = destination
        .parent()
        .ok_or("artifact destination has no parent")?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
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
        if size > expected_size || size > class.maximum_bytes() {
            return Err("artifact exceeded its exact size limit".into());
        }
        temporary.write_all(&buffer[..count])?;
        hasher.update(&buffer[..count]);
    }
    if size != expected_size {
        return Err("artifact size does not match lock".into());
    }
    if format!("{:x}", hasher.finalize()) != expected_sha256 {
        return Err("artifact SHA-256 does not match lock".into());
    }
    temporary.as_file_mut().sync_all()?;
    temporary.persist(destination)?;
    Ok(())
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
