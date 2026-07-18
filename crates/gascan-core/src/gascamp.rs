use std::path::Component;

use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;
use thiserror::Error;

pub const BUNDLED_GASCAMP_REVISION: &str = "f6b248c5926240856dbea83d1d2c5c90ea1c1456";

const WORKSPACE_GASCAMP_ROOT: &str = "/workspace/gascamp";

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum GascampSource {
    Bundled { revision: &'static str },
    Workspace { path: Utf8PathBuf },
}

impl GascampSource {
    pub const fn trusted(&self) -> bool {
        matches!(self, Self::Bundled { .. })
    }
}

#[derive(Debug, Error)]
#[error("Gascamp source must be bundled or remain beneath {WORKSPACE_GASCAMP_ROOT}: {path}")]
pub struct GascampSourceError {
    path: String,
}

pub fn resolve_gascamp(source: &str) -> Result<GascampSource, GascampSourceError> {
    if source == "bundled" {
        return Ok(GascampSource::Bundled {
            revision: BUNDLED_GASCAMP_REVISION,
        });
    }

    let path = normalize_absolute(source).ok_or_else(|| GascampSourceError {
        path: source.to_owned(),
    })?;
    let allowed = Utf8Path::new(WORKSPACE_GASCAMP_ROOT);
    if !path.starts_with(allowed) {
        return Err(GascampSourceError {
            path: source.to_owned(),
        });
    }

    Ok(GascampSource::Workspace { path })
}

fn normalize_absolute(source: &str) -> Option<Utf8PathBuf> {
    let source = Utf8Path::new(source);
    if !source.is_absolute() {
        return None;
    }

    let mut normalized = Utf8PathBuf::from("/");
    for component in source.as_std_path().components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(component) => normalized.push(component.to_str()?),
            Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
            Component::Prefix(_) => return None,
        }
    }
    Some(normalized)
}
