use crate::manifest::Manifest;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppliedState {
    tool_hash: Option<String>,
}

impl AppliedState {
    pub const fn empty() -> Self {
        Self { tool_hash: None }
    }

    pub fn with_tool_hash(hash: impl Into<String>) -> Self {
        Self {
            tool_hash: Some(hash.into()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProvisionStep {
    WriteSafeMiseConfig,
    InstallTools,
    RunSetup,
    VerifyGascamp,
    HealthCheck,
}

impl ProvisionStep {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WriteSafeMiseConfig => "write_safe_mise_config",
            Self::InstallTools => "install_tools",
            Self::RunSetup => "run_setup",
            Self::VerifyGascamp => "verify_gascamp",
            Self::HealthCheck => "health_check",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProvisionPlan {
    steps: Vec<ProvisionStep>,
    desired_tools: BTreeMap<String, String>,
    desired_tool_hash: String,
    tools_changed: bool,
}

impl ProvisionPlan {
    pub fn steps(&self) -> &[ProvisionStep] {
        &self.steps
    }

    pub fn desired_tool_hash(&self) -> &str {
        &self.desired_tool_hash
    }

    pub const fn tools_changed(&self) -> bool {
        self.tools_changed
    }

    pub fn safe_mise_toml(&self) -> Result<Option<String>, ProvisionError> {
        if !self.tools_changed {
            return Ok(None);
        }
        #[derive(Serialize)]
        struct SafeMiseConfig<'a> {
            tools: &'a BTreeMap<String, String>,
        }
        toml::to_string(&SafeMiseConfig {
            tools: &self.desired_tools,
        })
        .map(Some)
        .map_err(ProvisionError::SerializeConfig)
    }
}

pub struct ProvisioningPlanner;

impl ProvisioningPlanner {
    pub fn plan(
        manifest: &Manifest,
        applied: &AppliedState,
    ) -> Result<ProvisionPlan, ProvisionError> {
        let desired_tools = manifest.tools().clone();
        let serialized = serde_json::to_vec(&desired_tools).map_err(ProvisionError::HashTools)?;
        let desired_tool_hash = format!("sha256:{:x}", Sha256::digest(serialized));
        let tools_changed = match applied.tool_hash.as_deref() {
            Some(applied_hash) => applied_hash != desired_tool_hash,
            None => !desired_tools.is_empty(),
        };
        let mut steps = Vec::new();
        if tools_changed {
            steps.push(ProvisionStep::WriteSafeMiseConfig);
            steps.push(ProvisionStep::InstallTools);
        }
        if manifest.setup().is_some() {
            steps.push(ProvisionStep::RunSetup);
        }
        steps.push(ProvisionStep::VerifyGascamp);
        steps.push(ProvisionStep::HealthCheck);
        Ok(ProvisionPlan {
            steps,
            desired_tools,
            desired_tool_hash,
            tools_changed,
        })
    }
}

#[derive(Debug, Error)]
pub enum ProvisionError {
    #[error("failed to hash desired tools: {0}")]
    HashTools(serde_json::Error),
    #[error("failed to serialize safe mise configuration: {0}")]
    SerializeConfig(toml::ser::Error),
}
