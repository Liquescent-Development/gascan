use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::path::Component;
use thiserror::Error;

const MANIFEST_FILE: &str = "gascan.toml";

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Manifest {
    version: u32,
    name: Option<String>,
    network: NetworkMode,
    user: UserMode,
    gascamp: GascampSource,
    setup: Option<Utf8PathBuf>,
    resources: Resources,
    tools: BTreeMap<String, String>,
    ports: BTreeMap<String, u16>,
    #[serde(skip)]
    canonical_root: Utf8PathBuf,
}

impl Manifest {
    fn parse(source: &str, canonical_root: &Utf8Path) -> Result<Self, ManifestError> {
        let raw: RawManifest = toml::from_str(source)?;
        raw.validate(canonical_root)
    }

    pub fn load(root: &Utf8Path) -> Result<Self, ManifestError> {
        let canonical = std::fs::canonicalize(root).map_err(|source| ManifestError::Io {
            path: root.to_owned(),
            source,
        })?;
        if !canonical.is_dir() {
            return Err(ManifestError::RootNotDirectory(root.to_owned()));
        }
        let canonical =
            Utf8PathBuf::from_path_buf(canonical).map_err(ManifestError::NonUtf8Path)?;
        let path = canonical.join(MANIFEST_FILE);
        match std::fs::read_to_string(&path) {
            Ok(source) => Self::parse(&source, &canonical),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                Ok(Self::defaults_for_root(canonical))
            }
            Err(source) => Err(ManifestError::ManifestUnreadable { path, source }),
        }
    }

    fn defaults_for_root(canonical_root: Utf8PathBuf) -> Self {
        Self {
            version: 1,
            name: None,
            network: NetworkMode::Offline,
            user: UserMode::Workspace,
            gascamp: GascampSource::bundled(),
            setup: None,
            resources: Resources::empty(),
            tools: BTreeMap::new(),
            ports: BTreeMap::new(),
            canonical_root,
        }
    }

    pub(crate) fn validate_for_root(&self, canonical_root: &Utf8Path) -> Result<(), ManifestError> {
        if self.canonical_root != canonical_root {
            return Err(ManifestError::RootMismatch {
                loaded: self.canonical_root.clone(),
                requested: canonical_root.to_owned(),
            });
        }
        if let Some(setup) = self.setup.as_deref() {
            validate_setup_containment(canonical_root, setup)?;
        }
        Ok(())
    }

    pub const fn version(&self) -> u32 {
        self.version
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub const fn network(&self) -> NetworkMode {
        self.network
    }

    pub const fn user(&self) -> UserMode {
        self.user
    }

    pub const fn gascamp(&self) -> &GascampSource {
        &self.gascamp
    }

    pub fn setup(&self) -> Option<&Utf8Path> {
        self.setup.as_deref()
    }

    pub const fn resources(&self) -> &Resources {
        &self.resources
    }

    pub const fn tools(&self) -> &BTreeMap<String, String> {
        &self.tools
    }

    pub const fn ports(&self) -> &BTreeMap<String, u16> {
        &self.ports
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMode {
    Networked,
    #[default]
    Offline,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UserMode {
    #[default]
    Workspace,
    Root,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GascampSource(GascampSourceKind);

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
enum GascampSourceKind {
    Bundled,
    Workspace(Utf8PathBuf),
}

impl GascampSource {
    fn bundled() -> Self {
        Self(GascampSourceKind::Bundled)
    }

    fn workspace(path: Utf8PathBuf) -> Self {
        Self(GascampSourceKind::Workspace(path))
    }

    pub const fn is_bundled(&self) -> bool {
        matches!(self.0, GascampSourceKind::Bundled)
    }

    pub fn workspace_path(&self) -> Option<&Utf8Path> {
        match &self.0 {
            GascampSourceKind::Bundled => None,
            GascampSourceKind::Workspace(path) => Some(path),
        }
    }
}

/// Validated resource policy loaded as part of a [`Manifest`].
///
/// Resource policy cannot be deserialized independently of the manifest's
/// validation boundary.
///
/// ```compile_fail
/// use gascan_core::manifest::Resources;
///
/// let _: Resources = toml::from_str("cpus = 0")?;
/// # Ok::<(), toml::de::Error>(())
/// ```
///
/// ```compile_fail
/// use gascan_core::manifest::Resources;
///
/// let _ = Resources::default();
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Resources {
    cpus: Option<u16>,
    memory: Option<ResourceSize>,
    disk: Option<ResourceSize>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawResources {
    cpus: Option<u16>,
    memory: Option<ResourceSize>,
    disk: Option<ResourceSize>,
}

impl Resources {
    const fn empty() -> Self {
        Self {
            cpus: None,
            memory: None,
            disk: None,
        }
    }

    pub const fn cpus(&self) -> Option<u16> {
        self.cpus
    }

    pub const fn memory(&self) -> Option<ResourceSize> {
        self.memory
    }

    pub const fn disk(&self) -> Option<ResourceSize> {
        self.disk
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ResourceSize(u64);

impl ResourceSize {
    pub const fn bytes(self) -> u64 {
        self.0
    }

    fn parse(value: &str) -> Result<Self, String> {
        let (number, multiplier) = [
            ("KiB", 1024_u64),
            ("MiB", 1024_u64.pow(2)),
            ("GiB", 1024_u64.pow(3)),
            ("TiB", 1024_u64.pow(4)),
        ]
        .into_iter()
        .find_map(|(suffix, multiplier)| value.strip_suffix(suffix).map(|n| (n, multiplier)))
        .ok_or_else(|| "resource size must use KiB, MiB, GiB, or TiB units".to_owned())?;
        if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("resource size must be a positive integer with binary units".to_owned());
        }
        let number = number
            .parse::<u64>()
            .map_err(|error| format!("invalid resource size: {error}"))?;
        if number == 0 {
            return Err("resource size must be greater than zero".to_owned());
        }
        number
            .checked_mul(multiplier)
            .map(Self)
            .ok_or_else(|| "resource size exceeds the supported range".to_owned())
    }
}

impl<'de> Deserialize<'de> for ResourceSize {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ManifestError {
    #[error("invalid gascan.toml: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("unsupported manifest version {found}; expected version 1")]
    UnsupportedVersion { found: u32 },
    #[error("invalid manifest: {0}")]
    Invalid(String),
    #[error("manifest root is not a directory: {0}")]
    RootNotDirectory(Utf8PathBuf),
    #[error("setup path {setup} resolves outside the workspace root {root}")]
    SetupOutsideRoot {
        setup: Utf8PathBuf,
        root: Utf8PathBuf,
    },
    #[error("manifest was loaded for a different root: loaded {loaded}, requested {requested}")]
    RootMismatch {
        loaded: Utf8PathBuf,
        requested: Utf8PathBuf,
    },
    #[error("path is not valid UTF-8: {0:?}")]
    NonUtf8Path(std::path::PathBuf),
    #[error("could not access {path}: {source}")]
    Io {
        path: Utf8PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not read {path}: {source}")]
    ManifestUnreadable {
        path: Utf8PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl ManifestError {
    /// Whether this failure is about the project root itself (missing, not a
    /// directory, unreadable) rather than the manifest's content.
    ///
    /// `ManifestError` is `#[non_exhaustive]`, so callers outside this crate
    /// cannot match its variants exhaustively. This match lives here, inside
    /// the defining crate, so adding a variant without classifying it here
    /// fails to compile instead of silently reaching a default.
    pub fn is_project_root_error(&self) -> bool {
        match self {
            Self::Io { .. } | Self::RootNotDirectory(_) | Self::NonUtf8Path(_) => true,
            Self::Parse(_)
            | Self::UnsupportedVersion { .. }
            | Self::Invalid(_)
            | Self::SetupOutsideRoot { .. }
            | Self::RootMismatch { .. }
            | Self::ManifestUnreadable { .. } => false,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    version: u32,
    name: Option<String>,
    #[serde(default)]
    network: NetworkMode,
    #[serde(default)]
    user: UserMode,
    gascamp: Option<String>,
    setup: Option<Utf8PathBuf>,
    #[serde(default)]
    resources: RawResources,
    #[serde(default)]
    tools: BTreeMap<String, String>,
    #[serde(default)]
    ports: BTreeMap<String, u16>,
}

impl RawManifest {
    fn validate(self, canonical_root: &Utf8Path) -> Result<Manifest, ManifestError> {
        if self.version != 1 {
            return Err(ManifestError::UnsupportedVersion {
                found: self.version,
            });
        }
        if self.resources.cpus == Some(0) {
            return Err(ManifestError::Invalid(
                "resources.cpus must be greater than zero".to_owned(),
            ));
        }
        let resources = Resources {
            cpus: self.resources.cpus,
            memory: self.resources.memory,
            disk: self.resources.disk,
        };
        if let Some(setup) = self.setup.as_deref() {
            validate_workspace_relative_path("setup", setup)?;
            validate_setup_containment(canonical_root, setup)?;
        }
        let gascamp = match self.gascamp.as_deref() {
            None | Some("bundled") => GascampSource::bundled(),
            Some(value) => {
                let path = Utf8PathBuf::from(value);
                let allowed = Utf8Path::new("/workspace/gascamp");
                if !path.starts_with(allowed)
                    || path
                        .components()
                        .any(|component| component.as_str() == "..")
                {
                    return Err(ManifestError::Invalid(format!(
                        "gascamp workspace path must be beneath {allowed}"
                    )));
                }
                GascampSource::workspace(path)
            }
        };
        Ok(Manifest {
            version: self.version,
            name: self.name,
            network: self.network,
            user: self.user,
            gascamp,
            setup: self.setup,
            resources,
            tools: self.tools,
            ports: self.ports,
            canonical_root: canonical_root.to_owned(),
        })
    }
}

fn validate_workspace_relative_path(label: &str, path: &Utf8Path) -> Result<(), ManifestError> {
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                camino::Utf8Component::ParentDir | camino::Utf8Component::RootDir
            )
        })
    {
        return Err(ManifestError::Invalid(format!(
            "{label} path must remain beneath the workspace root"
        )));
    }
    if path.as_std_path().components().any(|component| {
        matches!(
            component,
            Component::Prefix(_) | Component::RootDir | Component::ParentDir
        )
    }) {
        return Err(ManifestError::Invalid(format!(
            "{label} path must remain beneath the workspace root"
        )));
    }
    Ok(())
}

fn validate_setup_containment(
    canonical_root: &Utf8Path,
    setup: &Utf8Path,
) -> Result<(), ManifestError> {
    let mut candidate = canonical_root.join(setup);
    loop {
        match std::fs::symlink_metadata(&candidate) {
            Ok(_) => {
                let resolved =
                    std::fs::canonicalize(&candidate).map_err(|source| ManifestError::Io {
                        path: candidate.clone(),
                        source,
                    })?;
                let resolved =
                    Utf8PathBuf::from_path_buf(resolved).map_err(ManifestError::NonUtf8Path)?;
                if !resolved.starts_with(canonical_root) {
                    return Err(ManifestError::SetupOutsideRoot {
                        setup: setup.to_owned(),
                        root: canonical_root.to_owned(),
                    });
                }
                return Ok(());
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                if !candidate.pop() {
                    return Err(ManifestError::Io {
                        path: canonical_root.join(setup),
                        source,
                    });
                }
            }
            Err(source) => {
                return Err(ManifestError::Io {
                    path: candidate,
                    source,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_project_root_error_classifies_every_variant() -> Result<(), Box<dyn std::error::Error>> {
        let path = Utf8PathBuf::from("/root");
        let parse_error = match toml::from_str::<RawManifest>("version = \"not-a-number\"\n") {
            Ok(_) => return Err("a non-numeric version must fail to parse".into()),
            Err(error) => error,
        };
        let cases: Vec<(ManifestError, bool)> = vec![
            (ManifestError::Parse(parse_error), false),
            (ManifestError::UnsupportedVersion { found: 2 }, false),
            (ManifestError::Invalid("bad".to_owned()), false),
            (ManifestError::RootNotDirectory(path.clone()), true),
            (
                ManifestError::SetupOutsideRoot {
                    setup: path.clone(),
                    root: path.clone(),
                },
                false,
            ),
            (
                ManifestError::RootMismatch {
                    loaded: path.clone(),
                    requested: path.clone(),
                },
                false,
            ),
            (
                ManifestError::NonUtf8Path(std::path::PathBuf::from("/root")),
                true,
            ),
            (
                ManifestError::Io {
                    path: path.clone(),
                    source: std::io::Error::from(std::io::ErrorKind::NotFound),
                },
                true,
            ),
            (
                ManifestError::ManifestUnreadable {
                    path,
                    source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
                },
                false,
            ),
        ];

        for (error, expected) in cases {
            assert_eq!(
                error.is_project_root_error(),
                expected,
                "wrong classification for {error:?}"
            );
        }
        Ok(())
    }
}
