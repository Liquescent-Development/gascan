mod host;
mod prompt;

pub(crate) use host::SystemHostDiscovery;
pub(crate) use prompt::TerminalPrompter;

use crate::guest::Secret;

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct GitDefaults {
    pub(crate) name: Option<String>,
    pub(crate) email: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct HostAccount {
    pub(crate) hostname: String,
    pub(crate) login: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Forge {
    GitHub,
    GitLab,
}

pub(crate) trait HostDiscovery {
    fn git_defaults(&self) -> Result<GitDefaults, ConfigureError>;
    fn accounts(&self, forge: Forge) -> Result<Vec<HostAccount>, ConfigureError>;
    fn token(&self, forge: Forge, account: &HostAccount) -> Result<Secret, ConfigureError>;
}

pub(crate) trait Prompter {
    fn confirm(&mut self, prompt: &str, default: bool) -> Result<bool, ConfigureError>;
    fn line(
        &mut self,
        prompt: &str,
        default: Option<&str>,
    ) -> Result<Option<String>, ConfigureError>;
    fn secret(&mut self, prompt: &str) -> Result<Option<Secret>, ConfigureError>;
}

#[derive(Debug)]
pub(crate) enum ConfigureError {
    Cancelled,
    Io(std::io::Error),
    HostCommand {
        category: &'static str,
        message: String,
    },
    GuestCommand {
        category: &'static str,
        message: String,
    },
    InvalidOutput {
        category: &'static str,
    },
    UnsafeState {
        path: String,
        remedy: String,
    },
}

impl std::fmt::Display for ConfigureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("configuration cancelled"),
            Self::Io(error) => write!(formatter, "configuration I/O failed: {error}"),
            Self::HostCommand { category, message } => {
                write!(formatter, "host {category} failed: {message}")
            }
            Self::GuestCommand { category, message } => {
                write!(formatter, "guest {category} failed: {message}")
            }
            Self::InvalidOutput { category } => {
                write!(formatter, "{category} returned invalid output")
            }
            Self::UnsafeState { path, remedy } => {
                write!(formatter, "unsafe state at {path}: {remedy}")
            }
        }
    }
}

impl std::error::Error for ConfigureError {}

impl From<std::io::Error> for ConfigureError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[cfg(test)]
mod tests;
