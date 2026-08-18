//! The four failure modes of the artifact fetch, each with a test.
//!
//! All of them are about what ends up on disk, so the tool runner is a fake and
//! the fixtures are small real gzip and tar archives built by the system tools
//! the production runner uses. Nothing here reaches the network: a test that
//! did would be testing GitHub, and the interesting cases -- a truncated file,
//! a moved pin -- are exactly the ones a healthy network never produces.

use gascan_core::engine_artifacts::{
    ArtifactError, ArtifactPaths, Pin, ToolRunner, fetch, verify_installed,
};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Builds a miniature but STRUCTURALLY REAL pair of artifacts.
///
/// A real gzip member and a real OCI layout inside a real tarball, made by the
/// same `/usr/bin/gzip` and `/usr/bin/tar` the production runner drives. A
/// hand-written fake would let the unpack steps pass against bytes those tools
/// would have rejected, which is the half of this code a fake cannot exercise.
struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    kernel_gz: PathBuf,
    vminit_tgz: PathBuf,
    pin: Pin,
}

fn build_fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let source = root.join("source");
    std::fs::create_dir_all(&source)?;

    // The kernel: any bytes will do, but they must be real gzip on the wire.
    let kernel_bytes = b"a kernel stands in for 28MB of one".to_vec();
    std::fs::write(source.join("vmlinux"), &kernel_bytes)?;
    let kernel_gz = root.join("vmlinux-arm64.gz");
    let gz = std::process::Command::new("/usr/bin/gzip")
        .arg("--stdout")
        .arg(source.join("vmlinux"))
        .output()?;
    assert!(gz.status.success(), "the fixture kernel must gzip");
    std::fs::write(&kernel_gz, &gz.stdout)?;

    // vminit: a real OCI layout. `index.json` names one manifest whose digest
    // is the manifest blob's own sha256, which is what makes the layout
    // content-addressed and what the verification asserts.
    let layout = source.join("vminit");
    std::fs::create_dir_all(layout.join("blobs").join("sha256"))?;
    let manifest = br#"{"schemaVersion":2,"layers":[]}"#.to_vec();
    let manifest_digest = sha256_hex(&manifest);
    std::fs::write(
        layout.join("blobs").join("sha256").join(&manifest_digest),
        &manifest,
    )?;
    std::fs::write(
        layout.join("index.json"),
        format!(
            r#"{{"schemaVersion":2,"manifests":[{{"digest":"sha256:{manifest_digest}","size":{}}}]}}"#,
            manifest.len()
        ),
    )?;
    let vminit_tgz = root.join("vminit-oci-arm64.tar.gz");
    let tar = std::process::Command::new("/usr/bin/tar")
        .arg("-czf")
        .arg(&vminit_tgz)
        .arg("-C")
        .arg(&source)
        .arg("vminit")
        .output()?;
    assert!(tar.status.success(), "the fixture layout must tar");

    let kernel_gz_bytes = std::fs::read(&kernel_gz)?;
    let vminit_tgz_bytes = std::fs::read(&vminit_tgz)?;
    let pin: Pin = serde_json::from_str(&format!(
        r#"{{
          "schema": 2, "name": "arca",
          "url": "https://example.invalid/arca.git",
          "tag": "fixture-tag",
          "revision": "{rev}",
          "artifacts": {{
            "kernel": {{
              "asset": "vmlinux-arm64.gz",
              "url": "https://example.invalid/vmlinux-arm64.gz",
              "bytes": {kgz_len}, "sha256": "{kgz_sha}",
              "content": {{ "kind": "gzip-member", "bytes": {k_len}, "sha256": "{k_sha}" }}
            }},
            "vminit": {{
              "asset": "vminit-oci-arm64.tar.gz",
              "url": "https://example.invalid/vminit-oci-arm64.tar.gz",
              "bytes": {vgz_len}, "sha256": "{vgz_sha}",
              "content": {{ "kind": "oci-manifest", "bytes": {m_len}, "sha256": "{m_sha}" }}
            }}
          }}
        }}"#,
        rev = "0".repeat(40),
        kgz_len = kernel_gz_bytes.len(),
        kgz_sha = sha256_hex(&kernel_gz_bytes),
        k_len = kernel_bytes.len(),
        k_sha = sha256_hex(&kernel_bytes),
        vgz_len = vminit_tgz_bytes.len(),
        vgz_sha = sha256_hex(&vminit_tgz_bytes),
        m_len = manifest.len(),
        m_sha = manifest_digest,
    ))?;

    Ok(Fixture {
        _temp: temp,
        root,
        kernel_gz,
        vminit_tgz,
        pin,
    })
}

/// Serves the fixture files instead of reaching the network, and can be told to
/// fail, truncate, or serve the wrong bytes.
struct FakeTools {
    kernel_gz: PathBuf,
    vminit_tgz: PathBuf,
    fault: Fault,
    downloads: Mutex<Vec<String>>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Fault {
    None,
    NoNetwork,
    /// Serve the right length with different bytes: the digest is the only
    /// check that can catch it, which is what makes it worth its own case.
    CorruptKernel,
    /// Serve a prefix, which is what an interrupted download or a full disk
    /// leaves behind.
    TruncatedVminit,
}

impl FakeTools {
    fn new(fixture: &Fixture, fault: Fault) -> Self {
        Self {
            kernel_gz: fixture.kernel_gz.clone(),
            vminit_tgz: fixture.vminit_tgz.clone(),
            fault,
            downloads: Mutex::new(Vec::new()),
        }
    }
    fn downloads(&self) -> Vec<String> {
        self.downloads
            .lock()
            .map(|seen| seen.clone())
            .unwrap_or_default()
    }
}

impl ToolRunner for FakeTools {
    fn download(&self, url: &str, destination: &Path) -> Result<(), String> {
        if let Ok(mut seen) = self.downloads.lock() {
            seen.push(url.to_owned());
        }
        if self.fault == Fault::NoNetwork {
            return Err("Could not resolve host: example.invalid".to_owned());
        }
        let kernel = url.ends_with("vmlinux-arm64.gz");
        let source = if kernel {
            &self.kernel_gz
        } else {
            &self.vminit_tgz
        };
        let mut bytes = std::fs::read(source).map_err(|error| error.to_string())?;
        if kernel && self.fault == Fault::CorruptKernel {
            // Same length, different content.
            let last = bytes.len() - 1;
            bytes[last] ^= 0xff;
        }
        if !kernel && self.fault == Fault::TruncatedVminit {
            bytes.truncate(bytes.len() / 2);
        }
        std::fs::write(destination, bytes).map_err(|error| error.to_string())
    }

    fn gunzip(&self, archive: &Path, destination: &Path) -> Result<(), String> {
        gascan_core::engine_artifacts::SystemTools.gunzip(archive, destination)
    }

    fn untar(&self, archive: &Path, directory: &Path) -> Result<(), String> {
        gascan_core::engine_artifacts::SystemTools.untar(archive, directory)
    }
}

fn installed(root: &Path) -> ArtifactPaths {
    ArtifactPaths::under(root.join("installed"))
}

/// The healthy path, which every negative case below is measured against.
#[test]
fn a_verified_fetch_installs_both_artifacts_and_verifies_again_afterwards() -> TestResult {
    let fixture = build_fixture()?;
    let paths = installed(&fixture.root);
    let tools = FakeTools::new(&fixture, Fault::None);

    let fetched = fetch(&paths, &fixture.pin, &tools)?;

    assert!(fetched.kernel.is_file(), "the kernel must be installed");
    assert!(
        fetched.vminit.is_dir(),
        "the vminit layout must be installed"
    );
    // The same verification the doctor performs, so the fetch cannot succeed
    // into a state the doctor would then call broken.
    verify_installed(&paths, &fixture.pin)?;
    // Nothing but the two artifacts survives: no staging directory, no
    // downloaded archives left to fill the user's disk.
    let mut remaining: Vec<_> = std::fs::read_dir(paths.root())?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    remaining.sort();
    assert_eq!(remaining, vec!["vminit".to_owned(), "vmlinux".to_owned()]);
    Ok(())
}

/// **No network: a hard failure that names the command, leaving nothing.**
#[test]
fn a_failed_download_names_the_command_and_leaves_no_partial_artifact() -> TestResult {
    let fixture = build_fixture()?;
    let paths = installed(&fixture.root);
    let tools = FakeTools::new(&fixture, Fault::NoNetwork);

    let error = fetch(&paths, &fixture.pin, &tools).expect_err("no network");

    assert!(matches!(error, ArtifactError::Download { .. }), "{error}");
    assert!(
        error.to_string().contains("gascan engine fetch"),
        "the failure must name the command to re-run: {error}"
    );
    assert!(!paths.kernel().exists(), "no kernel may be left in place");
    assert!(!paths.vminit().exists(), "no vminit may be left in place");
    assert!(
        !paths.root().join(".staging").exists(),
        "a failed fetch must leave no staging directory behind"
    );
    Ok(())
}

/// **A wrong digest is refused, naming both values, and nothing is installed.**
///
/// The corruption keeps the length identical, so the length check cannot catch
/// it and the digest is the only thing standing between these bytes and a
/// booting kernel.
#[test]
fn a_digest_mismatch_names_both_values_and_installs_nothing() -> TestResult {
    let fixture = build_fixture()?;
    let paths = installed(&fixture.root);
    let tools = FakeTools::new(&fixture, Fault::CorruptKernel);

    let error = fetch(&paths, &fixture.pin, &tools).expect_err("the kernel is corrupt");

    let ArtifactError::Digest {
        expected, observed, ..
    } = &error
    else {
        return Err(format!("expected a digest refusal, got {error}").into());
    };
    assert_ne!(expected, observed, "both values must be reported");
    assert_eq!(
        expected.len(),
        64,
        "the expected value is a sha256: {expected}"
    );
    assert!(
        !paths.kernel().exists(),
        "a refused artifact is not installed"
    );
    assert!(
        !paths.root().join(".staging").exists(),
        "the downloaded file must be deleted"
    );
    Ok(())
}

/// **A truncated download is indistinguishable from corruption, and treated so.**
///
/// It is reported by LENGTH rather than by digest, which is deliberate: two hex
/// strings tell a reader nothing, while "expected N bytes, observed N/2" says
/// the download did not finish.
#[test]
fn a_partially_written_artifact_is_treated_as_corrupt() -> TestResult {
    let fixture = build_fixture()?;
    let paths = installed(&fixture.root);
    let tools = FakeTools::new(&fixture, Fault::TruncatedVminit);

    let error = fetch(&paths, &fixture.pin, &tools).expect_err("vminit is truncated");

    let ArtifactError::Digest { what, .. } = &error else {
        return Err(format!("expected a refusal, got {error}").into());
    };
    assert!(
        what.contains("length"),
        "a short file is reported by length: {what}"
    );
    assert!(
        !paths.vminit().exists(),
        "a truncated artifact is not installed"
    );
    Ok(())
}

/// **A pin that moved under an installed artifact is detected, not re-fetched.**
///
/// This is the case that has no failure at fetch time at all: the files on disk
/// are intact, and were correct for the pin that fetched them. `verify_installed`
/// is what notices, and it is what `gascan doctor` calls -- so the user is told
/// to act rather than having ~83MB downloaded underneath them mid-operation.
#[test]
fn a_pin_that_moved_under_an_installed_artifact_is_detected_by_digest() -> TestResult {
    let fixture = build_fixture()?;
    let paths = installed(&fixture.root);
    let tools = FakeTools::new(&fixture, Fault::None);
    fetch(&paths, &fixture.pin, &tools)?;
    verify_installed(&paths, &fixture.pin)?;

    // The pin moves; the disk does not.
    let mut moved = fixture.pin.clone();
    moved.artifacts.kernel.content.sha256 = "f".repeat(64);

    let error = verify_installed(&paths, &moved).expect_err("the pin no longer describes the disk");

    assert!(matches!(error, ArtifactError::Digest { .. }), "{error}");
    assert_eq!(
        tools.downloads().len(),
        2,
        "detecting a moved pin must not download anything"
    );
    assert!(
        paths.kernel().is_file(),
        "a moved pin must not delete the artifact the user still has"
    );
    Ok(())
}

/// **An edited `index.json` cannot make a layout pass.**
///
/// `index.json` is the one file in an OCI layout that is not content-addressed,
/// so a layout whose index was rewritten to name the expected digest would
/// satisfy a check that stopped at the descriptor. The blob it names is hashed
/// as well, and its file name IS its digest.
#[test]
fn a_layout_whose_index_was_edited_to_claim_the_right_digest_is_refused() -> TestResult {
    let fixture = build_fixture()?;
    let paths = installed(&fixture.root);
    fetch(&paths, &fixture.pin, &FakeTools::new(&fixture, Fault::None))?;

    // Keep the index's claim, replace the blob it names.
    let digest = &fixture.pin.artifacts.vminit.content.sha256;
    std::fs::write(
        paths.vminit().join("blobs").join("sha256").join(digest),
        b"these are not the manifest bytes",
    )?;

    let error = verify_installed(&paths, &fixture.pin).expect_err("the blob no longer matches");

    assert!(matches!(error, ArtifactError::Digest { .. }), "{error}");
    Ok(())
}

/// The pin this binary was built from parses, and describes the release that
/// exists. A build carrying an unparseable pin would fail only when a user ran
/// the one command they reach for when nothing else works.
#[test]
fn the_compiled_in_pin_parses_and_names_both_artifacts() -> TestResult {
    let pin = Pin::compiled_in()?;
    assert_eq!(pin.schema, 2);
    assert_eq!(pin.revision.len(), 40);
    for artifact in [&pin.artifacts.kernel, &pin.artifacts.vminit] {
        assert_eq!(artifact.sha256.len(), 64);
        assert_eq!(artifact.content.sha256.len(), 64);
        assert!(artifact.bytes > 0);
        assert!(artifact.content.bytes > 0);
        assert!(
            artifact.url.contains(&pin.tag),
            "the asset URL must agree with the tag: {} vs {}",
            artifact.url,
            pin.tag
        );
    }
    Ok(())
}

/// **The artifact directories are private, whether this run made them or found
/// them.**
///
/// `create_dir_all` applies the process umask, which is `022` on a default
/// macOS host, so every directory the fetch made arrived `0755`. MEASURED on
/// the machine where this was written, after `gascan engine fetch` had run:
/// `dev.gascan` and `dev.gascan/controller` were `700` and
/// `dev.gascan/engine` was `755`.
///
/// **Two consumers require exactly `0700` and neither repairs.** `gascand`'s
/// `ensure_private_child_directory` `fchmod`s only a directory it created and
/// then validates the mode, so a `dev.gascan` this fetch made at `0755` stops
/// the daemon from starting on either backend -- and running the fetch before
/// the first daemon start is the documented order, because the fetch is the
/// doctor's remedy for a daemon that cannot start.
/// `packaging/macos/uninstall.sh` refuses a private child that is not `0700`,
/// with exit 65, after the controller and runtime data are already gone.
///
/// The repair half is asserted as well as the create half, because every host
/// that already ran the fetch is in the broken state and nothing else fixes it.
#[test]
fn the_artifact_directories_are_private_when_created_and_when_repaired() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;

    fn mode_of(path: &Path) -> Result<u32, Box<dyn std::error::Error>> {
        Ok(std::fs::metadata(path)?.permissions().mode() & 0o7777)
    }

    // Created. The root's parent is named `dev.gascan` so the walk this
    // exercises is the production one: the chain is tightened up to and
    // including `dev.gascan`, and no further.
    let fixture = build_fixture()?;
    let support = fixture.root.join("Application Support");
    std::fs::create_dir_all(&support)?;
    let dev_gascan = support.join("dev.gascan");
    let paths = ArtifactPaths::under_application_support(&support);
    let tools = FakeTools::new(&fixture, Fault::None);
    fetch(&paths, &fixture.pin, &tools)?;

    assert_eq!(mode_of(paths.root())?, 0o700, "the engine directory");
    assert_eq!(mode_of(&dev_gascan)?, 0o700, "the dev.gascan directory");

    // Repaired. This is the state a host that already ran the old fetch is in.
    for path in [dev_gascan.as_path(), paths.root()] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    assert_eq!(
        mode_of(paths.root())?,
        0o755,
        "the fixture's own precondition"
    );

    fetch(&paths, &fixture.pin, &tools)?;
    assert_eq!(
        mode_of(paths.root())?,
        0o700,
        "the engine directory, repaired"
    );
    assert_eq!(
        mode_of(&dev_gascan)?,
        0o700,
        "the dev.gascan directory, repaired"
    );

    // Above `dev.gascan` is the account's own -- `Library` and `Application
    // Support` are macOS's, and tightening them would be this crate reaching
    // outside itself.
    assert_ne!(
        mode_of(&support)?,
        0o700,
        "the fetch tightened a directory above dev.gascan, which is not its to own"
    );
    Ok(())
}
