use super::ConfigureError;
use crate::guest::{GuestCommand, GuestOutput, GuestRunner};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use gascan_proto::v1;
use serde::Deserialize;

const HELPER: &[u8] = b"/usr/local/bin/configure-developer-home";
const MAX_STATUS_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub(crate) enum GitProtocol {
    Ssh,
    Https,
}

impl GitProtocol {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Ssh => "ssh",
            Self::Https => "https",
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct GitRequest {
    pub(crate) sandbox_id: String,
    pub(crate) name: String,
    pub(crate) email: String,
    pub(crate) protocol: GitProtocol,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct GitSetup {
    pub(crate) name: String,
    pub(crate) email: String,
    pub(crate) protocol: GitProtocol,
    pub(crate) public_key: String,
    pub(crate) fingerprint: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeveloperStatus {
    name: Option<String>,
    email: Option<String>,
    protocol: Option<GitProtocol>,
    public_key: Option<String>,
    fingerprint: Option<String>,
    #[serde(rename = "receipt")]
    _receipt: ReceiptState,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum ReceiptState {
    Pending,
    Complete,
    Declined,
}

pub(crate) async fn configure_git<R: GuestRunner>(
    runner: &mut R,
    selector: v1::SandboxSelector,
    request: GitRequest,
) -> Result<GitSetup, ConfigureError> {
    if selector.sandbox_id != request.sandbox_id {
        return Err(ConfigureError::GuestCommand {
            category: "developer-home Git setup",
            message: "sandbox selector did not match the setup request".to_owned(),
        });
    }
    let protocol = request.protocol.as_str();
    let mutation = execute(
        runner,
        selector.clone(),
        vec![
            HELPER.to_vec(),
            b"git".to_vec(),
            b"--sandbox-id".to_vec(),
            request.sandbox_id.as_bytes().to_vec(),
            b"--name".to_vec(),
            request.name.as_bytes().to_vec(),
            b"--email".to_vec(),
            request.email.as_bytes().to_vec(),
            b"--protocol".to_vec(),
            protocol.as_bytes().to_vec(),
        ],
        "developer-home Git setup",
    )
    .await?;
    require_silent_success(mutation, "developer-home Git setup")?;

    let status = execute(
        runner,
        selector,
        vec![HELPER.to_vec(), b"status".to_vec()],
        "developer-home status",
    )
    .await?;
    let parsed = parse_status(status)?;
    if parsed.name.as_deref() != Some(request.name.as_str())
        || parsed.email.as_deref() != Some(request.email.as_str())
        || parsed.protocol != Some(request.protocol)
    {
        return Err(invalid_status());
    }
    let public_key = parsed.public_key.ok_or_else(invalid_status)?;
    let fingerprint = parsed.fingerprint.ok_or_else(invalid_status)?;
    if !valid_public_key(&public_key, &request.sandbox_id) || !valid_fingerprint(&fingerprint) {
        return Err(invalid_status());
    }
    Ok(GitSetup {
        name: request.name,
        email: request.email,
        protocol: request.protocol,
        public_key,
        fingerprint,
    })
}

pub(crate) async fn configure_ssh_host<R: GuestRunner>(
    runner: &mut R,
    selector: v1::SandboxSelector,
    hostname: &str,
) -> Result<(), ConfigureError> {
    let output = execute(
        runner,
        selector,
        vec![
            HELPER.to_vec(),
            b"ssh-host".to_vec(),
            b"--hostname".to_vec(),
            hostname.as_bytes().to_vec(),
        ],
        "developer-home SSH host setup",
    )
    .await?;
    require_silent_success(output, "developer-home SSH host setup")
}

async fn execute<R: GuestRunner>(
    runner: &mut R,
    selector: v1::SandboxSelector,
    argv: Vec<Vec<u8>>,
    category: &'static str,
) -> Result<GuestOutput, ConfigureError> {
    runner
        .execute(
            selector,
            GuestCommand {
                argv,
                environment: Vec::new(),
                stdin: None,
            },
        )
        .await
        .map_err(|_| ConfigureError::GuestCommand {
            category,
            message: "helper execution was unavailable".to_owned(),
        })
}

fn require_silent_success(
    output: GuestOutput,
    category: &'static str,
) -> Result<(), ConfigureError> {
    if output.code != 0 {
        return Err(ConfigureError::GuestCommand {
            category,
            message: "helper did not complete successfully".to_owned(),
        });
    }
    if !output.stdout.is_empty() || !output.stderr.is_empty() {
        return Err(ConfigureError::InvalidOutput { category });
    }
    Ok(())
}

fn parse_status(output: GuestOutput) -> Result<DeveloperStatus, ConfigureError> {
    if output.code != 0 {
        return Err(ConfigureError::GuestCommand {
            category: "developer-home status",
            message: "helper did not complete successfully".to_owned(),
        });
    }
    if !output.stderr.is_empty()
        || output.stdout.is_empty()
        || output.stdout.len() > MAX_STATUS_BYTES
    {
        return Err(invalid_status());
    }
    serde_json::from_slice(&output.stdout).map_err(|_| invalid_status())
}

fn invalid_status() -> ConfigureError {
    ConfigureError::InvalidOutput {
        category: "developer-home status",
    }
}

fn valid_public_key(public_key: &str, sandbox_id: &str) -> bool {
    let mut fields = public_key.split_ascii_whitespace();
    if fields.next() != Some("ssh-ed25519") {
        return false;
    }
    let Some(encoded) = fields.next() else {
        return false;
    };
    let Some(comment) = fields.next() else {
        return false;
    };
    let Ok(blob) = STANDARD.decode(encoded) else {
        return false;
    };
    fields.next().is_none()
        && comment == format!("gascan-{sandbox_id}")
        && blob.len() == 51
        && blob[0..4] == 11_u32.to_be_bytes()
        && &blob[4..15] == b"ssh-ed25519"
        && blob[15..19] == 32_u32.to_be_bytes()
        && STANDARD.encode(blob) == encoded
}

fn valid_fingerprint(fingerprint: &str) -> bool {
    let Some(encoded) = fingerprint.strip_prefix("SHA256:") else {
        return false;
    };
    encoded.len() == 43
        && encoded
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
}
