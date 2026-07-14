use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeVersion {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl RuntimeVersion {
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkIsolation {
    Proven,
    Unsupported,
    Unverified,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeCapabilities {
    pub version: RuntimeVersion,
    pub bind_mounts: bool,
    pub named_volumes: bool,
    pub tty: bool,
    pub signals: bool,
    pub loopback_publish: bool,
    pub resource_limits: bool,
    pub offline: NetworkIsolation,
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RuntimeError {
    #[error("{operation}: {message}")]
    CommandIo { operation: String, message: String },
    #[error("{operation} failed with exit code {exit_code:?}: {stderr}")]
    CommandFailed {
        operation: String,
        exit_code: Option<i32>,
        stderr: String,
    },
    #[error("invalid output from {operation}: {message}")]
    InvalidOutput { operation: String, message: String },
    #[error("unsupported runtime version {found:?}; supported versions: {supported}")]
    UnsupportedVersion {
        found: RuntimeVersion,
        supported: String,
    },
    #[error("unsupported capability: {capability}")]
    UnsupportedCapability { capability: String },
    #[error("resource ownership mismatch: {resource}")]
    OwnershipMismatch { resource: String },
    #[error("resource conflict for {resource}: {message}")]
    Conflict { resource: String, message: String },
}
