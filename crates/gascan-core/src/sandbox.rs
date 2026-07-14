use crate::manifest::Manifest;
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use thiserror::Error;

pub const WORKSPACE_TARGET: &str = "/workspace";

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
pub struct SandboxId(String);

impl SandboxId {
    pub fn from_root(name: &str, canonical_root: &Utf8Path) -> Self {
        let slug = slugify(name);
        let digest = Sha256::digest(canonical_root.as_str().as_bytes());
        let suffix = digest[..6]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Self(format!("{slug}-{suffix}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Builds a deterministic, valid identity for public test fixtures.
    ///
    /// Production identities must continue to enter through [`Self::from_root`].
    pub fn test(name: &str) -> Self {
        Self::from_root(name, Utf8Path::new("/gascan/test-fixture"))
    }
}

impl TryFrom<String> for SandboxId {
    type Error = SandboxIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        validate_sandbox_id(&value)?;
        Ok(Self(value))
    }
}

impl<'de> Deserialize<'de> for SandboxId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::try_from(value).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for SandboxId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BindMount {
    source: Utf8PathBuf,
    target: Utf8PathBuf,
    writable: bool,
}

impl BindMount {
    pub fn source(&self) -> &Utf8Path {
        &self.source
    }

    pub fn target(&self) -> &Utf8Path {
        &self.target
    }

    pub const fn is_writable(&self) -> bool {
        self.writable
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SandboxSpec {
    id: SandboxId,
    canonical_root: Utf8PathBuf,
    manifest: Manifest,
    bind_mounts: Vec<BindMount>,
}

impl SandboxSpec {
    pub fn from_root(
        name: &str,
        root: &Utf8Path,
        manifest: Manifest,
    ) -> Result<Self, SandboxError> {
        let canonical = std::fs::canonicalize(root).map_err(|source| SandboxError::Io {
            path: root.to_owned(),
            source,
        })?;
        if !canonical.is_dir() {
            return Err(SandboxError::RootNotDirectory(root.to_owned()));
        }
        let canonical_root =
            Utf8PathBuf::from_path_buf(canonical).map_err(SandboxError::NonUtf8Path)?;
        manifest.validate_for_root(&canonical_root)?;
        let id = SandboxId::from_root(name, &canonical_root);
        let bind_mounts = vec![BindMount {
            source: canonical_root.clone(),
            target: Utf8PathBuf::from(WORKSPACE_TARGET),
            writable: true,
        }];
        Ok(Self {
            id,
            canonical_root,
            manifest,
            bind_mounts,
        })
    }

    pub fn id(&self) -> &SandboxId {
        &self.id
    }

    pub fn canonical_root(&self) -> &Utf8Path {
        &self.canonical_root
    }

    pub const fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn bind_mounts(&self) -> &[BindMount] {
        &self.bind_mounts
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SandboxError {
    #[error("sandbox root is not a directory: {0}")]
    RootNotDirectory(Utf8PathBuf),
    #[error("canonical sandbox root is not valid UTF-8: {0:?}")]
    NonUtf8Path(std::path::PathBuf),
    #[error("could not canonicalize {path}: {source}")]
    Io {
        path: Utf8PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    InvalidManifest(#[from] crate::manifest::ManifestError),
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("invalid sandbox ID: {message}")]
pub struct SandboxIdError {
    message: String,
}

fn validate_sandbox_id(value: &str) -> Result<(), SandboxIdError> {
    const DIGEST_LENGTH: usize = 12;
    let Some(prefix_length) = value.len().checked_sub(DIGEST_LENGTH + 1) else {
        return Err(invalid_id("expected a slug and 12-character digest"));
    };
    let (slug_and_separator, digest) = value.split_at(prefix_length + 1);
    let Some(slug) = slug_and_separator.strip_suffix('-') else {
        return Err(invalid_id("missing digest separator"));
    };
    let valid_slug = !slug.is_empty()
        && slug
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !slug.starts_with('-')
        && !slug.ends_with('-')
        && !slug.contains("--");
    if !valid_slug {
        return Err(invalid_id(
            "slug must use lowercase letters, digits, and single hyphens",
        ));
    }
    if digest.len() != DIGEST_LENGTH
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid_id(
            "digest must be exactly 12 lowercase hexadecimal characters",
        ));
    }
    Ok(())
}

fn invalid_id(message: &str) -> SandboxIdError {
    SandboxIdError {
        message: message.to_owned(),
    }
}

fn slugify(name: &str) -> String {
    let (slug, _) = name.chars().fold(
        (String::new(), false),
        |(mut output, pending), character| {
            if character.is_ascii_alphanumeric() {
                if pending && !output.is_empty() {
                    output.push('-');
                }
                output.push(character.to_ascii_lowercase());
                (output, false)
            } else {
                let has_output = !output.is_empty();
                (output, has_output)
            }
        },
    );
    if slug.is_empty() {
        "sandbox".to_owned()
    } else {
        slug
    }
}
