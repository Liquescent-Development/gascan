use super::{
    FileIdentity, PRIVATE_MODE, PUBLIC_MODE, SshError, SshPaths, StateDirectory,
    maximum_managed_file_bytes, random_staging_name,
};
use base64::Engine as _;
use camino::Utf8PathBuf;
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

const PRIVATE_KEY_NAME: &str = "identity_ed25519";
const PUBLIC_KEY_NAME: &str = "identity_ed25519.pub";
const SSH_KEYGEN: &str = "/usr/bin/ssh-keygen";
const KEYGEN_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_SUBPROCESS_OUTPUT: usize = 16 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostIdentity {
    pub private_key: Utf8PathBuf,
    pub public_key: String,
    pub fingerprint: String,
}

pub async fn ensure_host_identity(paths: &SshPaths) -> Result<HostIdentity, SshError> {
    let directory = StateDirectory::open(paths)?;
    let private = directory.metadata(PRIVATE_KEY_NAME, PRIVATE_MODE)?;
    let public = directory.metadata(PUBLIC_KEY_NAME, PUBLIC_MODE)?;
    match (private, public) {
        (Some(_), Some(_)) => load_existing(&directory, paths).await,
        (None, None) => {
            generate(&directory).await?;
            load_existing(&directory, paths).await
        }
        _ => Err(SshError::InvalidState("managed SSH identity is incomplete")),
    }
}

async fn generate(directory: &StateDirectory) -> Result<(), SshError> {
    let private_stage = random_staging_name()?;
    let public_stage = format!("{private_stage}.pub");
    let mut guard = StagingGuard::new(directory, [&private_stage, &public_stage]);
    let output_path = directory.resolved_path(&private_stage)?;
    let output = run_ssh_keygen(vec![
        OsString::from("-q"),
        OsString::from("-t"),
        OsString::from("ed25519"),
        OsString::from("-N"),
        OsString::new(),
        OsString::from("-C"),
        OsString::from("gascan-managed"),
        OsString::from("-f"),
        output_path.into_os_string(),
    ])
    .await?;
    if !output.is_empty() {
        return Err(SshError::KeygenOutput);
    }

    directory.harden_staging(&private_stage, PRIVATE_MODE)?;
    directory.harden_staging(&public_stage, PUBLIC_MODE)?;
    validate_pair(directory, &private_stage, &public_stage).await?;

    directory.rename_new(&private_stage, PRIVATE_KEY_NAME)?;
    if let Err(error) = directory.rename_new(&public_stage, PUBLIC_KEY_NAME) {
        let rollback = directory.rename_back(PRIVATE_KEY_NAME, &private_stage);
        return match rollback {
            Ok(()) => Err(error),
            Err(_) => Err(SshError::InvalidState(
                "managed SSH identity publication could not be rolled back",
            )),
        };
    }
    directory.sync()?;
    guard.disarm();
    Ok(())
}

async fn load_existing(
    directory: &StateDirectory,
    paths: &SshPaths,
) -> Result<HostIdentity, SshError> {
    let parsed = validate_pair(directory, PRIVATE_KEY_NAME, PUBLIC_KEY_NAME).await?;
    Ok(HostIdentity {
        private_key: paths.private_key.clone(),
        public_key: parsed.normalized,
        fingerprint: parsed.fingerprint,
    })
}

async fn validate_pair(
    directory: &StateDirectory,
    private_name: &str,
    public_name: &str,
) -> Result<ParsedPublicKey, SshError> {
    let (private_file, private_identity) = directory.open_file(private_name, PRIVATE_MODE)?;
    let (public_bytes, public_identity) = directory.read_file(
        public_name,
        PUBLIC_MODE,
        maximum_managed_file_bytes().min(MAX_SUBPROCESS_OUTPUT as u64),
    )?;
    let stored_public = parse_public_key(&public_bytes)?;

    let private_path = directory.resolved_open_file(&private_file)?;
    let derived = run_ssh_keygen(vec![
        OsString::from("-y"),
        OsString::from("-f"),
        private_path.into_os_string(),
    ])
    .await?;
    let derived_public = parse_public_key(&derived)?;
    if stored_public.normalized != derived_public.normalized {
        return Err(SshError::InvalidState(
            "managed SSH private and public keys do not match",
        ));
    }
    require_unchanged(directory, private_name, PRIVATE_MODE, private_identity)?;
    require_unchanged(directory, public_name, PUBLIC_MODE, public_identity)?;
    Ok(stored_public)
}

fn require_unchanged(
    directory: &StateDirectory,
    name: &str,
    mode: u16,
    expected: FileIdentity,
) -> Result<(), SshError> {
    if directory.metadata(name, mode)? != Some(expected) {
        return Err(SshError::InvalidState(
            "managed SSH file changed during validation",
        ));
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) struct ParsedPublicKey {
    pub(crate) normalized: String,
    pub(crate) fingerprint: String,
}

pub(crate) fn parse_public_key(bytes: &[u8]) -> Result<ParsedPublicKey, SshError> {
    if bytes.is_empty() || bytes.len() > MAX_SUBPROCESS_OUTPUT || bytes.contains(&b'\0') {
        return Err(SshError::InvalidState(
            "managed SSH public key is malformed",
        ));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| SshError::InvalidState("managed SSH public key is malformed"))?;
    let trimmed = text.trim_end_matches(['\r', '\n']);
    if trimmed.is_empty() || trimmed.contains(['\r', '\n']) {
        return Err(SshError::InvalidState(
            "managed SSH public key is malformed",
        ));
    }
    let mut fields = trimmed.split_ascii_whitespace();
    if fields.next() != Some("ssh-ed25519") {
        return Err(SshError::InvalidState("managed SSH key is not Ed25519"));
    }
    let encoded = fields.next().ok_or(SshError::InvalidState(
        "managed SSH public key is malformed",
    ))?;
    let blob = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| SshError::InvalidState("managed SSH public key is malformed"))?;
    validate_ed25519_blob(&blob)?;
    let fingerprint = Sha256::digest(&blob);
    Ok(ParsedPublicKey {
        normalized: format!("ssh-ed25519 {encoded}"),
        fingerprint: format!(
            "SHA256:{}",
            base64::engine::general_purpose::STANDARD_NO_PAD.encode(fingerprint)
        ),
    })
}

fn validate_ed25519_blob(blob: &[u8]) -> Result<(), SshError> {
    const ALGORITHM: &[u8] = b"ssh-ed25519";
    const ALGORITHM_PREFIX: [u8; 4] = (ALGORITHM.len() as u32).to_be_bytes();
    const KEY_PREFIX: [u8; 4] = 32_u32.to_be_bytes();
    let expected_length = 4 + ALGORITHM.len() + 4 + 32;
    if blob.len() != expected_length
        || blob.get(..4) != Some(ALGORITHM_PREFIX.as_slice())
        || blob.get(4..4 + ALGORITHM.len()) != Some(ALGORITHM)
        || blob.get(4 + ALGORITHM.len()..8 + ALGORITHM.len()) != Some(KEY_PREFIX.as_slice())
    {
        return Err(SshError::InvalidState(
            "managed SSH public key is malformed",
        ));
    }
    Ok(())
}

async fn run_ssh_keygen(args: Vec<OsString>) -> Result<Vec<u8>, SshError> {
    let mut child = Command::new(SSH_KEYGEN);
    child
        .args(args)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = child
        .spawn()
        .map_err(|error| SshError::io("start bounded ssh-keygen", error))?;
    let stdout = child.stdout.take().ok_or(SshError::KeygenOutput)?;
    let stderr = child.stderr.take().ok_or(SshError::KeygenOutput)?;
    let completed = tokio::time::timeout(KEYGEN_TIMEOUT, async {
        tokio::try_join!(child.wait(), read_bounded(stdout), read_bounded(stderr))
    })
    .await;
    let (status, stdout, stderr) = match completed {
        Ok(result) => result.map_err(|error| SshError::io("run bounded ssh-keygen", error))?,
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(SshError::KeygenTimeout);
        }
    };
    if stdout.len() > MAX_SUBPROCESS_OUTPUT || stderr.len() > MAX_SUBPROCESS_OUTPUT {
        return Err(SshError::KeygenOutput);
    }
    if !status.success() {
        return Err(SshError::KeygenRejected);
    }
    Ok(stdout)
}

async fn read_bounded<R>(reader: R) -> Result<Vec<u8>, std::io::Error>
where
    R: AsyncRead + Unpin,
{
    let mut bytes = Vec::new();
    reader
        .take((MAX_SUBPROCESS_OUTPUT + 1) as u64)
        .read_to_end(&mut bytes)
        .await?;
    Ok(bytes)
}

struct StagingGuard<'a> {
    directory: &'a StateDirectory,
    names: Vec<String>,
    armed: bool,
}

impl<'a> StagingGuard<'a> {
    fn new<const N: usize>(directory: &'a StateDirectory, names: [&str; N]) -> Self {
        Self {
            directory,
            names: names.into_iter().map(str::to_owned).collect(),
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StagingGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            for name in &self.names {
                self.directory.remove(name);
            }
        }
    }
}
