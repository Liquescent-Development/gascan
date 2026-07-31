use super::{ConfigureError, Forge, GitDefaults, HostAccount, HostDiscovery};
use crate::guest::{Secret, SensitiveBytes};
#[cfg(test)]
use crate::guest::{SensitiveDropKind, SensitiveDropObserver};
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
    sensitive_capture: SensitiveCaptureConfig,
}

impl SystemHostDiscovery {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(super) fn with_program_directory(program_directory: PathBuf) -> Self {
        Self {
            program_directory: Some(program_directory),
            sensitive_capture: SensitiveCaptureConfig::default(),
        }
    }

    #[cfg(test)]
    pub(super) fn with_program_directory_and_sensitive_observer(
        program_directory: PathBuf,
        observer: SensitiveDropObserver,
    ) -> Self {
        Self {
            program_directory: Some(program_directory),
            sensitive_capture: SensitiveCaptureConfig {
                observer: Some(observer),
            },
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

    fn run_sensitive(
        &self,
        program: &'static str,
        arguments: &[&OsStr],
        category: &'static str,
    ) -> Result<SensitiveCommandOutput, ConfigureError> {
        run_sensitive_bounded(
            &self.program(program),
            arguments,
            category,
            self.sensitive_capture.clone(),
        )
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
            Forge::GitHub => self.run_sensitive(
                "gh",
                &[
                    OsStr::new("auth"),
                    OsStr::new("token"),
                    OsStr::new("--hostname"),
                    hostname,
                ],
                "GitHub token retrieval",
            )?,
            Forge::GitLab => self.run_sensitive(
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
        token.trim_one_line_ending();
        if token.is_empty() {
            return Err(ConfigureError::InvalidOutput {
                category: "forge token",
            });
        }
        Ok(Secret::from_sensitive(token))
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

#[derive(Clone, Default)]
struct SensitiveCaptureConfig {
    #[cfg(test)]
    observer: Option<SensitiveDropObserver>,
}

#[derive(Clone, Copy)]
enum SensitiveStream {
    Stdout,
    Stderr,
}

impl SensitiveCaptureConfig {
    fn buffer(&self, capacity: usize, stream: SensitiveStream, scratch: bool) -> SensitiveBytes {
        let mut bytes = SensitiveBytes::zeroed(capacity);
        #[cfg(test)]
        if let Some(observer) = &self.observer {
            let kind = match (stream, scratch) {
                (SensitiveStream::Stdout, true) => SensitiveDropKind::StdoutScratch,
                (SensitiveStream::Stderr, true) => SensitiveDropKind::StderrScratch,
                (SensitiveStream::Stdout, false) => SensitiveDropKind::StdoutAccumulation,
                (SensitiveStream::Stderr, false) => SensitiveDropKind::StderrAccumulation,
            };
            bytes.observe_drop(observer.clone(), kind);
        }
        #[cfg(not(test))]
        let _ = (stream, scratch);
        bytes
    }
}

struct SensitiveCommandOutput {
    status: ExitStatus,
    stdout: SensitiveBytes,
}

impl std::fmt::Debug for SensitiveCommandOutput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SensitiveCommandOutput")
            .field("status", &self.status)
            .field("stdout", &"[REDACTED]")
            .finish()
    }
}

struct SensitiveCapture {
    bytes: SensitiveBytes,
    exceeded: bool,
}

fn run_sensitive_bounded(
    program: &Path,
    arguments: &[&OsStr],
    category: &'static str,
    capture_config: SensitiveCaptureConfig,
) -> Result<SensitiveCommandOutput, ConfigureError> {
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
    let stdout_config = capture_config.clone();
    let stdout_reader = std::thread::spawn(move || {
        capture_sensitive(stdout, stdout_config, SensitiveStream::Stdout)
    });
    let stderr_reader = std::thread::spawn(move || {
        capture_sensitive(stderr, capture_config, SensitiveStream::Stderr)
    });

    let status = child.wait();
    let stdout = stdout_reader
        .join()
        .map_err(|_| std::io::Error::other("sensitive stdout reader stopped"));
    let stderr = stderr_reader
        .join()
        .map_err(|_| std::io::Error::other("sensitive stderr reader stopped"));
    let status = status?;
    let stdout = stdout??;
    let stderr = stderr??;
    if stdout.exceeded || stderr.exceeded {
        return Err(ConfigureError::InvalidOutput { category });
    }
    Ok(SensitiveCommandOutput {
        status,
        stdout: stdout.bytes,
    })
}

fn capture_sensitive(
    mut reader: impl std::io::Read,
    config: SensitiveCaptureConfig,
    stream: SensitiveStream,
) -> std::io::Result<SensitiveCapture> {
    let mut bytes = config.buffer(MAX_HOST_OUTPUT_BYTES, stream, false);
    let mut scratch = config.buffer(16 * 1024, stream, true);
    let mut exceeded = false;
    loop {
        let count = reader.read(scratch.storage_mut())?;
        if count == 0 {
            break;
        }
        exceeded |= bytes.append_bounded(&scratch.storage()[..count]);
        scratch.clear_storage();
    }
    Ok(SensitiveCapture { bytes, exceeded })
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
    if unauthenticated {
        if accounts.is_empty() && current_host.is_none() {
            return Ok(Vec::new());
        }
        return Err(ConfigureError::InvalidOutput {
            category: "GitLab account discovery",
        });
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
