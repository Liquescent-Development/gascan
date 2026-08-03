use super::{ConfigureError, Forge, GitProtocol, GitSetup, configure_ssh_host};
use crate::guest::{GuestCommand, GuestOutput, GuestRunner, Secret};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use gascan_proto::v1;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::net::IpAddr;

const MAX_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub(crate) struct ForgeRequest {
    pub(crate) forge: Forge,
    pub(crate) hostname: String,
    pub(crate) protocol: GitProtocol,
    pub(crate) token: Secret,
    pub(crate) key: GitSetup,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct ForgeSetup {
    pub(crate) forge: Forge,
    pub(crate) hostname: String,
    pub(crate) login: String,
    pub(crate) authenticated: bool,
    pub(crate) authentication_key: RegistrationState,
    pub(crate) signing_key: RegistrationState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RegistrationState {
    Existing,
    Added,
    Skipped,
    Failed,
}

#[derive(Clone, Copy)]
struct ForgeClient {
    forge: Forge,
}

impl ForgeClient {
    const fn binary(self) -> &'static [u8] {
        match self.forge {
            Forge::GitHub => b"gh",
            Forge::GitLab => b"glab",
        }
    }

    const fn authentication_category(self) -> &'static str {
        match self.forge {
            Forge::GitHub => "GitHub authentication",
            Forge::GitLab => "GitLab authentication",
        }
    }

    const fn registration_category(self) -> &'static str {
        match self.forge {
            Forge::GitHub => "GitHub key registration",
            Forge::GitLab => "GitLab key registration",
        }
    }

    const fn ssh_category(self) -> &'static str {
        match self.forge {
            Forge::GitHub => "GitHub SSH verification",
            Forge::GitLab => "GitLab SSH verification",
        }
    }

    const fn retry(self) -> &'static str {
        match self.forge {
            Forge::GitHub => "gascan configure gh",
            Forge::GitLab => "gascan configure glab",
        }
    }

    fn environment(self) -> Vec<v1::EnvironmentVariable> {
        let update = match self.forge {
            Forge::GitHub => ("GH_NO_UPDATE_NOTIFIER", "1"),
            Forge::GitLab => ("GLAB_CHECK_UPDATE", "0"),
        };
        [update, ("NO_COLOR", "1")]
            .into_iter()
            .map(|(name, value)| v1::EnvironmentVariable {
                name: name.to_owned(),
                value: value.to_owned(),
            })
            .collect()
    }

    fn login_argv(self, hostname: &str, protocol: GitProtocol) -> Vec<Vec<u8>> {
        let mut argv = vec![
            self.binary().to_vec(),
            b"auth".to_vec(),
            b"login".to_vec(),
            b"--hostname".to_vec(),
            hostname.as_bytes().to_vec(),
            b"--git-protocol".to_vec(),
            protocol_name(protocol).as_bytes().to_vec(),
        ];
        match self.forge {
            Forge::GitHub => argv.push(b"--with-token".to_vec()),
            Forge::GitLab => argv.push(b"--stdin".to_vec()),
        }
        argv
    }

    fn status_argv(self, hostname: &str) -> Vec<Vec<u8>> {
        vec![
            self.binary().to_vec(),
            b"auth".to_vec(),
            b"status".to_vec(),
            b"--hostname".to_vec(),
            hostname.as_bytes().to_vec(),
        ]
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GitHubKey {
    #[serde(rename = "id")]
    _id: u64,
    key: String,
    #[serde(rename = "title")]
    _title: String,
    #[serde(rename = "created_at")]
    _created_at: String,
    #[serde(default, rename = "url")]
    _url: Option<String>,
    #[serde(default, rename = "verified")]
    _verified: Option<bool>,
    #[serde(default, rename = "read_only")]
    _read_only: Option<bool>,
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum GitLabUsage {
    Auth,
    Signing,
    AuthAndSigning,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GitLabKey {
    #[serde(rename = "id")]
    _id: u64,
    #[serde(rename = "title")]
    _title: String,
    key: String,
    #[serde(rename = "created_at")]
    _created_at: String,
    #[serde(default, rename = "expires_at")]
    _expires_at: Option<String>,
    #[serde(default, rename = "last_used_at")]
    _last_used_at: Option<String>,
    usage_type: GitLabUsage,
}

pub(crate) async fn configure_forge<R: GuestRunner>(
    runner: &mut R,
    selector: v1::SandboxSelector,
    request: ForgeRequest,
) -> Result<ForgeSetup, ConfigureError> {
    let ForgeRequest {
        forge,
        hostname,
        protocol,
        token,
        key,
    } = request;
    let client = ForgeClient { forge };
    if !valid_hostname(&hostname) {
        return Err(ConfigureError::InvalidOutput {
            category: "forge request",
        });
    }
    let mut setup = ForgeSetup {
        forge,
        hostname: hostname.clone(),
        login: String::new(),
        authenticated: false,
        authentication_key: RegistrationState::Skipped,
        signing_key: RegistrationState::Skipped,
    };

    let Some(wanted) = public_key_identity(&key.public_key, Some(&selector.sandbox_id)) else {
        return Err(forge_error(
            setup,
            client,
            client.authentication_category(),
            "request validation did not complete".to_owned(),
        ));
    };
    if key.protocol != protocol {
        return Err(forge_error(
            setup,
            client,
            client.authentication_category(),
            "request validation did not complete".to_owned(),
        ));
    }

    let redaction = token.redaction_copy();
    let login = runner
        .execute(
            selector.clone(),
            GuestCommand {
                argv: client.login_argv(&hostname, protocol),
                environment: client.environment(),
                stdin: Some(token),
            },
        )
        .await;
    match login {
        Ok(GuestOutput { code: 0, .. }) => drop(redaction),
        Ok(output) => {
            let message =
                safe_native_diagnostic(&output, redaction.expose()).unwrap_or_else(|| {
                    format!("native authentication exited with status {}", output.code)
                });
            return Err(forge_error(
                setup,
                client,
                client.authentication_category(),
                message,
            ));
        }
        Err(_) => {
            return Err(forge_error(
                setup,
                client,
                client.authentication_category(),
                "authentication command could not be executed".to_owned(),
            ));
        }
    }

    let status = execute(
        runner,
        selector.clone(),
        client.status_argv(&hostname),
        client.environment(),
        None,
    )
    .await;
    let Some(login) = status.and_then(|output| parse_login(forge, &hostname, protocol, output))
    else {
        return Err(forge_error(
            setup,
            client,
            client.authentication_category(),
            "native authentication could not be verified".to_owned(),
        ));
    };
    setup.login = login;
    setup.authenticated = true;

    let (registration_failed, ssh_host_configured) = match forge {
        Forge::GitHub => {
            configure_github(
                runner, &selector, &hostname, protocol, &key, &wanted, &mut setup,
            )
            .await
        }
        Forge::GitLab => {
            configure_gitlab(
                runner, &selector, &hostname, protocol, &key, &wanted, &mut setup,
            )
            .await
        }
    };

    let mut ssh_failed = false;
    if protocol == GitProtocol::Ssh
        && matches!(
            setup.authentication_key,
            RegistrationState::Existing | RegistrationState::Added
        )
    {
        ssh_failed = !verify_ssh(
            runner,
            selector,
            forge,
            &hostname,
            &setup.login,
            !ssh_host_configured,
        )
        .await;
    }

    if registration_failed {
        Err(forge_error(
            setup,
            client,
            client.registration_category(),
            "one or more key registrations did not complete".to_owned(),
        ))
    } else if ssh_failed {
        Err(forge_error(
            setup,
            client,
            client.ssh_category(),
            "SSH authentication could not be verified".to_owned(),
        ))
    } else {
        Ok(setup)
    }
}

async fn configure_github<R: GuestRunner>(
    runner: &mut R,
    selector: &v1::SandboxSelector,
    hostname: &str,
    protocol: GitProtocol,
    key: &GitSetup,
    wanted: &KeyIdentity,
    setup: &mut ForgeSetup,
) -> (bool, bool) {
    let client = ForgeClient {
        forge: Forge::GitHub,
    };
    let authentication = github_list(runner, selector, hostname, "user/keys", client).await;
    let signing = github_list(runner, selector, hostname, "user/ssh_signing_keys", client).await;

    setup.authentication_key = match authentication {
        Some(keys) if keys.iter().any(|entry| key_matches(&entry.key, wanted)) => {
            RegistrationState::Existing
        }
        Some(_) => RegistrationState::Skipped,
        None => RegistrationState::Failed,
    };
    setup.signing_key = match signing {
        Some(keys) if keys.iter().any(|entry| key_matches(&entry.key, wanted)) => {
            RegistrationState::Existing
        }
        Some(_) => RegistrationState::Skipped,
        None => RegistrationState::Failed,
    };

    let title = format!("Gas Can {}", selector.sandbox_id);
    let mut ssh_host_configured = false;
    if setup.authentication_key == RegistrationState::Skipped {
        let host_ready = if protocol == GitProtocol::Ssh {
            ssh_host_configured = configure_ssh_host(runner, selector.clone(), hostname)
                .await
                .is_ok();
            ssh_host_configured
        } else {
            true
        };
        setup.authentication_key = if host_ready
            && github_register(
                runner,
                selector,
                hostname,
                "user/keys",
                &title,
                &key.public_key,
                client,
                wanted,
            )
            .await
        {
            RegistrationState::Added
        } else {
            RegistrationState::Failed
        };
    }
    if setup.signing_key == RegistrationState::Skipped {
        setup.signing_key = if github_register(
            runner,
            selector,
            hostname,
            "user/ssh_signing_keys",
            &title,
            &key.public_key,
            client,
            wanted,
        )
        .await
        {
            RegistrationState::Added
        } else {
            RegistrationState::Failed
        };
    }
    (
        setup.authentication_key == RegistrationState::Failed
            || setup.signing_key == RegistrationState::Failed,
        ssh_host_configured,
    )
}

async fn github_list<R: GuestRunner>(
    runner: &mut R,
    selector: &v1::SandboxSelector,
    hostname: &str,
    endpoint: &str,
    client: ForgeClient,
) -> Option<Vec<GitHubKey>> {
    let output = execute(
        runner,
        selector.clone(),
        vec![
            b"gh".to_vec(),
            b"api".to_vec(),
            b"--hostname".to_vec(),
            hostname.as_bytes().to_vec(),
            endpoint.as_bytes().to_vec(),
        ],
        client.environment(),
        None,
    )
    .await?;
    parse_json(output)
}

#[allow(clippy::too_many_arguments)]
async fn github_register<R: GuestRunner>(
    runner: &mut R,
    selector: &v1::SandboxSelector,
    hostname: &str,
    endpoint: &str,
    title: &str,
    public_key: &str,
    client: ForgeClient,
    wanted: &KeyIdentity,
) -> bool {
    let output = execute(
        runner,
        selector.clone(),
        vec![
            b"gh".to_vec(),
            b"api".to_vec(),
            b"--hostname".to_vec(),
            hostname.as_bytes().to_vec(),
            b"--method".to_vec(),
            b"POST".to_vec(),
            endpoint.as_bytes().to_vec(),
            b"--raw-field".to_vec(),
            format!("title={title}").into_bytes(),
            b"--raw-field".to_vec(),
            format!("key={public_key}").into_bytes(),
        ],
        client.environment(),
        None,
    )
    .await;
    output
        .and_then(parse_json::<GitHubKey>)
        .is_some_and(|created| key_matches(&created.key, wanted))
}

async fn configure_gitlab<R: GuestRunner>(
    runner: &mut R,
    selector: &v1::SandboxSelector,
    hostname: &str,
    protocol: GitProtocol,
    key: &GitSetup,
    wanted: &KeyIdentity,
    setup: &mut ForgeSetup,
) -> (bool, bool) {
    let client = ForgeClient {
        forge: Forge::GitLab,
    };
    let output = execute(
        runner,
        selector.clone(),
        vec![
            b"glab".to_vec(),
            b"api".to_vec(),
            b"--hostname".to_vec(),
            hostname.as_bytes().to_vec(),
            b"/user/keys".to_vec(),
        ],
        client.environment(),
        None,
    )
    .await;
    let Some(keys) = output.and_then(parse_json::<Vec<GitLabKey>>) else {
        setup.authentication_key = RegistrationState::Failed;
        setup.signing_key = RegistrationState::Failed;
        return (true, false);
    };

    let mut authentication = false;
    let mut signing = false;
    for entry in keys.iter().filter(|entry| key_matches(&entry.key, wanted)) {
        match entry.usage_type {
            GitLabUsage::Auth => authentication = true,
            GitLabUsage::Signing => signing = true,
            GitLabUsage::AuthAndSigning => {
                authentication = true;
                signing = true;
            }
        }
    }
    if authentication || signing {
        setup.authentication_key = if authentication {
            RegistrationState::Existing
        } else {
            RegistrationState::Failed
        };
        setup.signing_key = if signing {
            RegistrationState::Existing
        } else {
            RegistrationState::Failed
        };
        return (!(authentication && signing), false);
    }

    let ssh_host_configured = protocol == GitProtocol::Ssh
        && configure_ssh_host(runner, selector.clone(), hostname)
            .await
            .is_ok();
    let host_ready = protocol != GitProtocol::Ssh || ssh_host_configured;
    let registered = host_ready
        && gitlab_register(
            runner,
            selector,
            hostname,
            &format!("Gas Can {}", selector.sandbox_id),
            &key.public_key,
            client,
            wanted,
        )
        .await;
    let state = if registered {
        RegistrationState::Added
    } else {
        RegistrationState::Failed
    };
    setup.authentication_key = state;
    setup.signing_key = state;
    (!registered, ssh_host_configured)
}

async fn gitlab_register<R: GuestRunner>(
    runner: &mut R,
    selector: &v1::SandboxSelector,
    hostname: &str,
    title: &str,
    public_key: &str,
    client: ForgeClient,
    wanted: &KeyIdentity,
) -> bool {
    let output = execute(
        runner,
        selector.clone(),
        vec![
            b"glab".to_vec(),
            b"api".to_vec(),
            b"--hostname".to_vec(),
            hostname.as_bytes().to_vec(),
            b"--method".to_vec(),
            b"POST".to_vec(),
            b"/user/keys".to_vec(),
            b"--raw-field".to_vec(),
            format!("title={title}").into_bytes(),
            b"--raw-field".to_vec(),
            format!("key={public_key}").into_bytes(),
            b"--raw-field".to_vec(),
            b"usage_type=auth_and_signing".to_vec(),
        ],
        client.environment(),
        None,
    )
    .await;
    output
        .and_then(parse_json::<GitLabKey>)
        .is_some_and(|created| {
            created.usage_type == GitLabUsage::AuthAndSigning && key_matches(&created.key, wanted)
        })
}

async fn verify_ssh<R: GuestRunner>(
    runner: &mut R,
    selector: v1::SandboxSelector,
    forge: Forge,
    hostname: &str,
    expected_login: &str,
    configure_host: bool,
) -> bool {
    if configure_host
        && configure_ssh_host(runner, selector.clone(), hostname)
            .await
            .is_err()
    {
        return false;
    }
    let argv = vec![
        b"ssh".to_vec(),
        b"-T".to_vec(),
        format!("git@{hostname}").into_bytes(),
    ];
    let interactive = runner
        .execute_interactive(selector.clone(), argv.clone())
        .await;
    if !matches!(interactive, Ok(0 | 1)) {
        return false;
    }
    let Some(output) = execute(runner, selector, argv, Vec::new(), None).await else {
        return false;
    };
    valid_ssh_response(forge, expected_login, output)
}

async fn execute<R: GuestRunner>(
    runner: &mut R,
    selector: v1::SandboxSelector,
    argv: Vec<Vec<u8>>,
    environment: Vec<v1::EnvironmentVariable>,
    stdin: Option<Secret>,
) -> Option<GuestOutput> {
    runner
        .execute(
            selector,
            GuestCommand {
                argv,
                environment,
                stdin,
            },
        )
        .await
        .ok()
}

fn parse_login(
    forge: Forge,
    hostname: &str,
    protocol: GitProtocol,
    output: GuestOutput,
) -> Option<String> {
    if output.code != 0 {
        return None;
    }
    let text = bounded_text(&output)?;
    match forge {
        Forge::GitHub => parse_github_login(&text, hostname, protocol),
        Forge::GitLab => parse_gitlab_login(&text, hostname, protocol),
    }
}

struct GitHubStatusAccount {
    login: String,
    active: Option<bool>,
    protocol: Option<GitProtocol>,
}

fn parse_github_login(
    text: &str,
    hostname: &str,
    requested_protocol: GitProtocol,
) -> Option<String> {
    let login_prefix = format!("✓ Logged in to {hostname} account ");
    let mut current = None;
    let mut active_login = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix(&login_prefix) {
            if current.is_some() {
                finish_github_account(current.take(), requested_protocol, &mut active_login)?;
            }
            let (login, path) = value.split_once(" (")?;
            if !path.ends_with(')') || !valid_login(login) {
                return None;
            }
            current = Some(GitHubStatusAccount {
                login: login.to_owned(),
                active: None,
                protocol: None,
            });
        } else if line.contains("Logged in to ") {
            return None;
        } else if let Some(value) = line.strip_prefix("- Active account: ") {
            let account = current.as_mut()?;
            if account.active.is_some() {
                return None;
            }
            account.active = match value {
                "true" => Some(true),
                "false" => Some(false),
                _ => return None,
            };
        } else if let Some(value) = line.strip_prefix("- Git operations protocol: ") {
            let account = current.as_mut()?;
            if account.protocol.is_some() {
                return None;
            }
            account.protocol = parse_protocol(value);
            account.protocol?;
        }
    }
    finish_github_account(current, requested_protocol, &mut active_login)?;
    active_login
}

fn finish_github_account(
    account: Option<GitHubStatusAccount>,
    requested_protocol: GitProtocol,
    active_login: &mut Option<String>,
) -> Option<()> {
    let account = account?;
    let active = account.active?;
    let protocol = account.protocol?;
    if active {
        if protocol != requested_protocol || active_login.is_some() {
            return None;
        }
        *active_login = Some(account.login);
    }
    Some(())
}

fn parse_gitlab_login(
    text: &str,
    hostname: &str,
    requested_protocol: GitProtocol,
) -> Option<String> {
    let login_prefix = format!("✓ Logged in to {hostname} as ");
    let protocol_prefix = format!("✓ Git operations for {hostname} configured to use ");
    let mut login = None;
    let mut protocol = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix(&login_prefix) {
            if login.is_some() {
                return None;
            }
            let (value, path) = value.split_once(" (")?;
            if !path.ends_with(')') || !valid_login(value) {
                return None;
            }
            login = Some(value.to_owned());
        } else if line.contains("Logged in to ") {
            return None;
        } else if let Some(value) = line.strip_prefix(&protocol_prefix) {
            if protocol.is_some() {
                return None;
            }
            protocol = parse_protocol(value.strip_suffix(" protocol.")?);
            protocol?;
        } else if line.contains("Git operations for ") {
            return None;
        }
    }
    (protocol == Some(requested_protocol)).then_some(login?)
}

fn parse_protocol(value: &str) -> Option<GitProtocol> {
    match value {
        "ssh" => Some(GitProtocol::Ssh),
        "https" => Some(GitProtocol::Https),
        _ => None,
    }
}

fn valid_login(login: &str) -> bool {
    !login.is_empty()
        && login.len() <= 255
        && login
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn parse_json<T: DeserializeOwned>(output: GuestOutput) -> Option<T> {
    if output.code != 0
        || !output.stderr.is_empty()
        || output.stdout.is_empty()
        || output.stdout.len() > MAX_RESPONSE_BYTES
    {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

fn bounded_text(output: &GuestOutput) -> Option<String> {
    let length = output.stdout.len().checked_add(output.stderr.len())?;
    if length == 0 || length > MAX_RESPONSE_BYTES {
        return None;
    }
    let mut bytes = Vec::with_capacity(length.saturating_add(1));
    bytes.extend_from_slice(&output.stdout);
    if !output.stdout.is_empty() && !output.stderr.is_empty() {
        bytes.push(b'\n');
    }
    bytes.extend_from_slice(&output.stderr);
    String::from_utf8(bytes).ok()
}

fn safe_native_diagnostic(output: &GuestOutput, secret: &[u8]) -> Option<String> {
    for stream in [&output.stderr, &output.stdout] {
        let candidate = &stream[..stream.len().min(MAX_RESPONSE_BYTES)];
        let candidate = suppress_trailing_secret_prefix(candidate, secret);
        let redacted = redact_secret(candidate, secret);
        let Some(line) = redacted
            .split(|byte| matches!(byte, b'\n' | b'\r'))
            .find(|line| !line.is_empty())
        else {
            continue;
        };
        let diagnostic: String = String::from_utf8_lossy(line)
            .chars()
            .filter_map(|character| match character {
                '\t' => Some(' '),
                character if character.is_control() => None,
                character => Some(character),
            })
            .take(240)
            .collect();
        if !diagnostic.is_empty() {
            return Some(diagnostic);
        }
    }
    None
}

fn suppress_trailing_secret_prefix<'a>(input: &'a [u8], secret: &[u8]) -> &'a [u8] {
    if secret.len() <= 1 {
        return input;
    }

    let maximum = input.len().min(secret.len() - 1);
    for length in (1..=maximum).rev() {
        if input.ends_with(&secret[..length]) {
            return &input[..input.len() - length];
        }
    }
    input
}

fn redact_secret(input: &[u8], secret: &[u8]) -> Vec<u8> {
    if secret.is_empty() {
        return input.to_vec();
    }

    let mut redacted = Vec::with_capacity(input.len());
    let mut remaining = input;
    while let Some(offset) = remaining
        .windows(secret.len())
        .position(|candidate| candidate == secret)
    {
        redacted.extend_from_slice(&remaining[..offset]);
        redacted.extend_from_slice(b"[REDACTED]");
        remaining = &remaining[offset + secret.len()..];
    }
    redacted.extend_from_slice(remaining);
    redacted
}

fn valid_ssh_response(forge: Forge, expected_login: &str, output: GuestOutput) -> bool {
    let expected_code = match forge {
        Forge::GitHub => 1,
        Forge::GitLab => 0,
    };
    if output.code != expected_code {
        return false;
    }
    let Some(text) = bounded_text(&output) else {
        return false;
    };
    text.lines().any(|line| match forge {
        Forge::GitHub => line
            .strip_prefix("Hi ")
            .and_then(|rest| {
                rest.strip_suffix(
                    "! You've successfully authenticated, but GitHub does not provide shell access.",
                )
            })
            .is_some_and(|login| login == expected_login && valid_login(login)),
        Forge::GitLab => line
            .strip_prefix("Welcome to GitLab, @")
            .and_then(|rest| rest.strip_suffix('!'))
            .is_some_and(|login| login == expected_login && valid_login(login)),
    })
}

#[derive(Eq, PartialEq)]
struct KeyIdentity {
    algorithm: String,
    body: Vec<u8>,
}

fn public_key_identity(public_key: &str, sandbox_id: Option<&str>) -> Option<KeyIdentity> {
    let mut fields = public_key.split_ascii_whitespace();
    let algorithm = fields.next()?;
    let encoded = fields.next()?;
    let comment = fields.next();
    if fields.next().is_some()
        || sandbox_id.is_some_and(|id| comment != Some(format!("gascan-{id}").as_str()))
    {
        return None;
    }
    let body = STANDARD.decode(encoded).ok()?;
    if body.is_empty() || STANDARD.encode(&body) != encoded {
        return None;
    }
    let mut offset = 0;
    let decoded_algorithm = ssh_wire_string(&body, &mut offset)?;
    if decoded_algorithm != algorithm.as_bytes() || offset == body.len() {
        return None;
    }
    if sandbox_id.is_some() {
        if decoded_algorithm != b"ssh-ed25519" {
            return None;
        }
        let mut key_offset = offset;
        if ssh_wire_string(&body, &mut key_offset)?.len() != 32 || key_offset != body.len() {
            return None;
        }
    }
    Some(KeyIdentity {
        algorithm: std::str::from_utf8(decoded_algorithm).ok()?.to_owned(),
        body: body[offset..].to_vec(),
    })
}

fn ssh_wire_string<'a>(blob: &'a [u8], offset: &mut usize) -> Option<&'a [u8]> {
    let length_bytes: [u8; 4] = blob.get(*offset..offset.checked_add(4)?)?.try_into().ok()?;
    *offset += 4;
    let length = usize::try_from(u32::from_be_bytes(length_bytes)).ok()?;
    let end = offset.checked_add(length)?;
    let value = blob.get(*offset..end)?;
    *offset = end;
    Some(value)
}

fn key_matches(public_key: &str, wanted: &KeyIdentity) -> bool {
    public_key_identity(public_key, None).is_some_and(|identity| identity == *wanted)
}

fn valid_hostname(hostname: &str) -> bool {
    if hostname.len() > 253
        || hostname.parse::<IpAddr>().is_ok()
        || !hostname.is_ascii()
        || hostname.bytes().any(|byte| byte.is_ascii_uppercase())
    {
        return false;
    }
    let labels: Vec<&str> = hostname.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

const fn protocol_name(protocol: GitProtocol) -> &'static str {
    match protocol {
        GitProtocol::Ssh => "ssh",
        GitProtocol::Https => "https",
    }
}

fn forge_error(
    setup: ForgeSetup,
    client: ForgeClient,
    category: &'static str,
    message: String,
) -> ConfigureError {
    ConfigureError::Forge {
        hostname: setup.hostname.clone(),
        setup: Box::new(setup),
        category,
        message,
        retry: client.retry(),
    }
}
