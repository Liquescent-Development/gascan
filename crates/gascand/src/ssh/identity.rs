use super::{
    FileIdentity, KeygenMessage, KeygenOutcome, KeygenRejection, ManagedSshDiagnostic,
    ManagedSshDiagnosticKind, MappedDescriptor, PRIVATE_MODE, PUBLIC_MODE, SshError, SshPaths,
    StateDirectory, maximum_managed_file_bytes, random_staging_name,
};
use base64::Engine as _;
use camino::{Utf8Path, Utf8PathBuf};
use command_fds::{CommandFdExt, FdMapping};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs::File;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::MetadataExt as _;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

const PRIVATE_KEY_NAME: &str = "identity_ed25519";
const PUBLIC_KEY_NAME: &str = "identity_ed25519.pub";
const SSH_KEYGEN: &str = "/usr/bin/ssh-keygen";
const KEYGEN_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_SUBPROCESS_OUTPUT: usize = 16 * 1024;
/// The lowest descriptor number the private key's duplicate may occupy, keeping
/// it clear of the standard streams.
const LOWEST_PRIVATE_FD: RawFd = 3;

/// The two numbers a mapped descriptor has: the one this process holds, and the
/// one the child is told to read from.
///
/// They are currently the same number, and that is the crux of the outstanding
/// `Bad file descriptor` rejection: when they are equal, `command_fds` only
/// clears `FD_CLOEXEC`, so the child depends on the parent's allocation
/// surviving `exec` rather than on a descriptor actively installed for it.
/// Naming them separately is what lets a diagnostic say which one it means.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DescriptorMapping {
    parent_fd: RawFd,
    child_fd: RawFd,
}

impl DescriptorMapping {
    pub(crate) const fn parent_fd(self) -> RawFd {
        self.parent_fd
    }

    pub(crate) const fn child_fd(self) -> RawFd {
        self.child_fd
    }
}

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
    let output = KeygenInvocation::new(
        vec![
            OsString::from("-q"),
            OsString::from("-t"),
            OsString::from("ed25519"),
            OsString::from("-N"),
            OsString::new(),
            OsString::from("-C"),
            OsString::from("gascan-managed"),
            OsString::from("-f"),
            output_path.as_os_str().to_owned(),
        ],
        // Lossy on both sides: `KeygenMessage` decodes the child's stderr the
        // same way, so a non-UTF-8 pathname still matches itself.
        vec![output_path.to_string_lossy().into_owned()],
    )
    .run()
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
    G: FnOnce(DescriptorMapping) -> Result<(), SshError>,
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
    F: FnOnce(DescriptorMapping) -> Result<(), SshError>,
{
    // The child is given the parent's own descriptor number, so `command_fds`
    // maps it by clearing `FD_CLOEXEC` rather than by `dup2`.
    //
    // Pinning the child to a fixed low number instead was tried and MEASURED
    // WORSE: mapping to child descriptor 3 failed 6 times in 28 amplifier runs
    // under load, against 0 in 28 for this scheme, on the same machine
    // back to back. Do not "fix" this by choosing the child's number again
    // without re-running that comparison.
    let inherited = rustix::io::fcntl_dupfd_cloexec(private_file, LOWEST_PRIVATE_FD)
        .map_err(|error| SshError::io("duplicate managed SSH private descriptor", error))?;
    let mapping = DescriptorMapping {
        parent_fd: inherited.as_raw_fd(),
        child_fd: inherited.as_raw_fd(),
    };
    let parent_path = format!("/dev/fd/{}", mapping.parent_fd());
    let child_path = format!("/dev/fd/{}", mapping.child_fd());
    // The descriptor pathname is not secret -- it is a small integer -- and it
    // is exactly the discriminator between "the descriptor was missing" and
    // "the bytes behind it were not a key", so it is deliberately not redacted.
    let mut invocation = KeygenInvocation::new(
        vec![
            OsString::from("-y"),
            OsString::from("-f"),
            OsString::from(child_path),
        ],
        Vec::new(),
    );
    invocation
        .command_mut()
        .as_std_mut()
        .fd_mappings(vec![FdMapping {
            parent_fd: inherited,
            child_fd: mapping.child_fd(),
        }])
        .map_err(|error| {
            SshError::io(
                "configure managed SSH private descriptor mapping",
                std::io::Error::other(error),
            )
        })?;
    // Watches the parent's number, not the child's: a parent number that stops
    // referring to the private key is the one thing that would explain the
    // child being handed the wrong descriptor.
    invocation.watch_descriptor(DescriptorWitness::record(private_file, &parent_path)?);
    before_spawn(mapping)?;
    invocation.run().await
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

/// One `ssh-keygen` run together with the argument strings that must not appear
/// in a diagnostic.
///
/// Both live in a single value so that adding a pathname argument without also
/// registering it for redaction is not expressible.
struct KeygenInvocation {
    command: Command,
    sensitive: Vec<String>,
    witness: Option<DescriptorWitness>,
}

/// The identity a mapped descriptor number was expected to keep.
///
/// Recorded from the file itself, then re-read from the bare number once the
/// child exists. Only another part of this process closing and reusing the
/// number can make the two disagree, so a disagreement names the culprit.
struct DescriptorWitness {
    path: String,
    inode: u64,
}

impl DescriptorWitness {
    fn record(file: &File, path: &str) -> Result<Self, SshError> {
        let stat = rustix::fs::fstat(file)
            .map_err(|error| SshError::io("inspect managed SSH private descriptor", error))?;
        Ok(Self {
            path: path.to_owned(),
            inode: stat.st_ino,
        })
    }

    /// Resolves the same `/dev/fd` pathname the child was given, so the parent's
    /// answer and the child's are about the same thing.
    ///
    /// Compares the inode only. Darwin's `fdesc` filesystem reports the real
    /// inode through `/dev/fd/<N>` but substitutes its own `st_dev`, so
    /// comparing the device would report every descriptor as replaced --
    /// measured: `stat` of a file directly and through its own `/dev/fd` entry
    /// agree on `st_ino` and disagree on `st_dev`.
    fn observe(&self) -> MappedDescriptor {
        match std::fs::metadata(&self.path) {
            Err(_) => MappedDescriptor::Closed,
            Ok(metadata) if metadata.ino() == self.inode => MappedDescriptor::Intact,
            Ok(_) => MappedDescriptor::Replaced,
        }
    }
}

impl KeygenInvocation {
    fn new(args: Vec<OsString>, sensitive: Vec<String>) -> Self {
        let mut command = Command::new(SSH_KEYGEN);
        command
            .args(args)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        Self {
            command,
            sensitive,
            witness: None,
        }
    }

    fn command_mut(&mut self) -> &mut Command {
        &mut self.command
    }

    fn watch_descriptor(&mut self, witness: DescriptorWitness) {
        self.witness = Some(witness);
    }

    async fn run(mut self) -> Result<Vec<u8>, SshError> {
        let mut child = self
            .command
            .spawn()
            .map_err(|error| SshError::io("start bounded ssh-keygen", error))?;
        // Observed here, while the mapping still owns the number and the child
        // has just been forked: this is the last instant at which the parent's
        // view can still explain the child's.
        let descriptor = self
            .witness
            .as_ref()
            .map_or(MappedDescriptor::None, DescriptorWitness::observe);
        drop(self.command);
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
            // Status and message both travel with the error. Neither reaches a
            // `gascand` spawned as a real binary any other way, and exit 255 is
            // shared by every argument-level refusal, so the status alone
            // cannot say which one happened.
            use std::os::unix::process::ExitStatusExt as _;
            let outcome = match (status.code(), status.signal()) {
                (Some(code), _) => KeygenOutcome::Code(code),
                (None, Some(signal)) => KeygenOutcome::Signal(signal),
                (None, None) => KeygenOutcome::NoStatus,
            };
            return Err(SshError::KeygenRejected(KeygenRejection::new(
                outcome,
                KeygenMessage::redacted(&stderr, &self.sensitive),
                descriptor,
            )));
        }
        Ok(stdout)
    }
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
        KeygenInvocation, PRIVATE_KEY_NAME, PUBLIC_KEY_NAME, SshError, ensure_host_identity,
        validate_pair_with_spawn_hook,
    };
    use crate::ssh::{KeygenOutcome, SshPaths};
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};

    async fn rejection_of(
        target: &str,
        sensitive: Vec<String>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let result = KeygenInvocation::new(
            vec![
                OsString::from("-y"),
                OsString::from("-f"),
                OsString::from(target),
            ],
            sensitive,
        )
        .run()
        .await;
        let error = match result {
            Ok(_) => return Err("ssh-keygen accepted a target that is not a private key".into()),
            Err(error) => error,
        };
        let SshError::KeygenRejected(rejection) = error else {
            return Err(format!("unexpected error from ssh-keygen: {error}").into());
        };
        assert_eq!(
            rejection.outcome(),
            KeygenOutcome::Code(255),
            "ssh-keygen argument rejection changed exit status: {rejection}"
        );
        Ok(rejection.message().as_str().to_owned())
    }

    /// Both candidate causes of the observed `KeygenRejected(Code(255))` exit
    /// with the same status; only stderr tells them apart. This pins that the
    /// message survives to the error, and that the two remain distinguishable.
    #[tokio::test]
    async fn keygen_rejection_separates_a_missing_descriptor_from_unreadable_bytes()
    -> Result<(), Box<dyn std::error::Error>> {
        let absent = rejection_of("/dev/fd/999", Vec::new()).await?;
        let unreadable = rejection_of("/dev/null", Vec::new()).await?;
        assert!(
            absent.contains("Bad file descriptor"),
            "absent descriptor did not name itself: {absent}"
        );
        assert!(
            unreadable.contains("invalid format"),
            "unreadable key did not name itself: {unreadable}"
        );
        assert_ne!(absent, unreadable);
        Ok(())
    }

    #[tokio::test]
    async fn keygen_message_replaces_the_pathname_it_was_given()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let target = directory.path().join("not-a-key");
        fs::write(&target, b"not a private key\n")?;
        // 0644 makes `ssh-keygen` emit its multi-line unprotected-key banner
        // instead of the parse failure this test is about.
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600))?;
        let path = target.to_string_lossy().into_owned();
        let message = rejection_of(&path, vec![path.clone()]).await?;
        assert!(
            !message.contains(&path),
            "pathname survived redaction: {message}"
        );
        assert!(
            message.contains("<path>") && message.contains("invalid format"),
            "redacted message lost its diagnostic: {message}"
        );
        Ok(())
    }

    /// The child must read the key from a descriptor this code installed at a
    /// number the parent also holds, and never one of the standard streams.
    ///
    /// This records the scheme that MEASURED BETTER, not an ideal. Giving the
    /// child a number of our own choosing is the obvious-looking alternative and
    /// it lost the comparison badly (6 failures in 28 under load, against 0 in
    /// 28 here). If this assertion is ever changed, the comparison has to be
    /// re-run rather than reasoned about.
    #[tokio::test]
    async fn the_child_is_given_the_descriptor_number_the_parent_holds()
    -> Result<(), Box<dyn std::error::Error>> {
        let managed = tempfile::tempdir()?;
        let managed_home = managed.path().canonicalize()?;
        let paths = SshPaths::for_environment(None, Some(managed_home.as_os_str()))?;
        let expected = ensure_host_identity(&paths).await?;

        let directory = crate::ssh::StateDirectory::open(&paths)?;
        let parsed = validate_pair_with_spawn_hook(
            &directory,
            PRIVATE_KEY_NAME,
            PUBLIC_KEY_NAME,
            || Ok(()),
            |mapping| {
                assert_eq!(
                    mapping.parent_fd(),
                    mapping.child_fd(),
                    "the child was given a descriptor number the parent does not hold"
                );
                assert!(
                    mapping.child_fd() >= super::LOWEST_PRIVATE_FD,
                    "the private descriptor landed on a standard stream: {}",
                    mapping.child_fd()
                );
                Ok(())
            },
        )
        .await?;
        assert_eq!(parsed.normalized, expected.public_key());
        Ok(())
    }

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
            |mapping| {
                let parent_fd = mapping.parent_fd();
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
