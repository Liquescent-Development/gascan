use super::{
    FileIdentity, ManagedSshDiagnostic, ManagedSshDiagnosticKind, PRIVATE_MODE, PUBLIC_MODE,
    SshError, SshPaths, StateDirectory, maximum_managed_file_bytes, random_staging_name,
};
use base64::Engine as _;
use camino::{Utf8Path, Utf8PathBuf};
use command_fds::{CommandFdExt, FdMapping};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

const PRIVATE_KEY_NAME: &str = "identity_ed25519";
const PUBLIC_KEY_NAME: &str = "identity_ed25519.pub";
const SSH_KEYGEN: &str = "/usr/bin/ssh-keygen";
const KEYGEN_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_SUBPROCESS_OUTPUT: usize = 16 * 1024;

/// A validated managed SSH identity.
///
/// Its metadata can only be constructed by validating the managed on-disk
/// identity pair.
///
/// ```compile_fail
/// use gascand::HostIdentity;
///
/// let _forged = HostIdentity {
///     private_key: Default::default(),
///     public_key: String::from("ssh-ed25519 forged"),
///     fingerprint: String::from("SHA256:forged"),
/// };
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostIdentity {
    private_key: Utf8PathBuf,
    public_key: String,
    fingerprint: String,
}

impl HostIdentity {
    #[must_use]
    pub fn private_key(&self) -> &Utf8Path {
        &self.private_key
    }

    #[must_use]
    pub fn public_key(&self) -> &str {
        &self.public_key
    }

    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
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

pub(crate) async fn load_host_identity(paths: &SshPaths) -> Result<HostIdentity, SshError> {
    let directory = StateDirectory::open(paths)?;
    match (
        directory.metadata(PRIVATE_KEY_NAME, PRIVATE_MODE)?,
        directory.metadata(PUBLIC_KEY_NAME, PUBLIC_MODE)?,
    ) {
        (Some(_), Some(_)) => load_existing(&directory, paths).await,
        (None, None) => Err(SshError::InvalidState("managed SSH identity is missing")),
        _ => Err(SshError::InvalidState("managed SSH identity is incomplete")),
    }
}

pub(crate) async fn inspect_host_identity_if_present(
    paths: &SshPaths,
) -> Result<Option<HostIdentity>, ManagedSshDiagnostic<SshError>> {
    let Some(directory) = StateDirectory::open_existing_inspected(paths)? else {
        return Ok(None);
    };
    let private = directory.metadata_inspected(PRIVATE_KEY_NAME, PRIVATE_MODE)?;
    let public = directory.metadata_inspected(PUBLIC_KEY_NAME, PUBLIC_MODE)?;
    match (private, public) {
        (Some(_), Some(_)) => load_existing(&directory, paths)
            .await
            .map(Some)
            .map_err(|error| {
                let kind = if matches!(error, SshError::Io { .. }) {
                    ManagedSshDiagnosticKind::Internal
                } else {
                    ManagedSshDiagnosticKind::Inconsistent
                };
                ManagedSshDiagnostic::new(kind, paths.public_key.clone(), error)
            }),
        (None, None) => Ok(None),
        (None, Some(_)) => Err(ManagedSshDiagnostic::new(
            ManagedSshDiagnosticKind::Missing,
            paths.private_key.clone(),
            SshError::InvalidState("managed SSH identity is incomplete"),
        )),
        (Some(_), None) => Err(ManagedSshDiagnostic::new(
            ManagedSshDiagnosticKind::Missing,
            paths.public_key.clone(),
            SshError::InvalidState("managed SSH identity is incomplete"),
        )),
    }
}

pub(crate) fn open_revalidated_identity(
    paths: &SshPaths,
    identity: &HostIdentity,
) -> Result<StateDirectory, SshError> {
    let validated = std::thread::scope(|scope| {
        scope
            .spawn(|| -> Result<StateDirectory, SshError> {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| SshError::io("start SSH identity validation worker", error))?;
                runtime.block_on(open_revalidated_identity_async(paths, identity))
            })
            .join()
    })
    .map_err(|_| SshError::InvalidState("managed SSH identity validation worker failed"))??;
    Ok(validated)
}

pub(crate) async fn open_revalidated_identity_async(
    paths: &SshPaths,
    identity: &HostIdentity,
) -> Result<StateDirectory, SshError> {
    if identity.private_key != paths.private_key {
        return Err(SshError::InvalidState(
            "SSH config identity is outside managed state",
        ));
    }
    let directory = StateDirectory::open(paths)?;
    let parsed = validate_pair(&directory, PRIVATE_KEY_NAME, PUBLIC_KEY_NAME).await?;
    if parsed.normalized != identity.public_key || parsed.fingerprint != identity.fingerprint {
        return Err(SshError::InvalidState(
            "managed SSH identity changed after validation",
        ));
    }
    Ok(directory)
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
    validate_pair_with(directory, private_name, public_name, || Ok(())).await
}

async fn validate_pair_with<F>(
    directory: &StateDirectory,
    private_name: &str,
    public_name: &str,
    after_open: F,
) -> Result<ParsedPublicKey, SshError>
where
    F: FnOnce() -> Result<(), SshError>,
{
    validate_pair_with_spawn_hook(directory, private_name, public_name, after_open, |_| Ok(()))
        .await
}

async fn validate_pair_with_spawn_hook<F, G>(
    directory: &StateDirectory,
    private_name: &str,
    public_name: &str,
    after_open: F,
    before_spawn: G,
) -> Result<ParsedPublicKey, SshError>
where
    F: FnOnce() -> Result<(), SshError>,
    G: FnOnce(i32) -> Result<(), SshError>,
{
    let (private_file, private_identity) = directory.open_file(private_name, PRIVATE_MODE)?;
    let (public_bytes, public_identity) = directory.read_file(
        public_name,
        PUBLIC_MODE,
        maximum_managed_file_bytes().min(MAX_SUBPROCESS_OUTPUT as u64),
    )?;
    let stored_public = parse_public_key(&public_bytes)?;

    after_open()?;
    let derived = derive_public_key_with_spawn_hook(&private_file, before_spawn).await?;
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

async fn derive_public_key_with_spawn_hook<F>(
    private_file: &File,
    before_spawn: F,
) -> Result<Vec<u8>, SshError>
where
    F: FnOnce(i32) -> Result<(), SshError>,
{
    let inherited = rustix::io::fcntl_dupfd_cloexec(private_file, 3)
        .map_err(|error| SshError::io("duplicate managed SSH private descriptor", error))?;
    let parent_fd = inherited.as_raw_fd();
    let descriptor_path = format!("/dev/fd/{parent_fd}");
    let mut command = ssh_keygen_command(vec![
        OsString::from("-y"),
        OsString::from("-f"),
        OsString::from(descriptor_path),
    ]);
    command
        .as_std_mut()
        .fd_mappings(vec![FdMapping {
            parent_fd: inherited,
            child_fd: parent_fd,
        }])
        .map_err(|error| {
            SshError::io(
                "configure managed SSH private descriptor mapping",
                std::io::Error::other(error),
            )
        })?;
    before_spawn(parent_fd)?;
    run_configured_ssh_keygen(command).await
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
    run_configured_ssh_keygen(ssh_keygen_command(args)).await
}

fn ssh_keygen_command(args: Vec<OsString>) -> Command {
    let mut child = Command::new(SSH_KEYGEN);
    child
        .args(args)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    child
}

async fn run_configured_ssh_keygen(mut command: Command) -> Result<Vec<u8>, SshError> {
    let mut child = command
        .spawn()
        .map_err(|error| SshError::io("start bounded ssh-keygen", error))?;
    drop(command);
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
        #[cfg(test)]
        eprintln!(
            "ssh-keygen rejection: code={:?} stdout_bytes={} stderr_bytes={} stderr_sha256={:x}",
            status.code(),
            stdout.len(),
            stderr.len(),
            Sha256::digest(&stderr)
        );
        // The outcome travels with the error rather than only through the
        // `#[cfg(test)]` line above, which cannot reach a `gascand` that an
        // end-to-end test spawned as a real binary.
        use std::os::unix::process::ExitStatusExt as _;
        let outcome = match (status.code(), status.signal()) {
            (Some(code), _) => crate::ssh::KeygenOutcome::Code(code),
            (None, Some(signal)) => crate::ssh::KeygenOutcome::Signal(signal),
            (None, None) => crate::ssh::KeygenOutcome::NoStatus,
        };
        return Err(SshError::KeygenRejected(outcome));
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

#[cfg(test)]
mod tests {
    use super::{
        PRIVATE_KEY_NAME, PUBLIC_KEY_NAME, SshError, ensure_host_identity,
        validate_pair_with_spawn_hook,
    };
    use crate::ssh::SshPaths;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};

    #[tokio::test]
    async fn private_key_descriptor_is_inherited_only_by_the_intended_child()
    -> Result<(), Box<dyn std::error::Error>> {
        let managed = tempfile::tempdir()?;
        let managed_home = managed.path().canonicalize()?;
        let paths = SshPaths::for_environment(None, Some(managed_home.as_os_str()))?;
        ensure_host_identity(&paths)
            .await
            .map_err(|error| format!("prepare managed identity: {error}"))?;

        let replacement = tempfile::tempdir()?;
        let replacement_home = replacement.path().canonicalize()?;
        let replacement_paths =
            SshPaths::for_environment(None, Some(replacement_home.as_os_str()))?;
        ensure_host_identity(&replacement_paths)
            .await
            .map_err(|error| format!("prepare replacement identity: {error}"))?;
        let replacement_private = fs::read(replacement_paths.private_key().as_std_path())?;

        let directory = crate::ssh::StateDirectory::open(&paths)?;
        let displaced = paths.directory().join("identity_ed25519.displaced");
        let result = validate_pair_with_spawn_hook(
            &directory,
            PRIVATE_KEY_NAME,
            PUBLIC_KEY_NAME,
            || -> Result<(), SshError> {
                fs::rename(paths.private_key().as_std_path(), displaced.as_std_path())
                    .map_err(|error| SshError::io("replace managed private key pathname", error))?;
                fs::write(paths.private_key().as_std_path(), &replacement_private)
                    .map_err(|error| SshError::io("write replacement private key", error))?;
                fs::set_permissions(
                    paths.private_key().as_std_path(),
                    fs::Permissions::from_mode(0o600),
                )
                .map_err(|error| SshError::io("secure replacement private key", error))
            },
            |parent_fd| {
                let output = Command::new("/bin/cat")
                    .arg(format!("/dev/fd/{parent_fd}"))
                    .env_clear()
                    .stdin(Stdio::null())
                    .stderr(Stdio::null())
                    .output()
                    .map_err(|error| SshError::io("spawn unrelated descriptor probe", error))?;
                if output.status.success() || !output.stdout.is_empty() {
                    return Err(SshError::InvalidState(
                        "unrelated child inherited managed SSH private descriptor",
                    ));
                }
                Ok(())
            },
        )
        .await;
        let error = match result {
            Ok(_) => {
                return Err(
                    "pathname replacement unexpectedly passed descriptor validation".into(),
                );
            }
            Err(error) => error,
        };

        assert!(
            matches!(
                &error,
                SshError::InvalidState("managed SSH file changed during validation")
            ),
            "intended descriptor validation returned an unexpected error: {error}"
        );
        Ok(())
    }
}
