use super::{ConfigureError, Forge, GitDefaults, HostAccount, HostDiscovery};
use crate::guest::Secret;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};

const MAX_HOST_OUTPUT_BYTES: usize = 1024 * 1024;
const FORGE_TOKEN_ENVIRONMENT: [&str; 7] = [
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
    "GITLAB_TOKEN",
    "GITLAB_ACCESS_TOKEN",
    "OAUTH_TOKEN",
];

#[derive(Default)]
pub(crate) struct SystemHostDiscovery {
    program_directory: Option<PathBuf>,
}

impl SystemHostDiscovery {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(super) fn with_program_directory(program_directory: PathBuf) -> Self {
        Self {
            program_directory: Some(program_directory),
        }
    }

    fn program(&self, name: &'static str) -> PathBuf {
        self.program_directory
            .as_ref()
            .map_or_else(|| PathBuf::from(name), |directory| directory.join(name))
    }

    fn run(
        &self,
        program: &'static str,
        arguments: &[&OsStr],
        category: &'static str,
    ) -> Result<CommandOutput, ConfigureError> {
        run_bounded(&self.program(program), arguments, category)
    }
}

impl HostDiscovery for SystemHostDiscovery {
    fn git_defaults(&self) -> Result<GitDefaults, ConfigureError> {
        Ok(GitDefaults {
            name: self.git_global_value("user.name")?,
            email: self.git_global_value("user.email")?,
        })
    }

    fn accounts(&self, forge: Forge) -> Result<Vec<HostAccount>, ConfigureError> {
        match forge {
            Forge::GitHub => {
                let output = self.run(
                    "gh",
                    &os_arguments(&["auth", "status", "--json", "hosts"]),
                    "GitHub account discovery",
                )?;
                let accounts = parse_github_accounts(&output.stdout)?;
                if !output.status.success() && !accounts.is_empty() {
                    return Err(host_failure("GitHub account discovery"));
                }
                Ok(accounts)
            }
            Forge::GitLab => {
                let output = self.run(
                    "glab",
                    &os_arguments(&["auth", "status", "--all"]),
                    "GitLab account discovery",
                )?;
                parse_gitlab_accounts(&output.stdout, output.status.success())
            }
        }
    }

    fn token(&self, forge: Forge, account: &HostAccount) -> Result<Secret, ConfigureError> {
        if !valid_hostname(&account.hostname) {
            return Err(ConfigureError::InvalidOutput {
                category: "host account",
            });
        }
        let hostname = OsStr::new(&account.hostname);
        let output = match forge {
            Forge::GitHub => self.run(
                "gh",
                &[
                    OsStr::new("auth"),
                    OsStr::new("token"),
                    OsStr::new("--hostname"),
                    hostname,
                ],
                "GitHub token retrieval",
            )?,
            Forge::GitLab => self.run(
                "glab",
                &[
                    OsStr::new("config"),
                    OsStr::new("get"),
                    OsStr::new("token"),
                    OsStr::new("--global"),
                    OsStr::new("--host"),
                    hostname,
                ],
                "GitLab token retrieval",
            )?,
        };
        if !output.status.success() {
            return Err(host_failure(match forge {
                Forge::GitHub => "GitHub token retrieval",
                Forge::GitLab => "GitLab token retrieval",
            }));
        }
        let mut token = output.stdout;
        trim_one_line_ending(&mut token);
        if token.is_empty() {
            return Err(ConfigureError::InvalidOutput {
                category: "forge token",
            });
        }
        Ok(Secret::new(token))
    }
}

impl SystemHostDiscovery {
    fn git_global_value(&self, key: &'static str) -> Result<Option<String>, ConfigureError> {
        let output = self.run(
            "git",
            &[
                OsStr::new("config"),
                OsStr::new("--global"),
                OsStr::new("--get"),
                OsStr::new(key),
            ],
            "Git global configuration",
        )?;
        if output.status.success() {
            let mut bytes = output.stdout;
            trim_one_line_ending(&mut bytes);
            let value = String::from_utf8(bytes).map_err(|_| ConfigureError::InvalidOutput {
                category: "Git global configuration",
            })?;
            return Ok((!value.is_empty()).then_some(value));
        }
        if output.status.code() == Some(1) {
            return Ok(None);
        }
        Err(host_failure("Git global configuration"))
    }
}

fn os_arguments<'a>(arguments: &'a [&'a str]) -> Vec<&'a OsStr> {
    arguments.iter().map(OsStr::new).collect()
}

fn host_failure(category: &'static str) -> ConfigureError {
    ConfigureError::HostCommand {
        category,
        message: "command did not complete successfully".to_owned(),
    }
}

struct CommandOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
}

struct Capture {
    bytes: Vec<u8>,
    exceeded: bool,
}

fn run_bounded(
    program: &Path,
    arguments: &[&OsStr],
    category: &'static str,
) -> Result<CommandOutput, ConfigureError> {
    let mut command = std::process::Command::new(program);
    command
        .args(arguments)
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for name in FORGE_TOKEN_ENVIRONMENT {
        command.env_remove(name);
    }
    let mut child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ConfigureError::HostCommand {
                category,
                message: "command is not available".to_owned(),
            }
        } else {
            ConfigureError::Io(error)
        }
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ConfigureError::HostCommand {
            category,
            message: "stdout capture was unavailable".to_owned(),
        })?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ConfigureError::HostCommand {
            category,
            message: "stderr capture was unavailable".to_owned(),
        })?;
    let stdout_reader = std::thread::spawn(move || capture_bounded(stdout));
    let stderr_reader = std::thread::spawn(move || capture_bounded(stderr));
    let status = child.wait()?;
    let stdout = stdout_reader
        .join()
        .map_err(|_| std::io::Error::other("host stdout reader stopped"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| std::io::Error::other("host stderr reader stopped"))??;
    if stdout.exceeded || stderr.exceeded {
        return Err(ConfigureError::InvalidOutput { category });
    }
    Ok(CommandOutput {
        status,
        stdout: stdout.bytes,
    })
}

fn capture_bounded(mut reader: impl std::io::Read) -> std::io::Result<Capture> {
    let mut bytes = Vec::new();
    let mut exceeded = false;
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let remaining = MAX_HOST_OUTPUT_BYTES.saturating_sub(bytes.len());
        let retained = remaining.min(count);
        bytes.extend_from_slice(&buffer[..retained]);
        exceeded |= retained < count;
    }
    Ok(Capture { bytes, exceeded })
}

#[derive(Deserialize)]
struct GitHubStatus {
    hosts: BTreeMap<String, Vec<GitHubAccount>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GitHubAccount {
    active: bool,
    host: String,
    login: String,
}

fn parse_github_accounts(bytes: &[u8]) -> Result<Vec<HostAccount>, ConfigureError> {
    let status: GitHubStatus =
        serde_json::from_slice(bytes).map_err(|_| ConfigureError::InvalidOutput {
            category: "GitHub account discovery",
        })?;
    let mut accounts = Vec::new();
    for (hostname, records) in status.hosts {
        if !valid_hostname(&hostname) {
            return Err(ConfigureError::InvalidOutput {
                category: "GitHub account discovery",
            });
        }
        let mut active = 0_usize;
        for record in records {
            if record.host != hostname || !valid_login(&record.login) {
                return Err(ConfigureError::InvalidOutput {
                    category: "GitHub account discovery",
                });
            }
            if record.active {
                active += 1;
                accounts.push(HostAccount {
                    hostname: hostname.clone(),
                    login: Some(record.login),
                });
            }
        }
        if active > 1 {
            return Err(ConfigureError::InvalidOutput {
                category: "GitHub account discovery",
            });
        }
    }
    Ok(accounts)
}

fn parse_gitlab_accounts(
    bytes: &[u8],
    command_succeeded: bool,
) -> Result<Vec<HostAccount>, ConfigureError> {
    let text = std::str::from_utf8(bytes).map_err(|_| ConfigureError::InvalidOutput {
        category: "GitLab account discovery",
    })?;
    let mut accounts = Vec::new();
    let mut current_host: Option<&str> = None;
    let mut unauthenticated = false;
    let mut seen = BTreeSet::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("You are not logged into any GitLab hosts.") {
            unauthenticated = true;
            continue;
        }
        if !line.starts_with(char::is_whitespace) && valid_hostname(trimmed) {
            current_host = Some(trimmed);
            continue;
        }
        if let Some(record) = trimmed.strip_prefix("✓ Logged in to ") {
            let (hostname, login_and_path) =
                record
                    .split_once(" as ")
                    .ok_or(ConfigureError::InvalidOutput {
                        category: "GitLab account discovery",
                    })?;
            let (login, _) =
                login_and_path
                    .split_once(" (")
                    .ok_or(ConfigureError::InvalidOutput {
                        category: "GitLab account discovery",
                    })?;
            if current_host != Some(hostname)
                || !valid_hostname(hostname)
                || !valid_login(login)
                || !seen.insert(hostname.to_owned())
            {
                return Err(ConfigureError::InvalidOutput {
                    category: "GitLab account discovery",
                });
            }
            accounts.push(HostAccount {
                hostname: hostname.to_owned(),
                login: Some(login.to_owned()),
            });
            continue;
        }
        if trimmed.contains("Logged in") {
            return Err(ConfigureError::InvalidOutput {
                category: "GitLab account discovery",
            });
        }
        if trimmed.starts_with('✓') || trimmed.starts_with('x') || trimmed.starts_with('!') {
            continue;
        }
        return Err(ConfigureError::InvalidOutput {
            category: "GitLab account discovery",
        });
    }
    if unauthenticated && accounts.is_empty() {
        return Ok(Vec::new());
    }
    if !command_succeeded {
        return Err(host_failure("GitLab account discovery"));
    }
    if accounts.is_empty() {
        return Err(ConfigureError::InvalidOutput {
            category: "GitLab account discovery",
        });
    }
    Ok(accounts)
}

fn valid_hostname(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':'))
}

fn valid_login(value: &str) -> bool {
    !value.is_empty() && !value.chars().any(char::is_control)
}

fn trim_one_line_ending(bytes: &mut Vec<u8>) {
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    } else if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
}
