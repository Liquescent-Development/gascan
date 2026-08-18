//! The engine's boot artifacts: fetched once, verified by digest, unpacked.
//!
//! The Arca engine boots each sandbox as a real Linux VM, so it needs a kernel
//! and `vminit` -- the guest-side init packaged as an OCI image layout. Neither
//! ships in the `.pkg`: together they are about 83MB compressed, carried by
//! every user including those who never select this backend, and `vmlinux` is a
//! Linux kernel whose redistribution carries a source obligation. So they are
//! fetched from the signed Arca release the pin already names.
//!
//! **This never runs inside another operation.** A ~83MB download must not
//! surprise someone who typed `gascan up`. `gascan doctor` reports the artifacts
//! as missing and names this command as the remedy, which is the same pattern
//! the product already uses for a prerequisite the installer cannot satisfy.
//!
//! **Every artifact is verified twice, and the two digests answer different
//! questions.** The asset's own sha256 proves the download arrived intact and
//! entire. The digest of what is INSIDE it -- the uncompressed kernel, the OCI
//! manifest -- proves the content is the intended one however it was packaged,
//! and it is the one that survives repackaging, because `tar czf` is not
//! reproducible. A check with only the first breaks the moment anyone
//! repackages; a check with only the second cannot detect a truncated download.

use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest as _, Sha256};

/// The pin, compiled in rather than read from disk at run time.
///
/// An installed `gascan` has no source tree beside it, so there is no
/// `engine/arca-pin.json` to read. Compiling it in also matches the trust
/// model exactly: a signed Gas Can release commit fixes the pin, and the binary
/// built from that commit carries the digests that commit chose. A binary and a
/// pin file that could drift apart at run time would be two things to keep in
/// step for no gain.
const PINNED: &str = include_str!("../../../engine/arca-pin.json");

/// The mode every directory this module owns carries.
///
/// `0o700` and not the umask's answer, and it is the SAME value `gascand`'s
/// `controller_state::DIRECTORY_MODE` requires and `packaging/macos/uninstall.sh`
/// refuses to delete without. Those two are the consumers; this is the producer,
/// and it was the only one of the three that did not name a mode.
const PRIVATE_MODE: u32 = 0o700;

/// What the pin says an artifact must be.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct Artifact {
    pub asset: String,
    pub url: String,
    pub bytes: u64,
    pub sha256: String,
    pub content: Content,
}

/// The identity that survives repackaging, and how to recover it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct Content {
    pub kind: ContentKind,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ContentKind {
    /// Decompress the asset; the single member is the content.
    GzipMember,
    /// Unpack the asset and read the OCI image index; the one manifest
    /// descriptor it names carries the digest and size.
    OciManifest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct Artifacts {
    pub kernel: Artifact,
    pub vminit: Artifact,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct Pin {
    pub schema: u32,
    pub tag: String,
    pub revision: String,
    pub artifacts: Artifacts,
}

impl Pin {
    /// The pin this binary was built from.
    ///
    /// Parsed rather than returned as text so that a malformed pin is a build
    /// this crate's tests fail on rather than a run-time surprise in the one
    /// command a user reaches for when nothing works.
    pub fn compiled_in() -> Result<Self, ArtifactError> {
        serde_json::from_str(PINNED).map_err(|error| ArtifactError::MalformedPin {
            detail: error.to_string(),
        })
    }
}

#[derive(Debug)]
pub enum ArtifactError {
    MalformedPin {
        detail: String,
    },
    /// The download did not complete. Nothing usable is left behind.
    Download {
        asset: String,
        detail: String,
    },
    /// What arrived is not what the pin describes.
    ///
    /// Carries both values, because "the digest was wrong" without them leaves
    /// a user unable to tell a corrupted download from a moved pin.
    Digest {
        what: String,
        expected: String,
        observed: String,
    },
    /// The asset is the right bytes but could not be unpacked.
    Unpack {
        asset: String,
        detail: String,
    },
    Io(io::Error),
}

impl std::fmt::Display for ArtifactError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MalformedPin { detail } => {
                write!(
                    formatter,
                    "the compiled-in engine pin is malformed: {detail}"
                )
            }
            Self::Download { asset, detail } => write!(
                formatter,
                "could not download {asset}: {detail}; nothing was left in place, \
                 re-run `gascan engine fetch` once the network is available"
            ),
            Self::Digest {
                what,
                expected,
                observed,
            } => write!(
                formatter,
                "{what} does not match the pin: expected {expected}, observed {observed}; \
                 the downloaded file was deleted"
            ),
            Self::Unpack { asset, detail } => {
                write!(formatter, "could not unpack {asset}: {detail}")
            }
            Self::Io(error) => write!(formatter, "engine artifact I/O error: {error}"),
        }
    }
}

impl std::error::Error for ArtifactError {}

impl From<io::Error> for ArtifactError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Where the artifacts live.
///
/// `~/Library/Application Support/dev.gascan/engine/`: per-user, durable, Gas
/// Can-owned, beside the existing `dev.gascan/controller/`.
///
/// **Explicitly not `~/.arca`.** That is the engine's own state root, and
/// milestone 2's thesis -- that the engine writes only inside it -- cost two
/// defects to establish. Gas Can writing into it would be the same boundary
/// violation from the other side.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactPaths {
    root: PathBuf,
    /// The directories this instance may create or tighten, outermost first.
    ///
    /// **Enumerated and never derived by walking up from `root`.** A walk has
    /// to know where to stop, and the only honest stopping rule is the one the
    /// constructor already knows: `for_user` owns `dev.gascan` and its `engine`
    /// child, and `under` owns exactly the directory it was handed. A first
    /// version of this derived the chain by climbing until it saw a component
    /// named `dev.gascan`, which for any other root climbed to `/` and set
    /// about chmod'ing the filesystem.
    owned: Vec<PathBuf>,
}

impl ArtifactPaths {
    /// One directory, owned by the caller's choosing. Nothing above it is this
    /// instance's to create or tighten.
    #[must_use]
    pub fn under(root: PathBuf) -> Self {
        Self {
            owned: vec![root.clone()],
            root,
        }
    }

    /// The production layout: `<support>/dev.gascan/engine`, owning both.
    ///
    /// `dev.gascan` is owned because on a machine where `gascan engine fetch`
    /// runs before the first daemon start -- the documented order, since the
    /// fetch is the doctor's remedy for a daemon that cannot start -- this is
    /// what creates it, and `gascand` will refuse to start unless it is `0700`.
    /// Above it, `Library` and `Application Support` are macOS's own.
    #[must_use]
    pub fn under_application_support(support: &Path) -> Self {
        let container = support.join("dev.gascan");
        let root = container.join("engine");
        Self {
            owned: vec![container, root.clone()],
            root,
        }
    }

    pub fn for_user() -> Result<Self, ArtifactError> {
        let home = crate::account::effective_account_home()
            .map_err(|error| ArtifactError::Io(io::Error::other(error.to_string())))?;
        Ok(Self::under_application_support(
            &home.join("Library").join("Application Support"),
        ))
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Creates the artifact root, and every parent this crate owns, at `0700`.
    ///
    /// **`create_dir_all` alone was a defect, and it reached beyond this file.**
    /// It applies the process umask, which is `022` on a default macOS host, so
    /// the directories arrived `0755`. MEASURED on the machine this was written
    /// on, after `gascan engine fetch` had run: `dev.gascan` and
    /// `dev.gascan/controller` were `700` and `dev.gascan/engine` was `755`.
    ///
    /// Two consumers require exactly `0700` and neither repairs:
    ///
    /// - `gascand`'s `ensure_private_child_directory` `fchmod`s only a
    ///   directory it CREATED, then validates the mode. So on a machine where
    ///   `gascan engine fetch` ran before the first daemon start -- which is the
    ///   documented order, since the fetch is the doctor's remedy for a daemon
    ///   that cannot start -- `create_dir_all` makes `dev.gascan` itself `0755`
    ///   and the daemon then refuses to start on EITHER backend, with a message
    ///   that says "ownership, type, or mode is unsafe" and does not isolate
    ///   which.
    /// - `packaging/macos/uninstall.sh`'s private-child check requires `0700`
    ///   and refuses otherwise with 65, after the controller and runtime data
    ///   have already been removed.
    ///
    /// **It repairs as well as creates**, because every host that has already
    /// run the fetch is in the broken state and nothing else will fix it. Only
    /// a directory this account owns is tightened; anything else is an error
    /// rather than a chmod of a path that is not ours. What it may touch at all
    /// is the enumerated `owned` list and nothing above it.
    pub fn prepare_private_root(&self) -> Result<(), ArtifactError> {
        for path in &self.owned {
            Self::create_or_tighten(path)?;
        }
        Ok(())
    }

    /// One directory at `0700`, created with that mode or tightened to it.
    ///
    /// `DirBuilder::mode` and not `create_dir_all` followed by
    /// `set_permissions`: the latter leaves a window in which the directory
    /// exists group- and world-readable, and this is the directory a kernel
    /// image is about to be written into.
    fn create_or_tighten(path: &Path) -> Result<(), ArtifactError> {
        use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};

        match std::fs::DirBuilder::new().mode(PRIVATE_MODE).create(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(ArtifactError::Io(error)),
        }

        // `symlink_metadata` and not `metadata`: a symlink planted here would
        // otherwise be followed, and the mode reported would be the target's.
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.is_dir() {
            return Err(ArtifactError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a directory", path.display()),
            )));
        }
        if metadata.uid() != rustix::process::getuid().as_raw() {
            return Err(ArtifactError::Io(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "{} is owned by uid {} and not by this account",
                    path.display(),
                    metadata.uid()
                ),
            )));
        }
        if metadata.permissions().mode() & 0o7777 != PRIVATE_MODE {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(PRIVATE_MODE))?;
        }
        Ok(())
    }

    /// The uncompressed kernel, which is what `GASCAN_ARCA_KERNEL_PATH` names.
    #[must_use]
    pub fn kernel(&self) -> PathBuf {
        self.root.join("vmlinux")
    }

    /// The unpacked OCI layout, which is what `GASCAN_ARCA_VMINIT_LAYOUT` names.
    #[must_use]
    pub fn vminit(&self) -> PathBuf {
        self.root.join("vminit")
    }
}

/// Running the system tools this command needs.
///
/// Behind a trait because the four failure modes -- no network, a wrong digest,
/// a moved pin, a truncated file -- are all about what arrives on disk, and a
/// test that had to reach the network to produce them would be testing GitHub.
///
/// The tools are the system's own. This is how the product already reaches
/// outside itself: `gascan-apple` drives Apple's `container` CLI through the
/// same shape, and `scripts/build-arca-engine.sh` drives git, jq, swift and
/// codesign. It also keeps a TLS stack, a gzip decoder and a tar reader out of
/// a dependency list that is a reviewed release input.
pub trait ToolRunner: Send + Sync {
    /// Fetch `url` to `destination`, replacing whatever is there.
    fn download(&self, url: &str, destination: &Path) -> Result<(), String>;
    /// Decompress a single gzip member to `destination`.
    fn gunzip(&self, archive: &Path, destination: &Path) -> Result<(), String>;
    /// Extract a gzipped tar into `directory`.
    fn untar(&self, archive: &Path, directory: &Path) -> Result<(), String>;
}

/// The system tools.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemTools;

impl SystemTools {
    fn run(command: &mut std::process::Command, what: &str) -> Result<(), String> {
        let output = command
            .output()
            .map_err(|error| format!("{what} could not be run: {error}"))?;
        if output.status.success() {
            return Ok(());
        }
        // stderr and not just the status: curl's exit code alone does not say
        // whether the host was unreachable, the TLS handshake failed or the
        // asset is gone, and those send a reader to three different places.
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(format!(
            "{what} failed ({}){}",
            output.status,
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        ))
    }
}

impl ToolRunner for SystemTools {
    fn download(&self, url: &str, destination: &Path) -> Result<(), String> {
        // --fail so an HTTP error page is an error rather than a file that
        // then fails its digest check with a confusing message. --location
        // because a release asset redirects to object storage. --silent with
        // --show-error so the only output is a diagnostic when there is one.
        Self::run(
            std::process::Command::new("/usr/bin/curl")
                .arg("--fail")
                .arg("--location")
                .arg("--silent")
                .arg("--show-error")
                .arg("--output")
                .arg(destination)
                .arg(url),
            "curl",
        )
    }

    fn gunzip(&self, archive: &Path, destination: &Path) -> Result<(), String> {
        let output = std::fs::File::create(destination)
            .map_err(|error| format!("could not create {}: {error}", destination.display()))?;
        Self::run(
            std::process::Command::new("/usr/bin/gunzip")
                .arg("--stdout")
                .arg(archive)
                .stdout(output),
            "gunzip",
        )
    }

    fn untar(&self, archive: &Path, directory: &Path) -> Result<(), String> {
        Self::run(
            std::process::Command::new("/usr/bin/tar")
                .arg("-xzf")
                .arg(archive)
                .arg("-C")
                .arg(directory),
            "tar",
        )
    }
}

/// The sha256 of a file, and its length, in one pass.
fn measure(path: &Path) -> io::Result<(u64, String)> {
    use std::io::Read as _;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 16];
    let mut length = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        length += read as u64;
        hasher.update(&buffer[..read]);
    }
    Ok((length, hex(&hasher.finalize())))
}

fn hex(bytes: &[u8]) -> String {
    crate::hex::lower(bytes)
}

/// Both checks, in the order that gives the clearer message first.
///
/// Length before digest deliberately: a truncated or empty download is the
/// common failure, and "expected 9092349 bytes, observed 0" tells a reader what
/// happened, where two different hex strings do not.
fn require_file(path: &Path, what: &str, bytes: u64, sha256: &str) -> Result<(), ArtifactError> {
    let (observed_bytes, observed_sha) = measure(path)?;
    if observed_bytes != bytes {
        return Err(ArtifactError::Digest {
            what: format!("{what} length"),
            expected: format!("{bytes} bytes"),
            observed: format!("{observed_bytes} bytes"),
        });
    }
    if observed_sha != sha256 {
        return Err(ArtifactError::Digest {
            what: format!("{what} sha256"),
            expected: sha256.to_owned(),
            observed: observed_sha,
        });
    }
    Ok(())
}

/// Verifies the OCI layout that was unpacked, by its manifest.
///
/// Reads `index.json`, requires exactly one manifest descriptor, and checks
/// both that the descriptor matches the pin AND that the blob it names hashes
/// to its own digest. The second check is not redundant: a layout whose
/// `index.json` was edited to name the expected digest would pass the first and
/// fail this one, and `index.json` is the one file in the layout that is not
/// content-addressed.
fn require_oci_manifest(layout: &Path, bytes: u64, sha256: &str) -> Result<(), ArtifactError> {
    let index_path = layout.join("index.json");
    let index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&index_path)?).map_err(|error| {
            ArtifactError::Unpack {
                asset: index_path.display().to_string(),
                detail: error.to_string(),
            }
        })?;
    let manifests = index
        .get("manifests")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| ArtifactError::Unpack {
            asset: index_path.display().to_string(),
            detail: "the image index names no manifests".to_owned(),
        })?;
    let [manifest] = manifests.as_slice() else {
        return Err(ArtifactError::Unpack {
            asset: index_path.display().to_string(),
            detail: format!(
                "the image index names {} manifests; the pin describes one",
                manifests.len()
            ),
        });
    };
    let digest = manifest
        .get("digest")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let expected_digest = format!("sha256:{sha256}");
    if digest != expected_digest {
        return Err(ArtifactError::Digest {
            what: "vminit OCI manifest digest".to_owned(),
            expected: expected_digest,
            observed: digest.to_owned(),
        });
    }
    let size = manifest.get("size").and_then(serde_json::Value::as_u64);
    if size != Some(bytes) {
        return Err(ArtifactError::Digest {
            what: "vminit OCI manifest size".to_owned(),
            expected: format!("{bytes} bytes"),
            observed: size.map_or_else(|| "absent".to_owned(), |size| format!("{size} bytes")),
        });
    }
    // The blob's file name IS its digest, so this asserts the layout is
    // internally consistent rather than merely self-describing.
    require_file(
        &layout.join("blobs").join("sha256").join(sha256),
        "vminit OCI manifest blob",
        bytes,
        sha256,
    )
}

/// What a completed fetch produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fetched {
    pub kernel: PathBuf,
    pub vminit: PathBuf,
}

/// Downloads, verifies and installs both artifacts.
///
/// **Staged, then promoted.** Everything lands under a sibling staging
/// directory and is verified there; only then does it replace the live paths.
/// This is the shape Landing 1's Critical taught: a formatter pointed at the
/// final path left a valid-looking, empty artifact in place when it refused, and
/// every later run reused it. An artifact directory is the same trap -- a
/// half-written kernel that a later run treats as present is a container booting
/// on bytes nobody verified.
///
/// The staging directory is removed on every exit path, so a failure leaves
/// nothing behind for a later run to mistake for a fetch.
pub fn fetch<R: ToolRunner>(
    paths: &ArtifactPaths,
    pin: &Pin,
    runner: &R,
) -> Result<Fetched, ArtifactError> {
    paths.prepare_private_root()?;
    let staging = paths.root().join(".staging");
    // A leftover staging directory is from a run that was killed rather than
    // one that failed, since every failure below removes it. It is discarded
    // rather than resumed: resuming would mean trusting bytes whose provenance
    // this run cannot establish.
    let _ = std::fs::remove_dir_all(&staging);
    // The staging directory holds unverified bytes and is private for the same
    // reason its parent is.
    {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new()
            .mode(PRIVATE_MODE)
            .create(&staging)?;
    }

    let result = fetch_into(&staging, paths, pin, runner);
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn fetch_into<R: ToolRunner>(
    staging: &Path,
    paths: &ArtifactPaths,
    pin: &Pin,
    runner: &R,
) -> Result<Fetched, ArtifactError> {
    // The kernel: download, check the asset, decompress, check what came out.
    let kernel_asset = staging.join(&pin.artifacts.kernel.asset);
    runner
        .download(&pin.artifacts.kernel.url, &kernel_asset)
        .map_err(|detail| ArtifactError::Download {
            asset: pin.artifacts.kernel.asset.clone(),
            detail,
        })?;
    require_file(
        &kernel_asset,
        &pin.artifacts.kernel.asset,
        pin.artifacts.kernel.bytes,
        &pin.artifacts.kernel.sha256,
    )?;
    let staged_kernel = staging.join("vmlinux");
    runner
        .gunzip(&kernel_asset, &staged_kernel)
        .map_err(|detail| ArtifactError::Unpack {
            asset: pin.artifacts.kernel.asset.clone(),
            detail,
        })?;
    require_file(
        &staged_kernel,
        "the uncompressed kernel",
        pin.artifacts.kernel.content.bytes,
        &pin.artifacts.kernel.content.sha256,
    )?;

    // vminit: download, check the asset, unpack, check the manifest inside.
    let vminit_asset = staging.join(&pin.artifacts.vminit.asset);
    runner
        .download(&pin.artifacts.vminit.url, &vminit_asset)
        .map_err(|detail| ArtifactError::Download {
            asset: pin.artifacts.vminit.asset.clone(),
            detail,
        })?;
    require_file(
        &vminit_asset,
        &pin.artifacts.vminit.asset,
        pin.artifacts.vminit.bytes,
        &pin.artifacts.vminit.sha256,
    )?;
    let unpacked = staging.join("unpacked");
    std::fs::create_dir_all(&unpacked)?;
    runner
        .untar(&vminit_asset, &unpacked)
        .map_err(|detail| ArtifactError::Unpack {
            asset: pin.artifacts.vminit.asset.clone(),
            detail,
        })?;
    let staged_vminit = unpacked.join("vminit");
    require_oci_manifest(
        &staged_vminit,
        pin.artifacts.vminit.content.bytes,
        &pin.artifacts.vminit.content.sha256,
    )?;

    // Promotion, only now. `rename` is atomic within a filesystem and staging
    // is a sibling of the destination, so a reader either sees the previous
    // artifact or this one, never a partial.
    let kernel = paths.kernel();
    let vminit = paths.vminit();
    let _ = std::fs::remove_file(&kernel);
    let _ = std::fs::remove_dir_all(&vminit);
    std::fs::rename(&staged_kernel, &kernel)?;
    std::fs::rename(&staged_vminit, &vminit)?;
    Ok(Fetched { kernel, vminit })
}

/// Are the installed artifacts present and still what the pin describes?
///
/// This is what `gascan doctor` reports and it is deliberately the SAME
/// verification the fetch performs, not a cheaper stand-in. A doctor check that
/// only tested for the files' existence would report a moved pin, a truncated
/// file and a corrupted one all as healthy -- which is the whole class of
/// failure the digests exist to catch.
pub fn verify_installed(paths: &ArtifactPaths, pin: &Pin) -> Result<(), ArtifactError> {
    require_file(
        &paths.kernel(),
        "the installed kernel",
        pin.artifacts.kernel.content.bytes,
        &pin.artifacts.kernel.content.sha256,
    )?;
    require_oci_manifest(
        &paths.vminit(),
        pin.artifacts.vminit.content.bytes,
        &pin.artifacts.vminit.content.sha256,
    )
}
