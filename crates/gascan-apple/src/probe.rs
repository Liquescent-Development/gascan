use gascan_core::runtime::{NetworkIsolation, RuntimeCapabilities, RuntimeError, RuntimeVersion};
use serde_json::Value;

use crate::{CommandRunner, CommandSpec};

const VERSION_OPERATION: &str = "container system version";
const STATUS_OPERATION: &str = "container system status";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppleSystemStatus {
    pub app_root: String,
    pub api_server_version: RuntimeVersion,
}

/// Probes the installed Apple Container runtime through its structured CLI output.
pub struct AppleProbe<R> {
    runner: R,
}

impl<R> AppleProbe<R> {
    /// Creates a probe backed by an injectable command runner.
    pub const fn new(runner: R) -> Self {
        Self { runner }
    }
}

impl<R: CommandRunner> AppleProbe<R> {
    pub async fn status(&self) -> Result<AppleSystemStatus, RuntimeError> {
        let output = self
            .runner
            .run(CommandSpec::new(
                "container",
                ["system", "status", "--format", "json"],
            ))
            .await?;
        let value: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
            RuntimeError::InvalidOutput {
                operation: STATUS_OPERATION.to_owned(),
                message: error.to_string(),
            }
        })?;
        let field = |name: &str| {
            value
                .get(name)
                .and_then(Value::as_str)
                .ok_or_else(|| RuntimeError::InvalidOutput {
                    operation: STATUS_OPERATION.to_owned(),
                    message: format!("missing string field {name}"),
                })
        };
        if field("status")? != "running" {
            return Err(RuntimeError::InvalidState {
                resource: "Apple container service".to_owned(),
                message: "service is not running".to_owned(),
            });
        }
        if field("apiServerAppName")? != "container-apiserver"
            || field("apiServerBuild")? != "release"
            || field("apiServerCommit")?.is_empty()
        {
            return Err(RuntimeError::InvalidOutput {
                operation: STATUS_OPERATION.to_owned(),
                message: "unsupported service identity schema".to_owned(),
            });
        }
        let raw_version = field("apiServerVersion")?;
        let version = raw_version
            .strip_prefix("container-apiserver version ")
            .and_then(|value| {
                value
                    .split_once(" (build: release, commit: ")
                    .map(|(version, _)| version)
            })
            .ok_or_else(|| RuntimeError::InvalidOutput {
                operation: STATUS_OPERATION.to_owned(),
                message: "unsupported apiServerVersion schema".to_owned(),
            })?;
        let api_server_version = parse_version(version)?;
        let app_root = field("appRoot")?.to_owned();
        if !app_root.starts_with('/') || app_root.is_empty() {
            return Err(RuntimeError::InvalidOutput {
                operation: STATUS_OPERATION.to_owned(),
                message: "appRoot must be absolute".to_owned(),
            });
        }
        Ok(AppleSystemStatus {
            app_root,
            api_server_version,
        })
    }

    /// Returns the supported Apple Container application version.
    pub async fn version(&self) -> Result<RuntimeVersion, RuntimeError> {
        let output = self
            .runner
            .run(CommandSpec::new(
                "container",
                ["system", "version", "--format", "json"],
            ))
            .await?;
        let entries: Vec<Value> = serde_json::from_slice(&output.stdout).map_err(invalid_output)?;
        let mut containers = entries
            .iter()
            .filter(|entry| entry.get("appName").and_then(Value::as_str) == Some("container"));
        let entry = containers
            .next()
            .ok_or_else(|| RuntimeError::InvalidOutput {
                operation: VERSION_OPERATION.to_owned(),
                message: "missing container version entry".to_owned(),
            })?;
        if containers.next().is_some() {
            return Err(RuntimeError::InvalidOutput {
                operation: VERSION_OPERATION.to_owned(),
                message: "duplicate container version entry".to_owned(),
            });
        }

        let version_value = entry
            .get("version")
            .and_then(Value::as_str)
            .ok_or_else(|| RuntimeError::InvalidOutput {
                operation: VERSION_OPERATION.to_owned(),
                message: "container version must be a string".to_owned(),
            })?;
        if entry.get("buildType").and_then(Value::as_str) != Some("release")
            || entry
                .get("commit")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
        {
            return Err(RuntimeError::InvalidOutput {
                operation: VERSION_OPERATION.to_owned(),
                message: "container version entry requires release buildType and non-empty commit"
                    .to_owned(),
            });
        }
        let version = parse_version(version_value)?;
        if version.major != 1 {
            return Err(RuntimeError::UnsupportedVersion {
                found: version,
                supported: "major version 1".to_owned(),
            });
        }
        Ok(version)
    }

    /// Returns the conservative capability baseline for a supported runtime.
    pub async fn base_capabilities(&self) -> Result<RuntimeCapabilities, RuntimeError> {
        let version = self.version().await?;
        let supported = version == RuntimeVersion::new(1, 1, 0);
        Ok(RuntimeCapabilities {
            offline: if supported {
                NetworkIsolation::Proven
            } else {
                NetworkIsolation::Unsupported
            },
            version,
            bind_mounts: supported,
            named_volumes: supported,
            tty: supported,
            signals: supported,
            loopback_publish: supported,
            resource_limits: supported,
        })
    }
}

/// Constructs the Apple no-network arguments only after live isolation proof.
/// Mount construction is deliberately sequenced after this fail-closed gate.
pub fn offline_network_args(
    capability: NetworkIsolation,
    construct_mount: impl FnOnce(),
) -> Result<Vec<String>, RuntimeError> {
    if capability != NetworkIsolation::Proven {
        return Err(RuntimeError::UnsupportedCapability {
            capability: "hard offline networking".to_owned(),
        });
    }
    construct_mount();
    Ok(vec!["--network".to_owned(), "none".to_owned()])
}

fn parse_version(value: &str) -> Result<RuntimeVersion, RuntimeError> {
    let parts: Vec<_> = value.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|part| !valid_component(part)) {
        return Err(RuntimeError::InvalidOutput {
            operation: VERSION_OPERATION.to_owned(),
            message: format!("container version is not plain semantic version: {value:?}"),
        });
    }

    let component = |index: usize| {
        parts[index]
            .parse::<u64>()
            .map_err(|error| RuntimeError::InvalidOutput {
                operation: VERSION_OPERATION.to_owned(),
                message: format!("invalid container version component: {error}"),
            })
    };
    Ok(RuntimeVersion::new(
        component(0)?,
        component(1)?,
        component(2)?,
    ))
}

fn valid_component(component: &str) -> bool {
    !component.is_empty()
        && component.bytes().all(|byte| byte.is_ascii_digit())
        && (component == "0" || !component.starts_with('0'))
}

fn invalid_output(error: serde_json::Error) -> RuntimeError {
    RuntimeError::InvalidOutput {
        operation: VERSION_OPERATION.to_owned(),
        message: error.to_string(),
    }
}
