use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom},
    path::{Component, Path},
};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tempfile::Builder;
use thiserror::Error;

pub const BUNDLE_MEDIA_TYPE: &str = "application/vnd.gascan.workspace-bundle.v1+tar.zstd";
pub const BUNDLE_PLATFORM: &str = "linux/arm64";
const MANIFEST_PATH: &str = "bundle-manifest.json";
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 1_000_000;
const MAX_EXPANDED_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_OVERHEAD: u64 = (MAX_ENTRIES as u64 + 2) * 512;

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

impl PublishedBundleLocks {
    pub fn from_toml(contents: &str) -> Result<Self, BundleError> {
        let document: LockDocument =
            toml::from_str(contents).map_err(|error| BundleError::LockFormat(error.to_string()))?;
        let parsed = document.workspace_bundles;
        if parsed.media_type.as_deref() != Some(BUNDLE_MEDIA_TYPE)
            || parsed.platform.as_deref() != Some(BUNDLE_PLATFORM)
            || !matches!(parsed.publication.as_deref(), Some("pending" | "published"))
        {
            return Err(BundleError::InvalidLockRecord("workspace_bundles"));
        }
        let ubuntu_packages = parsed
            .ubuntu_packages
            .ok_or(BundleError::MissingLockRecord("ubuntu_packages"))?;
        let mise_runtimes = parsed
            .mise_runtimes
            .ok_or(BundleError::MissingLockRecord("mise_runtimes"))?;
        let gascamp_source_vendor = parsed
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
        Ok(Self {
            ubuntu_packages,
            mise_runtimes,
            gascamp_source_vendor,
        })
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

#[derive(Debug, Eq, PartialEq, Error)]
pub enum BundleError {
    #[error("bundle lock is missing required record {0}")]
    MissingLockRecord(&'static str),
    #[error("bundle lock record {0} is invalid")]
    InvalidLockRecord(&'static str),
    #[error("unknown bundle lock record {0}")]
    UnknownLockRecord(String),
    #[error("could not parse bundle lock: {0}")]
    LockFormat(String),
    #[error("bundle archive byte size does not match lock")]
    ArchiveSizeMismatch,
    #[error("bundle archive SHA-256 does not match lock")]
    ArchiveHashMismatch,
    #[error("bundle archive is invalid: {0}")]
    Archive(String),
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

pub fn validate_bundle(
    lock: &BundleLock,
    archive: &Path,
    destination: &Path,
) -> Result<BundleEvidence, BundleError> {
    if !lock.is_valid() {
        return Err(BundleError::InvalidLockRecord("bundle"));
    }
    if destination.exists() {
        return Err(BundleError::DestinationExists);
    }
    let (archive_size, archive_sha256) = hash_archive(archive)?;
    if archive_size != lock.size {
        return Err(BundleError::ArchiveSizeMismatch);
    }
    if archive_sha256 != lock.sha256 {
        return Err(BundleError::ArchiveHashMismatch);
    }
    let parent = destination
        .parent()
        .ok_or_else(|| BundleError::Io("bundle destination has no parent directory".to_owned()))?;
    fs::create_dir_all(parent).map_err(io_error)?;
    let temporary = Builder::new()
        .prefix(".workspace-bundle-")
        .tempdir_in(parent)
        .map_err(io_error)?;
    let (entries, regular_files) = extract_archive(archive, temporary.path())?;
    let temporary_path = temporary.keep();
    if let Err(error) = fs::rename(&temporary_path, destination) {
        let _ignored = fs::remove_dir_all(&temporary_path);
        return Err(io_error(error));
    }
    Ok(BundleEvidence {
        archive_sha256,
        archive_size,
        entries,
        regular_files,
    })
}

fn hash_archive(path: &Path) -> Result<(u64, String), BundleError> {
    let mut file = File::open(path).map_err(io_error)?;
    let mut hasher = Sha256::new();
    let size = io::copy(&mut file, &mut hasher).map_err(io_error)?;
    Ok((size, format!("{:x}", hasher.finalize())))
}

fn extract_archive(path: &Path, root: &Path) -> Result<(usize, usize), BundleError> {
    let file = File::open(path).map_err(io_error)?;
    let mut decoder = zstd::stream::read::Decoder::new(file)
        .map_err(|error| BundleError::Archive(error.to_string()))?;
    let mut expanded = tempfile::tempfile().map_err(io_error)?;
    let expanded_limit = MAX_EXPANDED_BYTES + MAX_MANIFEST_BYTES + MAX_ARCHIVE_OVERHEAD;
    let copied = io::copy(
        &mut decoder.by_ref().take(expanded_limit + 1),
        &mut expanded,
    )
    .map_err(|error| BundleError::Archive(error.to_string()))?;
    if copied > expanded_limit {
        return Err(BundleError::Archive(
            "expanded archive size limit exceeded".to_owned(),
        ));
    }
    if copied < 1024 || copied % 512 != 0 {
        return Err(BundleError::Archive("truncated tar archive".to_owned()));
    }
    expanded.seek(SeekFrom::End(-1024)).map_err(io_error)?;
    let mut terminator = [1_u8; 1024];
    expanded.read_exact(&mut terminator).map_err(io_error)?;
    if terminator.iter().any(|byte| *byte != 0) {
        return Err(BundleError::Archive("truncated tar archive".to_owned()));
    }
    expanded.seek(SeekFrom::Start(0)).map_err(io_error)?;
    let mut archive = tar::Archive::new(expanded);
    let mut entries = archive
        .entries()
        .map_err(|error| BundleError::Archive(error.to_string()))?;
    let Some(first) = entries.next() else {
        return Err(BundleError::ManifestMustBeFirst);
    };
    let mut first = first.map_err(|error| BundleError::Archive(error.to_string()))?;
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
        .map_err(|error| BundleError::Archive(error.to_string()))?;
    let manifest: BundleManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| BundleError::Manifest(error.to_string()))?;
    validate_manifest(&manifest)?;

    let mut seen = BTreeSet::new();
    let mut regular_files = 0;
    for entry in entries {
        let mut entry = entry.map_err(|error| BundleError::Archive(error.to_string()))?;
        let entry_path = entry_path(&entry)?;
        if !seen.insert(entry_path.clone()) {
            return Err(BundleError::DuplicateArchiveEntry(entry_path));
        }
        if seen.len() > MAX_ENTRIES {
            return Err(BundleError::Archive(
                "archive entry limit exceeded".to_owned(),
            ));
        }
        let Some(expected) = manifest.files.iter().find(|item| item.path() == entry_path) else {
            return Err(BundleError::UnexpectedArchiveEntry(entry_path));
        };
        extract_entry(&mut entry, expected, root, &mut regular_files)?;
    }
    for expected in &manifest.files {
        if !seen.contains(expected.path()) {
            return Err(BundleError::MissingArchiveEntry(expected.path().to_owned()));
        }
    }
    Ok((seen.len(), regular_files))
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
    for entry in &manifest.files {
        let path = entry.path();
        validate_relative_path(path)?;
        if previous.is_some_and(|previous_path| previous_path >= path) {
            return Err(BundleError::NonCanonicalManifest);
        }
        previous = Some(path);
        match entry {
            ManifestEntry::File { size, sha256, .. } => {
                if !is_lower_sha256(sha256) {
                    return Err(BundleError::Manifest(format!(
                        "invalid SHA-256 for {}",
                        path
                    )));
                }
                expanded = expanded
                    .checked_add(*size)
                    .ok_or_else(|| BundleError::Manifest("expanded size overflow".to_owned()))?;
            }
            ManifestEntry::Directory { .. } => {}
            ManifestEntry::Symlink { target, .. } => validate_link(path, target)?,
        }
    }
    if expanded > MAX_EXPANDED_BYTES {
        return Err(BundleError::Manifest(
            "expanded file size limit exceeded".to_owned(),
        ));
    }
    Ok(())
}

fn extract_entry<R: Read>(
    entry: &mut tar::Entry<'_, R>,
    expected: &ManifestEntry,
    root: &Path,
    regular_files: &mut usize,
) -> Result<(), BundleError> {
    let path = expected.path();
    let output = root.join(path);
    match expected {
        ManifestEntry::File { size, sha256, .. } => {
            if !entry.header().entry_type().is_file() {
                return unsupported_or_mismatch(entry, path);
            }
            if entry.size() != *size {
                return Err(BundleError::FileSizeMismatch(path.to_owned()));
            }
            create_parent(&output)?;
            let mut file = File::create(&output).map_err(io_error)?;
            let mut hasher = Sha256::new();
            let copied = io::copy(
                &mut entry.take(*size + 1),
                &mut HashingWriter::new(&mut file, &mut hasher),
            )
            .map_err(|error| BundleError::Archive(error.to_string()))?;
            if copied != *size {
                return Err(BundleError::FileSizeMismatch(path.to_owned()));
            }
            if format!("{:x}", hasher.finalize()) != *sha256 {
                return Err(BundleError::FileHashMismatch(path.to_owned()));
            }
            *regular_files += 1;
        }
        ManifestEntry::Directory { .. } => {
            if !entry.header().entry_type().is_dir() {
                return unsupported_or_mismatch(entry, path);
            }
            fs::create_dir_all(&output).map_err(io_error)?;
        }
        ManifestEntry::Symlink { target, .. } => {
            if !entry.header().entry_type().is_symlink() {
                return unsupported_or_mismatch(entry, path);
            }
            let actual = entry
                .link_name()
                .map_err(|error| BundleError::Archive(error.to_string()))?
                .ok_or_else(|| BundleError::UnsafeLink(path.to_owned()))?;
            if actual.to_str() != Some(target) {
                return Err(BundleError::UnsafeLink(path.to_owned()));
            }
            validate_link(path, target)?;
            create_parent(&output)?;
            std::os::unix::fs::symlink(target, &output).map_err(io_error)?;
        }
    }
    Ok(())
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
    let path = entry
        .path()
        .map_err(|error| BundleError::Archive(error.to_string()))?;
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

fn create_parent(path: &Path) -> Result<(), BundleError> {
    let parent = path
        .parent()
        .ok_or_else(|| BundleError::Io("archive entry has no parent".to_owned()))?;
    fs::create_dir_all(parent).map_err(io_error)
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn io_error(error: io::Error) -> BundleError {
    BundleError::Io(error.to_string())
}

struct HashingWriter<'a, W> {
    output: &'a mut W,
    hasher: &'a mut Sha256,
}

impl<'a, W> HashingWriter<'a, W> {
    fn new(output: &'a mut W, hasher: &'a mut Sha256) -> Self {
        Self { output, hasher }
    }
}

impl<W: io::Write> io::Write for HashingWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.output.write(bytes)?;
        self.hasher.update(&bytes[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}
