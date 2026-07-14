use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::path::Component;
use thiserror::Error;

const MANIFEST_FILE: &str = "gascan.toml";

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Manifest {
    pub version: u32,
    pub name: Option<String>,
    pub network: NetworkMode,
    pub user: UserMode,
    pub gascamp: GascampSource,
    pub setup: Option<Utf8PathBuf>,
    pub resources: Resources,
    pub tools: BTreeMap<String, String>,
    pub ports: BTreeMap<String, u16>,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            version: 1,
            name: None,
            network: NetworkMode::Offline,
            user: UserMode::Workspace,
            gascamp: GascampSource::Bundled,
            setup: None,
            resources: Resources::default(),
            tools: BTreeMap::new(),
            ports: BTreeMap::new(),
        }
    }
}

impl Manifest {
    pub fn parse(source: &str) -> Result<Self, ManifestError> {
        let raw: RawManifest = toml::from_str(source)?;
        raw.validate()
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
            Ok(source) => Self::parse(&source),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ManifestError::Io { path, source }),
        }
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

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub enum GascampSource {
    #[default]
    Bundled,
    Workspace(Utf8PathBuf),
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Resources {
    pub cpus: Option<u16>,
    pub memory: Option<ResourceSize>,
    pub disk: Option<ResourceSize>,
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
    #[error("path is not valid UTF-8: {0:?}")]
    NonUtf8Path(std::path::PathBuf),
    #[error("could not access {path}: {source}")]
    Io {
        path: Utf8PathBuf,
        #[source]
        source: std::io::Error,
    },
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
    resources: Resources,
    #[serde(default)]
    tools: BTreeMap<String, String>,
    #[serde(default)]
    ports: BTreeMap<String, u16>,
}

impl RawManifest {
    fn validate(self) -> Result<Manifest, ManifestError> {
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
        if let Some(setup) = self.setup.as_deref() {
            validate_workspace_relative_path("setup", setup)?;
        }
        let gascamp = match self.gascamp.as_deref() {
            None | Some("bundled") => GascampSource::Bundled,
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
                GascampSource::Workspace(path)
            }
        };
        Ok(Manifest {
            version: self.version,
            name: self.name,
            network: self.network,
            user: self.user,
            gascamp,
            setup: self.setup,
            resources: self.resources,
            tools: self.tools,
            ports: self.ports,
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
