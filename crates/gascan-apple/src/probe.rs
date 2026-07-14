use gascan_core::runtime::{NetworkIsolation, RuntimeCapabilities, RuntimeError, RuntimeVersion};
use serde_json::Value;

use crate::{CommandRunner, CommandSpec};

const VERSION_OPERATION: &str = "container system version";

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
        Ok(RuntimeCapabilities {
            version: self.version().await?,
            bind_mounts: false,
            named_volumes: false,
            tty: false,
            signals: false,
            loopback_publish: false,
            resource_limits: false,
            offline: NetworkIsolation::Unverified,
        })
    }
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
