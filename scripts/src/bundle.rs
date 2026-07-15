use std::{
    collections::BTreeSet,
    fs::{File, Metadata},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Component, Path},
    sync::atomic::{AtomicU64, Ordering},
};

use cap_primitives::fs::{
    self as cap_fs, DirOptions, FollowSymlinks, MetadataExt as CapMetadataExt, OpenOptions,
};
use cap_std::{ambient_authority, fs::Dir};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const BUNDLE_MEDIA_TYPE: &str = "application/vnd.gascan.workspace-bundle.v1+tar.zstd";
pub const BUNDLE_PLATFORM: &str = "linux/arm64";
const MANIFEST_PATH: &str = "bundle-manifest.json";
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 1_000_000;
const MAX_EXPANDED_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_OVERHEAD: u64 = (MAX_ENTRIES as u64 + 2) * 512;
static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BundleLock {
    pub url: String,
    pub sha256: String,
    pub size: u64,
    pub media_type: String,
    pub platform: String,
}

impl BundleLock {
    fn is_valid(&self) -> bool {
        self.url.starts_with("https://")
            && is_lower_sha256(&self.sha256)
            && self.size > 0
            && self.media_type == BUNDLE_MEDIA_TYPE
            && self.platform == BUNDLE_PLATFORM
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedBundleLocks {
    pub ubuntu_packages: BundleLock,
    pub mise_runtimes: BundleLock,
    pub gascamp_source_vendor: BundleLock,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingBundleContract {
    pub media_type: String,
    pub platform: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BundlePublication {
    Pending(PendingBundleContract),
    Published(Box<PublishedBundleLocks>),
}

#[derive(Deserialize)]
struct LockDocument {
    workspace_bundles: WorkspaceBundleSection,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceBundleSection {
    media_type: Option<String>,
    platform: Option<String>,
    publication: Option<String>,
    ubuntu_packages: Option<BundleLock>,
    mise_runtimes: Option<BundleLock>,
    gascamp_source_vendor: Option<BundleLock>,
}

impl BundlePublication {
    pub fn from_toml(contents: &str) -> Result<Self, BundleError> {
        let section = parse_lock_section(contents)?;
        validate_contract(&section)?;
        match section.publication.as_deref() {
            Some("pending") => Ok(Self::Pending(PendingBundleContract {
                media_type: BUNDLE_MEDIA_TYPE.to_owned(),
                platform: BUNDLE_PLATFORM.to_owned(),
            })),
            Some("published") => published_from_section(section)
                .map(Box::new)
                .map(Self::Published),
            _ => Err(BundleError::InvalidPublicationState),
        }
    }
}

impl PublishedBundleLocks {
    pub fn from_toml(contents: &str) -> Result<Self, BundleError> {
        let section = parse_lock_section(contents)?;
        validate_contract(&section)?;
        if section.publication.as_deref() != Some("published") {
            return Err(BundleError::InvalidPublicationState);
        }
        published_from_section(section)
    }

    pub fn named(&self, name: &str) -> Result<&BundleLock, BundleError> {
        match name {
            "ubuntu_packages" => Ok(&self.ubuntu_packages),
            "mise_runtimes" => Ok(&self.mise_runtimes),
            "gascamp_source_vendor" => Ok(&self.gascamp_source_vendor),
            _ => Err(BundleError::UnknownLockRecord(name.to_owned())),
        }
    }
}

fn parse_lock_section(contents: &str) -> Result<WorkspaceBundleSection, BundleError> {
    let document: LockDocument =
        toml::from_str(contents).map_err(|error| BundleError::LockFormat(error.to_string()))?;
    Ok(document.workspace_bundles)
}

fn validate_contract(section: &WorkspaceBundleSection) -> Result<(), BundleError> {
    if section.media_type.as_deref() != Some(BUNDLE_MEDIA_TYPE)
        || section.platform.as_deref() != Some(BUNDLE_PLATFORM)
    {
        return Err(BundleError::InvalidLockRecord("workspace_bundles"));
    }
    Ok(())
}

fn published_from_section(
    section: WorkspaceBundleSection,
) -> Result<PublishedBundleLocks, BundleError> {
    let ubuntu_packages = section
        .ubuntu_packages
        .ok_or(BundleError::MissingLockRecord("ubuntu_packages"))?;
    let mise_runtimes = section
        .mise_runtimes
        .ok_or(BundleError::MissingLockRecord("mise_runtimes"))?;
    let gascamp_source_vendor = section
        .gascamp_source_vendor
        .ok_or(BundleError::MissingLockRecord("gascamp_source_vendor"))?;
    for (name, lock) in [
        ("ubuntu_packages", &ubuntu_packages),
        ("mise_runtimes", &mise_runtimes),
        ("gascamp_source_vendor", &gascamp_source_vendor),
    ] {
        if !lock.is_valid() {
            return Err(BundleError::InvalidLockRecord(name));
        }
    }
    Ok(PublishedBundleLocks {
        ubuntu_packages,
        mise_runtimes,
        gascamp_source_vendor,
    })
}

#[derive(Debug, Eq, PartialEq, Error)]
pub enum BundleError {
    #[error("bundle lock is missing required record {0}")]
    MissingLockRecord(&'static str),
    #[error("bundle lock record {0} is invalid")]
    InvalidLockRecord(&'static str),
    #[error("bundle publication state is not published")]
    InvalidPublicationState,
    #[error("unknown bundle lock record {0}")]
    UnknownLockRecord(String),
    #[error("could not parse bundle lock: {0}")]
    LockFormat(String),
    #[error("bundle archive is not a regular file")]
    ArchiveNotRegular,
    #[error("bundle archive changed while it was being validated")]
    ArchiveChanged,
    #[error("bundle archive byte size does not match lock")]
    ArchiveSizeMismatch,
    #[error("bundle archive SHA-256 does not match lock")]
    ArchiveHashMismatch,
    #[error("bundle archive is invalid: {0}")]
    Archive(String),
    #[error("bundle archive contains bytes after its canonical terminator")]
    TrailingArchiveData,
    #[error("bundle-manifest.json must be the first archive entry")]
    ManifestMustBeFirst,
    #[error("bundle manifest is invalid: {0}")]
    Manifest(String),
    #[error("bundle manifest is not canonically sorted and unique")]
    NonCanonicalManifest,
    #[error("unsafe bundle path: {0}")]
    UnsafePath(String),
    #[error("duplicate archive entry: {0}")]
    DuplicateArchiveEntry(String),
    #[error("unsupported archive entry type: {0}")]
    UnsupportedEntryType(String),
    #[error("unsafe symlink target for: {0}")]
    UnsafeLink(String),
    #[error("symlink entry is an ancestor of another entry: {0}")]
    SymlinkAncestor(String),
    #[error("unexpected archive entry: {0}")]
    UnexpectedArchiveEntry(String),
    #[error("manifested archive entry is missing: {0}")]
    MissingArchiveEntry(String),
    #[error("archive entry type differs from manifest: {0}")]
    EntryTypeMismatch(String),
    #[error("archive file size differs from manifest: {0}")]
    FileSizeMismatch(String),
    #[error("archive file SHA-256 differs from manifest: {0}")]
    FileHashMismatch(String),
    #[error("destination already exists")]
    DestinationExists,
    #[error("staging directory identity changed")]
    StagingChanged,
    #[error("destination parent directory identity changed")]
    ParentChanged,
    #[error("bundle I/O failed: {0}")]
    Io(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BundleEvidence {
    pub archive_sha256: String,
    pub archive_size: u64,
    pub entries: usize,
    pub regular_files: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleManifest {
    version: u32,
    platform: String,
    files: Vec<ManifestEntry>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
enum ManifestEntry {
    File {
        path: String,
        size: u64,
        sha256: String,
    },
    Directory {
        path: String,
    },
    Symlink {
        path: String,
        target: String,
    },
}

impl ManifestEntry {
    fn path(&self) -> &str {
        match self {
            Self::File { path, .. } | Self::Directory { path } | Self::Symlink { path, .. } => path,
        }
    }
}

struct ValidatedArchive {
    manifest: BundleManifest,
    entries: usize,
    regular_files: usize,
}

trait ValidationHooks {
    fn after_archive_hash(&mut self, _archive_path: &Path) -> Result<(), BundleError> {
        Ok(())
    }

    fn after_staging_open(
        &mut self,
        _parent_path: &Path,
        _staging_name: &str,
    ) -> Result<(), BundleError> {
        Ok(())
    }

    fn before_publish(
        &mut self,
        _parent_path: &Path,
        _staging_name: &str,
    ) -> Result<(), BundleError> {
        Ok(())
    }
}

struct NoHooks;
impl ValidationHooks for NoHooks {}

pub fn validate_bundle(
    lock: &BundleLock,
    archive: &Path,
    destination: &Path,
) -> Result<BundleEvidence, BundleError> {
    validate_bundle_inner(lock, archive, destination, &mut NoHooks)
}

fn validate_bundle_inner(
    lock: &BundleLock,
    archive_path: &Path,
    destination: &Path,
    hooks: &mut dyn ValidationHooks,
) -> Result<BundleEvidence, BundleError> {
    if !lock.is_valid() {
        return Err(BundleError::InvalidLockRecord("bundle"));
    }
    let mut archive = File::open(archive_path).map_err(io_error)?;
    let initial = FileSnapshot::capture(&archive)?;
    if !initial.is_regular {
        return Err(BundleError::ArchiveNotRegular);
    }
    let (archive_size, archive_sha256) = hash_from_start(&mut archive)?;
    if archive_size != lock.size {
        return Err(BundleError::ArchiveSizeMismatch);
    }
    if archive_sha256 != lock.sha256 {
        return Err(BundleError::ArchiveHashMismatch);
    }
    hooks.after_archive_hash(archive_path)?;
    initial.require_unchanged(&archive)?;

    archive.seek(SeekFrom::Start(0)).map_err(io_error)?;
    let (second_size, second_hash) = hash_from_current(&mut archive)?;
    if second_size != archive_size || second_hash != archive_sha256 {
        return Err(BundleError::ArchiveChanged);
    }
    initial.require_unchanged(&archive)?;

    archive.seek(SeekFrom::Start(0)).map_err(io_error)?;
    let mut expanded = decompress_same_handle(&mut archive, archive_size, &archive_sha256)?;
    initial.require_unchanged(&archive)?;
    validate_tar_layout(&mut expanded)?;
    expanded.seek(SeekFrom::Start(0)).map_err(io_error)?;
    let validated = validate_expanded_archive(&mut expanded)?;

    let parent_path = destination
        .parent()
        .ok_or_else(|| BundleError::Io("bundle destination has no parent directory".to_owned()))?;
    let destination_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| BundleError::UnsafePath(destination.display().to_string()))?;
    validate_single_name(destination_name)?;
    let parent = Dir::open_ambient_dir(parent_path, ambient_authority())
        .map_err(io_error)?
        .into_std_file();
    let parent_identity = DirectoryIdentity::capture(&parent)?;
    ensure_absent(&parent, destination_name)?;
    let (staging_name, staging) = create_staging(&parent)?;
    hooks.after_staging_open(parent_path, &staging_name)?;

    let extraction = (|| {
        expanded.seek(SeekFrom::Start(0)).map_err(io_error)?;
        extract_validated(&mut expanded, &validated.manifest, &staging)?;
        hooks.before_publish(parent_path, &staging_name)?;
        parent_identity.require_path(parent_path)?;
        require_same_staging(&parent, &staging_name, &staging)?;
        ensure_absent(&parent, destination_name)?;
        publish_noreplace(&parent, &staging_name, destination_name)
    })();
    if extraction.is_err() && require_same_staging(&parent, &staging_name, &staging).is_ok() {
        let _ignored = cap_fs::remove_dir_all(&parent, Path::new(&staging_name));
    }
    extraction?;
    Ok(BundleEvidence {
        archive_sha256,
        archive_size,
        entries: validated.entries,
        regular_files: validated.regular_files,
    })
}

#[derive(Clone, Eq, PartialEq)]
struct DirectoryIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl DirectoryIdentity {
    fn capture(directory: &File) -> Result<Self, BundleError> {
        let metadata = directory.metadata().map_err(io_error)?;
        if !metadata.is_dir() {
            return Err(BundleError::ParentChanged);
        }
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        })
    }

    fn require_path(&self, path: &Path) -> Result<(), BundleError> {
        let reopened = Dir::open_ambient_dir(path, ambient_authority())
            .map_err(|_| BundleError::ParentChanged)?
            .into_std_file();
        if *self != Self::capture(&reopened)? {
            return Err(BundleError::ParentChanged);
        }
        Ok(())
    }
}

#[derive(Clone, Eq, PartialEq)]
struct FileSnapshot {
    is_regular: bool,
    len: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl FileSnapshot {
    fn capture(file: &File) -> Result<Self, BundleError> {
        let metadata = file.metadata().map_err(io_error)?;
        Ok(Self::from_metadata(&metadata))
    }

    fn from_metadata(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            is_regular: metadata.file_type().is_file(),
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        }
    }

    fn require_unchanged(&self, file: &File) -> Result<(), BundleError> {
        if *self != Self::capture(file)? {
            return Err(BundleError::ArchiveChanged);
        }
        Ok(())
    }
}

fn hash_from_start(file: &mut File) -> Result<(u64, String), BundleError> {
    file.seek(SeekFrom::Start(0)).map_err(io_error)?;
    hash_from_current(file)
}

fn hash_from_current(reader: &mut impl Read) -> Result<(u64, String), BundleError> {
    let mut hasher = Sha256::new();
    let size = io::copy(reader, &mut hasher).map_err(io_error)?;
    Ok((size, format!("{:x}", hasher.finalize())))
}

fn decompress_same_handle(
    archive: &mut File,
    expected_size: u64,
    expected_hash: &str,
) -> Result<File, BundleError> {
    let mut hashing_reader = HashingReader::new(archive);
    let mut decoder = zstd::stream::read::Decoder::new(&mut hashing_reader)
        .map_err(|error| BundleError::Archive(error.to_string()))?;
    let mut expanded = tempfile::tempfile().map_err(io_error)?;
    let expanded_limit = MAX_EXPANDED_BYTES + MAX_MANIFEST_BYTES + MAX_ARCHIVE_OVERHEAD;
    let copied = io::copy(
        &mut decoder.by_ref().take(expanded_limit + 1),
        &mut expanded,
    )
    .map_err(|error| BundleError::Archive(error.to_string()))?;
    drop(decoder);
    if copied > expanded_limit {
        return Err(BundleError::Archive(
            "expanded archive size limit exceeded".to_owned(),
        ));
    }
    let (actual_size, actual_hash) = hashing_reader.finish();
    if actual_size != expected_size || actual_hash != expected_hash {
        return Err(BundleError::ArchiveChanged);
    }
    expanded.seek(SeekFrom::Start(0)).map_err(io_error)?;
    Ok(expanded)
}

fn validate_tar_layout(expanded: &mut File) -> Result<(), BundleError> {
    let length = expanded.metadata().map_err(io_error)?.len();
    if length < 1024 || length % 512 != 0 {
        return Err(BundleError::Archive("truncated tar archive".to_owned()));
    }
    expanded.seek(SeekFrom::Start(0)).map_err(io_error)?;
    let mut offset = 0_u64;
    let mut header = [0_u8; 512];
    while offset < length {
        expanded.read_exact(&mut header).map_err(io_error)?;
        offset += 512;
        if header.iter().all(|byte| *byte == 0) {
            let mut second = [1_u8; 512];
            expanded.read_exact(&mut second).map_err(io_error)?;
            offset += 512;
            if second.iter().any(|byte| *byte != 0) {
                return Err(BundleError::Archive(
                    "single zero tar terminator block".to_owned(),
                ));
            }
            if offset != length {
                return Err(BundleError::TrailingArchiveData);
            }
            return Ok(());
        }
        let size = parse_tar_octal(&header[124..136])?;
        let padded = size
            .checked_add(511)
            .ok_or_else(|| BundleError::Archive("tar entry size overflow".to_owned()))?
            / 512
            * 512;
        offset = offset
            .checked_add(padded)
            .ok_or_else(|| BundleError::Archive("tar offset overflow".to_owned()))?;
        if offset > length {
            return Err(BundleError::Archive("truncated tar archive".to_owned()));
        }
        expanded.seek(SeekFrom::Start(offset)).map_err(io_error)?;
    }
    Err(BundleError::Archive("missing tar terminator".to_owned()))
}

fn parse_tar_octal(field: &[u8]) -> Result<u64, BundleError> {
    let value = field
        .iter()
        .copied()
        .skip_while(|byte| matches!(byte, b' ' | 0))
        .take_while(|byte| *byte != 0 && *byte != b' ')
        .try_fold(0_u64, |value, byte| {
            if !(b'0'..=b'7').contains(&byte) {
                return None;
            }
            value.checked_mul(8)?.checked_add(u64::from(byte - b'0'))
        })
        .ok_or_else(|| BundleError::Archive("invalid tar size".to_owned()))?;
    Ok(value)
}

fn validate_expanded_archive(expanded: &mut File) -> Result<ValidatedArchive, BundleError> {
    let mut archive = tar::Archive::new(expanded);
    let mut entries = archive
        .entries()
        .map_err(|error| BundleError::Archive(error.to_string()))?;
    let Some(first) = entries.next() else {
        return Err(BundleError::ManifestMustBeFirst);
    };
    let mut first = first.map_err(archive_error)?;
    let first_path = entry_path(&first)?;
    if first_path != MANIFEST_PATH || !first.header().entry_type().is_file() {
        return Err(BundleError::ManifestMustBeFirst);
    }
    if first.size() > MAX_MANIFEST_BYTES {
        return Err(BundleError::Manifest(
            "manifest exceeds size limit".to_owned(),
        ));
    }
    let mut manifest_bytes = Vec::new();
    first
        .read_to_end(&mut manifest_bytes)
        .map_err(archive_error)?;
    let manifest: BundleManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| BundleError::Manifest(error.to_string()))?;
    validate_manifest(&manifest)?;

    let mut seen = BTreeSet::new();
    let mut regular_files = 0;
    for entry in entries {
        let mut entry = entry.map_err(archive_error)?;
        let path = entry_path(&entry)?;
        if !seen.insert(path.clone()) {
            return Err(BundleError::DuplicateArchiveEntry(path));
        }
        if seen.len() > MAX_ENTRIES {
            return Err(BundleError::Archive(
                "archive entry limit exceeded".to_owned(),
            ));
        }
        let Some(expected) = manifest.files.iter().find(|item| item.path() == path) else {
            return Err(BundleError::UnexpectedArchiveEntry(path));
        };
        validate_entry(&mut entry, expected, &mut regular_files)?;
    }
    for expected in &manifest.files {
        if !seen.contains(expected.path()) {
            return Err(BundleError::MissingArchiveEntry(expected.path().to_owned()));
        }
    }
    Ok(ValidatedArchive {
        manifest,
        entries: seen.len(),
        regular_files,
    })
}

fn validate_manifest(manifest: &BundleManifest) -> Result<(), BundleError> {
    if manifest.version != 1 || manifest.platform != BUNDLE_PLATFORM {
        return Err(BundleError::Manifest(
            "unsupported version or platform".to_owned(),
        ));
    }
    if manifest.files.len() > MAX_ENTRIES {
        return Err(BundleError::Manifest(
            "manifest entry limit exceeded".to_owned(),
        ));
    }
    let mut previous: Option<&str> = None;
    let mut expanded = 0_u64;
    for (index, entry) in manifest.files.iter().enumerate() {
        let path = entry.path();
        validate_relative_path(path)?;
        if previous.is_some_and(|previous_path| previous_path >= path) {
            return Err(BundleError::NonCanonicalManifest);
        }
        previous = Some(path);
        match entry {
            ManifestEntry::File { size, sha256, .. } => {
                if !is_lower_sha256(sha256) {
                    return Err(BundleError::Manifest(format!("invalid SHA-256 for {path}")));
                }
                expanded = expanded
                    .checked_add(*size)
                    .ok_or_else(|| BundleError::Manifest("expanded size overflow".to_owned()))?;
            }
            ManifestEntry::Directory { .. } => {}
            ManifestEntry::Symlink { target, .. } => {
                validate_link(path, target)?;
                if manifest.files[index + 1..]
                    .iter()
                    .any(|other| is_descendant(other.path(), path))
                {
                    return Err(BundleError::SymlinkAncestor(path.to_owned()));
                }
            }
        }
    }
    if expanded > MAX_EXPANDED_BYTES {
        return Err(BundleError::Manifest(
            "expanded file size limit exceeded".to_owned(),
        ));
    }
    Ok(())
}

fn is_descendant(candidate: &str, ancestor: &str) -> bool {
    candidate
        .strip_prefix(ancestor)
        .is_some_and(|suffix| suffix.starts_with('/'))
}

fn validate_entry<R: Read>(
    entry: &mut tar::Entry<'_, R>,
    expected: &ManifestEntry,
    regular_files: &mut usize,
) -> Result<(), BundleError> {
    let path = expected.path();
    match expected {
        ManifestEntry::File { size, sha256, .. } => {
            if !entry.header().entry_type().is_file() {
                return unsupported_or_mismatch(entry, path);
            }
            if entry.size() != *size {
                return Err(BundleError::FileSizeMismatch(path.to_owned()));
            }
            let (copied, actual) = hash_from_current(entry)?;
            if copied != *size {
                return Err(BundleError::FileSizeMismatch(path.to_owned()));
            }
            if actual != *sha256 {
                return Err(BundleError::FileHashMismatch(path.to_owned()));
            }
            *regular_files += 1;
        }
        ManifestEntry::Directory { .. } => {
            if !entry.header().entry_type().is_dir() {
                return unsupported_or_mismatch(entry, path);
            }
        }
        ManifestEntry::Symlink { target, .. } => {
            if !entry.header().entry_type().is_symlink() {
                return unsupported_or_mismatch(entry, path);
            }
            let actual = entry
                .link_name()
                .map_err(archive_error)?
                .ok_or_else(|| BundleError::UnsafeLink(path.to_owned()))?;
            if actual.to_str() != Some(target) {
                return Err(BundleError::UnsafeLink(path.to_owned()));
            }
        }
    }
    Ok(())
}

fn extract_validated(
    expanded: &mut File,
    manifest: &BundleManifest,
    root: &File,
) -> Result<(), BundleError> {
    extract_pass(expanded, manifest, root, false)?;
    expanded.seek(SeekFrom::Start(0)).map_err(io_error)?;
    extract_pass(expanded, manifest, root, true)
}

fn extract_pass(
    expanded: &mut File,
    manifest: &BundleManifest,
    root: &File,
    symlinks_only: bool,
) -> Result<(), BundleError> {
    let mut archive = tar::Archive::new(expanded);
    let entries = archive.entries().map_err(archive_error)?;
    for entry in entries.skip(1) {
        let mut entry = entry.map_err(archive_error)?;
        let path = entry_path(&entry)?;
        let expected = manifest
            .files
            .iter()
            .find(|item| item.path() == path)
            .ok_or_else(|| BundleError::UnexpectedArchiveEntry(path.clone()))?;
        match expected {
            ManifestEntry::Symlink { target, .. } if symlinks_only => {
                create_symlink(root, &path, target)?;
            }
            ManifestEntry::Directory { .. } if !symlinks_only => {
                ensure_directory_path(root, Path::new(&path))?;
            }
            ManifestEntry::File { size, .. } if !symlinks_only => {
                write_file(root, Path::new(&path), &mut entry, *size)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn create_staging(parent: &File) -> Result<(String, File), BundleError> {
    for _attempt in 0..128 {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let name = format!(".workspace-bundle-{}-{sequence}", std::process::id());
        match cap_fs::create_dir(parent, Path::new(&name), &DirOptions::new()) {
            Ok(()) => {
                let directory =
                    cap_fs::open_dir_nofollow(parent, Path::new(&name)).map_err(io_error)?;
                require_same_staging(parent, &name, &directory)?;
                return Ok((name, directory));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(io_error(error)),
        }
    }
    Err(BundleError::Io(
        "could not allocate staging directory".to_owned(),
    ))
}

fn ensure_directory_path(root: &File, path: &Path) -> Result<File, BundleError> {
    let mut current = root.try_clone().map_err(io_error)?;
    for component in path.components() {
        let Component::Normal(name) = component else {
            return Err(BundleError::UnsafePath(path.display().to_string()));
        };
        let name = Path::new(name);
        match cap_fs::create_dir(&current, name, &DirOptions::new()) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(io_error(error)),
        }
        current = cap_fs::open_dir_nofollow(&current, name).map_err(io_error)?;
    }
    Ok(current)
}

fn write_file(
    root: &File,
    path: &Path,
    input: &mut impl Read,
    size: u64,
) -> Result<(), BundleError> {
    let parent_path = parent_or_empty(path);
    let parent = ensure_directory_path(root, parent_path)?;
    let name = path
        .file_name()
        .ok_or_else(|| BundleError::UnsafePath(path.display().to_string()))?;
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        ._cap_fs_ext_follow(FollowSymlinks::No);
    let mut output = cap_fs::open(&parent, Path::new(name), &options).map_err(io_error)?;
    let copied = io::copy(&mut input.take(size + 1), &mut output).map_err(io_error)?;
    if copied != size {
        return Err(BundleError::FileSizeMismatch(path.display().to_string()));
    }
    output.flush().map_err(io_error)
}

fn create_symlink(root: &File, path: &str, target: &str) -> Result<(), BundleError> {
    let path = Path::new(path);
    let parent = ensure_directory_path(root, parent_or_empty(path))?;
    let name = path
        .file_name()
        .ok_or_else(|| BundleError::UnsafePath(path.display().to_string()))?;
    cap_fs::symlink(Path::new(target), &parent, Path::new(name)).map_err(io_error)
}

fn parent_or_empty(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) => parent,
        None => Path::new(""),
    }
}

fn ensure_absent(parent: &File, name: &str) -> Result<(), BundleError> {
    match cap_fs::stat(parent, Path::new(name), FollowSymlinks::No) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(BundleError::DestinationExists),
        Err(error) => Err(io_error(error)),
    }
}

fn require_same_staging(parent: &File, name: &str, staging: &File) -> Result<(), BundleError> {
    let named = cap_fs::stat(parent, Path::new(name), FollowSymlinks::No)
        .map_err(|_| BundleError::StagingChanged)?;
    let opened = cap_fs::Metadata::from_file(staging).map_err(io_error)?;
    if !named.is_dir() || named.dev() != opened.dev() || named.ino() != opened.ino() {
        return Err(BundleError::StagingChanged);
    }
    Ok(())
}

#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "redox"))]
fn publish_noreplace(parent: &File, staging: &str, destination: &str) -> Result<(), BundleError> {
    match rustix::fs::renameat_with(
        parent,
        staging,
        parent,
        destination,
        rustix::fs::RenameFlags::NOREPLACE,
    ) {
        Ok(()) => Ok(()),
        Err(error) if error == rustix::io::Errno::EXIST => Err(BundleError::DestinationExists),
        Err(error) => Err(BundleError::Io(error.to_string())),
    }
}

#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "redox")))]
fn publish_noreplace(
    _parent: &File,
    _staging: &str,
    _destination: &str,
) -> Result<(), BundleError> {
    Err(BundleError::Io(
        "atomic no-replace publication is unsupported on this platform".to_owned(),
    ))
}

fn unsupported_or_mismatch<R: Read>(
    entry: &tar::Entry<'_, R>,
    path: &str,
) -> Result<(), BundleError> {
    let kind = entry.header().entry_type();
    if kind.is_hard_link()
        || kind.is_block_special()
        || kind.is_character_special()
        || kind.is_fifo()
        || !(kind.is_file() || kind.is_dir() || kind.is_symlink())
    {
        Err(BundleError::UnsupportedEntryType(path.to_owned()))
    } else {
        Err(BundleError::EntryTypeMismatch(path.to_owned()))
    }
}

fn entry_path<R: Read>(entry: &tar::Entry<'_, R>) -> Result<String, BundleError> {
    let path = entry.path().map_err(archive_error)?;
    let value = path
        .to_str()
        .ok_or_else(|| BundleError::UnsafePath("non-UTF-8 path".to_owned()))?
        .to_owned();
    validate_relative_path(&value)?;
    Ok(value)
}

fn validate_relative_path(value: &str) -> Result<(), BundleError> {
    let path = Path::new(value);
    if value.is_empty()
        || value.ends_with('/')
        || path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || normalized(path) != value
    {
        return Err(BundleError::UnsafePath(value.to_owned()));
    }
    Ok(())
}

fn validate_single_name(value: &str) -> Result<(), BundleError> {
    validate_relative_path(value)?;
    if Path::new(value).components().count() != 1 {
        return Err(BundleError::UnsafePath(value.to_owned()));
    }
    Ok(())
}

fn normalized(path: &Path) -> String {
    path.components()
        .filter_map(|part| match part {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn validate_link(path: &str, target: &str) -> Result<(), BundleError> {
    let target_path = Path::new(target);
    if target.is_empty() || target_path.is_absolute() || target_path.to_str() != Some(target) {
        return Err(BundleError::UnsafeLink(path.to_owned()));
    }
    let mut depth = Path::new(path)
        .parent()
        .map_or(0, |parent| parent.components().count());
    for component in target_path.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir if depth > 0 => depth -= 1,
            _ => return Err(BundleError::UnsafeLink(path.to_owned())),
        }
    }
    Ok(())
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn archive_error(error: io::Error) -> BundleError {
    BundleError::Archive(error.to_string())
}

fn io_error(error: io::Error) -> BundleError {
    BundleError::Io(error.to_string())
}

struct HashingReader<'a, R> {
    input: &'a mut R,
    hasher: Sha256,
    size: u64,
}

impl<'a, R> HashingReader<'a, R> {
    fn new(input: &'a mut R) -> Self {
        Self {
            input,
            hasher: Sha256::new(),
            size: 0,
        }
    }

    fn finish(self) -> (u64, String) {
        (self.size, format!("{:x}", self.hasher.finalize()))
    }
}

impl<R: Read> Read for HashingReader<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let read = self.input.read(bytes)?;
        self.hasher.update(&bytes[..read]);
        self.size = self
            .size
            .checked_add(read as u64)
            .ok_or_else(|| io::Error::other("archive size overflow"))?;
        Ok(read)
    }
}

#[cfg(test)]
mod review_tests {
    use std::{fs, path::PathBuf};

    use super::*;

    type ArchiveHook = Box<dyn FnMut(&Path) -> Result<(), BundleError>>;
    type FilesystemHook = Box<dyn FnMut(&Path, &str) -> Result<(), BundleError>>;

    #[derive(Default)]
    struct TestHooks {
        after_hash: Option<ArchiveHook>,
        after_staging: Option<FilesystemHook>,
        before_publish: Option<FilesystemHook>,
    }

    impl ValidationHooks for TestHooks {
        fn after_archive_hash(&mut self, archive: &Path) -> Result<(), BundleError> {
            match &mut self.after_hash {
                Some(hook) => hook(archive),
                None => Ok(()),
            }
        }

        fn after_staging_open(&mut self, parent: &Path, staging: &str) -> Result<(), BundleError> {
            match &mut self.after_staging {
                Some(hook) => hook(parent, staging),
                None => Ok(()),
            }
        }

        fn before_publish(&mut self, parent: &Path, staging: &str) -> Result<(), BundleError> {
            match &mut self.before_publish {
                Some(hook) => hook(parent, staging),
                None => Ok(()),
            }
        }
    }

    fn append_octal(field: &mut [u8], value: u64) {
        let width = field.len();
        let rendered = format!("{:0width$o}\0", value, width = width - 1);
        field.copy_from_slice(rendered.as_bytes());
    }

    fn archive_with_payload(payload: &[u8]) -> Vec<u8> {
        let manifest = serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "platform": BUNDLE_PLATFORM,
            "files": [{
                "path": "payload",
                "kind": "file",
                "size": payload.len(),
                "sha256": format!("{:x}", Sha256::digest(payload))
            }]
        }))
        .unwrap();
        let mut tar = Vec::new();
        append_entry(&mut tar, MANIFEST_PATH, &manifest);
        append_entry(&mut tar, "payload", payload);
        tar.extend_from_slice(&[0_u8; 1024]);
        zstd::stream::encode_all(tar.as_slice(), 1).unwrap()
    }

    fn append_entry(tar: &mut Vec<u8>, path: &str, body: &[u8]) {
        let mut header = [0_u8; 512];
        header[..path.len()].copy_from_slice(path.as_bytes());
        append_octal(&mut header[100..108], 0o644);
        append_octal(&mut header[108..116], 0);
        append_octal(&mut header[116..124], 0);
        append_octal(&mut header[124..136], body.len() as u64);
        append_octal(&mut header[136..148], 0);
        header[148..156].fill(b' ');
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let checksum: u64 = header.iter().map(|byte| u64::from(*byte)).sum();
        header[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
        tar.extend_from_slice(&header);
        tar.extend_from_slice(body);
        tar.resize(tar.len().div_ceil(512) * 512, 0);
    }

    fn lock(bytes: &[u8]) -> BundleLock {
        BundleLock {
            url: "https://example.invalid/bundle.tar.zst".to_owned(),
            sha256: format!("{:x}", Sha256::digest(bytes)),
            size: bytes.len() as u64,
            media_type: BUNDLE_MEDIA_TYPE.to_owned(),
            platform: BUNDLE_PLATFORM.to_owned(),
        }
    }

    #[test]
    fn archive_path_replacement_after_hash_cannot_change_extracted_bytes() {
        let original = archive_with_payload(b"original");
        let replacement = archive_with_payload(b"replacement");
        let temp = tempfile::tempdir().unwrap();
        let archive_path = temp.path().join("bundle.tar.zst");
        fs::write(&archive_path, &original).unwrap();
        let displaced = temp.path().join("verified-open-handle.tar.zst");
        let replacement_for_hook = replacement.clone();
        let displaced_for_hook = displaced.clone();
        let mut hooks = TestHooks {
            after_hash: Some(Box::new(move |path| {
                fs::rename(path, &displaced_for_hook).map_err(io_error)?;
                fs::write(path, &replacement_for_hook).map_err(io_error)
            })),
            ..TestHooks::default()
        };
        let destination = temp.path().join("output");
        validate_bundle_inner(&lock(&original), &archive_path, &destination, &mut hooks).unwrap();
        assert_eq!(fs::read(destination.join("payload")).unwrap(), b"original");
        assert_eq!(fs::read(&archive_path).unwrap(), replacement);
    }

    #[cfg(unix)]
    #[test]
    fn staging_path_replacement_does_not_redirect_extraction() {
        let archive = archive_with_payload(b"confined");
        let temp = tempfile::tempdir().unwrap();
        let archive_path = temp.path().join("bundle.tar.zst");
        fs::write(&archive_path, &archive).unwrap();
        let parent = temp.path().join("parent");
        let outside = temp.path().join("outside");
        fs::create_dir(&parent).unwrap();
        fs::create_dir(&outside).unwrap();
        let outside_for_hook = outside.clone();
        let mut hooks = TestHooks {
            after_staging: Some(Box::new(move |parent, staging| {
                let hidden = parent.join("displaced-staging");
                fs::rename(parent.join(staging), hidden).map_err(io_error)?;
                std::os::unix::fs::symlink(&outside_for_hook, parent.join(staging))
                    .map_err(io_error)
            })),
            ..TestHooks::default()
        };
        let result = validate_bundle_inner(
            &lock(&archive),
            &archive_path,
            &parent.join("output"),
            &mut hooks,
        );
        assert_eq!(result.unwrap_err(), BundleError::StagingChanged);
        assert!(fs::read_dir(&outside).unwrap().next().is_none());
        assert_eq!(
            fs::read(parent.join("displaced-staging/payload")).unwrap(),
            b"confined"
        );
    }

    #[cfg(unix)]
    #[test]
    fn destination_parent_replacement_does_not_redirect_extraction_or_publication() {
        let archive = archive_with_payload(b"anchored");
        let temp = tempfile::tempdir().unwrap();
        let archive_path = temp.path().join("bundle.tar.zst");
        fs::write(&archive_path, &archive).unwrap();
        let parent = temp.path().join("parent");
        let moved_parent = temp.path().join("moved-parent");
        let outside = temp.path().join("outside");
        fs::create_dir(&parent).unwrap();
        fs::create_dir(&outside).unwrap();
        let moved_for_hook: PathBuf = moved_parent.clone();
        let outside_for_hook = outside.clone();
        let mut hooks = TestHooks {
            before_publish: Some(Box::new(move |parent, _staging| {
                fs::rename(parent, &moved_for_hook).map_err(io_error)?;
                std::os::unix::fs::symlink(&outside_for_hook, parent).map_err(io_error)
            })),
            ..TestHooks::default()
        };
        let result = validate_bundle_inner(
            &lock(&archive),
            &archive_path,
            &parent.join("output"),
            &mut hooks,
        );
        assert_eq!(result.unwrap_err(), BundleError::ParentChanged);
        assert!(fs::read_dir(&outside).unwrap().next().is_none());
        assert!(!moved_parent.join("output").exists());
    }
}
