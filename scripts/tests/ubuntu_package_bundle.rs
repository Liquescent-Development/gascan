use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const FINGERPRINT: &str = "F6ECB3762474EDA9D21B7022871920D1991BC93C";

struct Fixture {
    temp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("repository/pool")).unwrap();
        fs::create_dir_all(root.join("signed-releases/fixture")).unwrap();
        fs::create_dir_all(root.join("signed-indexes/fixture/main/binary-arm64")).unwrap();
        fs::write(root.join("repository/pool/dep.deb"), b"dependency").unwrap();
        fs::write(root.join("repository/pool/root.deb"), b"root package").unwrap();
        fs::write(root.join("repository/pool/recommended.deb"), b"recommended").unwrap();
        fs::write(root.join("repository/pool/provider.deb"), b"provider").unwrap();
        let dep_hash = sha(root.join("repository/pool/dep.deb"));
        let root_hash = sha(root.join("repository/pool/root.deb"));
        let rec_hash = sha(root.join("repository/pool/recommended.deb"));
        let provider_hash = sha(root.join("repository/pool/provider.deb"));
        let packages = format!(
            "Package: dep\nVersion: 1.0\nArchitecture: arm64\nFilename: pool/dep.deb\nSHA256: {dep_hash}\nSize: 10\nMulti-Arch: same\n\nPackage: provider\nVersion: 3.0\nArchitecture: arm64\nFilename: pool/provider.deb\nSHA256: {provider_hash}\nSize: 8\nMulti-Arch: allowed\nProvides: virtual-dep (= 3.0)\n\nPackage: recommended\nVersion: 1.0\nArchitecture: arm64\nFilename: pool/recommended.deb\nSHA256: {rec_hash}\nSize: 11\n\nPackage: root\nVersion: 2.0\nArchitecture: arm64\nFilename: pool/root.deb\nSHA256: {root_hash}\nSize: 12\nDepends: dep:any (>= 1.0) [arm64] | virtual-dep\nPre-Depends: dep (= 1.0)\nRecommends: recommended\n\n"
        );
        fs::write(root.join("repository/Packages"), &packages).unwrap();
        sign_packages(root, &packages);
        fs::write(root.join("archive-keyring.gpg"), b"fixture keyring").unwrap();
        fs::write(root.join("roots.txt"), "provider\nroot\n").unwrap();
        fs::write(
            root.join("root-bindings.tsv"),
            "provider\tprovider\t3.0\tarm64\nroot\troot\t2.0\tarm64\n",
        )
        .unwrap();
        fs::write(
            root.join("package-manifest.tsv"),
            format!(
                "dep\t1.0\tarm64\tpool/dep.deb\t{dep_hash}\t10\nprovider\t3.0\tarm64\tpool/provider.deb\t{provider_hash}\t8\nroot\t2.0\tarm64\tpool/root.deb\t{root_hash}\t12\n"
            ),
        )
        .unwrap();
        fs::write(root.join("dependency-edges.tsv"), "root\t2.0\tarm64\tDepends\t0\tdep:any (>= 1.0) [arm64] | virtual-dep\tprovider\t3.0\tarm64\nroot\t2.0\tarm64\tPre-Depends\t0\tdep (= 1.0)\tdep\t1.0\tarm64\n").unwrap();
        fs::write(root.join("dependency-requirements.tsv"), "root\t2.0\tarm64\tDepends\t0\tdep:any (>= 1.0) [arm64] | virtual-dep\nroot\t2.0\tarm64\tPre-Depends\t0\tdep (= 1.0)\n").unwrap();
        fs::write(
            root.join("offline-apt-check.tsv"),
            "apt-simulation\tpassed\nselection-sha256\tfixture\n",
        )
        .unwrap();
        fs::write(
            root.join("provenance.env"),
            format!("SNAPSHOT=2026-07-13T00:00:00Z\nBASE_IMAGE=ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab\nSIGNING_KEY_FINGERPRINT={FINGERPRINT}\nARCHITECTURE=arm64\nINSTALL_RECOMMENDS=false\nSYSTEM_PACKAGES_PATH=tests/image/system-tools.txt\nSYSTEM_PACKAGES_SHA256=b68046c4450d7ec11362905551a793d0e4884e20b63f82b26335d2e7610acce8\n"),
        ).unwrap();
        let gpgv = root.join("gpgv");
        fs::write(&gpgv, format!("#!/bin/sh\nprintf '%s\\n' '[GNUPG:] VALIDSIG {FINGERPRINT} 20260713 0 4 0 1 10 01 {FINGERPRINT}' >&2\n")).unwrap();
        let mut mode = fs::metadata(&gpgv).unwrap().permissions();
        mode.set_mode(0o755);
        fs::set_permissions(gpgv, mode).unwrap();
        let verifier = root.join("debian-verifier");
        fs::write(&verifier, r#"#!/usr/bin/env python3
import lzma,sys
from pathlib import Path
root=Path(sys.argv[2])
text=lzma.decompress(next((root/'signed-indexes').rglob('Packages.xz')).read_bytes()).decode()
required=[]
for relation in ('Depends','Pre-Depends'):
 marker=relation+': '
 for line in text.splitlines():
  if line.startswith(marker): required.append('root\t2.0\tarm64\t'+relation+'\t0\t'+line[len(marker):])
if (root/'dependency-requirements.tsv').read_text()!='\n'.join(sorted(required))+'\n': raise SystemExit('independent requirements mismatch')
edges=(root/'dependency-edges.tsv').read_text().splitlines()
if sorted('\t'.join(line.split('\t')[:6]) for line in edges)!=sorted(required): raise SystemExit('independent chosen edges mismatch')
depends=next(line for line in edges if '\tDepends\t' in line)
pre=next(line for line in edges if '\tPre-Depends\t' in line)
if not depends.endswith('\tprovider\t3.0\tarm64') or not pre.endswith('\tdep\t1.0\tarm64'): raise SystemExit('independent architecture eligibility mismatch')
if (root/'roots.txt').read_text()!='provider\nroot\n': raise SystemExit('independent roots mismatch')
if (root/'offline-apt-check.tsv').read_text()!='apt-simulation\tpassed\nselection-sha256\tfixture\n': raise SystemExit('independent offline APT mismatch')
"#).unwrap();
        let mut mode = fs::metadata(&verifier).unwrap().permissions();
        mode.set_mode(0o755);
        fs::set_permissions(verifier, mode).unwrap();
        Self { temp }
    }

    fn root(&self) -> &Path {
        self.temp.path()
    }

    fn verify(&self) -> std::process::Output {
        Command::new(script())
            .arg("--verify-evidence")
            .arg(self.root())
            .env("GPGV", self.root().join("gpgv"))
            .env(
                "DEBIAN_EVIDENCE_VERIFIER",
                self.root().join("debian-verifier"),
            )
            .output()
            .unwrap()
    }

    fn assert_rejected(&self, needle: &str) {
        let output = self.verify();
        assert!(!output.status.success(), "fixture unexpectedly passed");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(needle), "expected {needle:?} in {stderr:?}");
    }
}

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("produce-ubuntu-package-bundle.sh")
}

fn sha(path: impl AsRef<Path>) -> String {
    let output = Command::new("shasum")
        .args(["-a", "256"])
        .arg(path.as_ref())
        .output()
        .unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned()
}

#[test]
fn package_input_digest_is_locked_in_config_image_lock_and_producer_provenance() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let digest = sha(root.join("tests/image/system-tools.txt"));
    let config =
        fs::read_to_string(root.join("images/workspace/bundles/ubuntu-packages.toml")).unwrap();
    let lock = fs::read_to_string(root.join("images/workspace/versions.lock")).unwrap();
    let producer =
        fs::read_to_string(root.join("scripts/produce-ubuntu-package-bundle.sh")).unwrap();

    let assignment = format!("system_packages_sha256 = \"{digest}\"");
    assert!(
        config.contains(&assignment),
        "package bundle config does not lock the package input digest"
    );
    assert!(
        lock.contains(&assignment),
        "image lock does not lock the package input digest"
    );
    assert!(
        producer.contains(&format!("SYSTEM_PACKAGES_SHA256={digest}")),
        "producer provenance does not bind the package input digest"
    );
    assert!(producer.contains("--download-only --no-install-recommends install"));
    assert!(producer.contains("APT::Install-Recommends=false"));
}

#[test]
fn signed_snapshot_fetches_retry_only_bounded_transient_failures() {
    let producer = fs::read_to_string(script()).unwrap();
    for required in [
        "fetch_signed_snapshot",
        "--retry 4",
        "--retry-delay 2",
        "--retry-max-time 60",
        "--connect-timeout 20",
    ] {
        assert!(
            producer.contains(required),
            "missing bounded signed-snapshot retry contract: {required}"
        );
    }
    assert!(
        !producer.contains("--retry-all-errors"),
        "permanent HTTP errors must not be retried"
    );
    assert!(
        producer.contains("\"$gpgv_bin\" --status-fd 2"),
        "successful retries must still require signature verification"
    );
}

#[test]
fn downloaded_deb_identity_is_one_unlabeled_nonempty_tuple() {
    let producer = fs::read_to_string(script()).unwrap();
    assert!(producer.contains(r#"'--showformat=${Package}\t${Version}\t${Architecture}\n'"#));
    assert!(!producer.contains(r#"['dpkg-deb','-f',str(deb),'Package','Version','Architecture']"#));
    for rejection in [
        r#"raw.count('\n') != 1"#,
        r#"columns=raw.removesuffix('\n').split('\t')"#,
        "len(columns) != 3",
        "not all(columns)",
        "ord(character) < 32 or ord(character) == 127",
    ] {
        assert!(
            producer.contains(rejection),
            "missing malformed dpkg-deb output rejection: {rejection}"
        );
    }
}

#[test]
fn package_cache_is_scoped_to_snapshot_architecture_and_reviewed_input() {
    let producer = fs::read_to_string(script()).unwrap();
    for required in [
        "UBUNTU_PACKAGE_CACHE",
        "$snapshot-arm64-$system_packages_sha256",
        "cp -- \"$package_cache\"/*.deb",
        "cp -- \"$work/apt/cache/archives\"/*.deb",
        "downloaded deb is not uniquely bound to signed Packages metadata",
        "payload hash/size mismatch against signed Packages",
    ] {
        assert!(
            producer.contains(required),
            "missing safe package cache contract: {required}"
        );
    }
}

#[test]
fn independent_solver_resolves_all_arch_sources_against_native_arm64_dependencies() {
    let verifier = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("scripts/verify-ubuntu-debian-evidence.py"),
    )
    .unwrap();
    assert!(verifier
        .contains(r#"return candidate_arch in ("arm64", "all") or multi_arch == "foreign""#));
    assert!(!verifier
        .contains(r#"return candidate_arch in (source_arch, "all") or multi_arch == "foreign""#));
}

#[test]
fn producer_verification_is_explicitly_fail_closed_and_binds_virtual_roots() {
    let producer = fs::read_to_string(script()).unwrap();
    assert!(producer.contains("<<'PY' || return 1"));
    assert!(producer.contains("\"$debian_verifier\" --verify \"$evidence\" || return 1"));
    for required in [
        "root-bindings.tsv",
        "invalid requested root provider",
        "missing root package",
    ] {
        assert!(
            producer.contains(required),
            "missing fail-closed root binding contract: {required}"
        );
    }
    let verify = producer.find("verify_evidence \"$work/evidence\"").unwrap();
    let output = producer[verify..].find("mkdir -- \"$output\"").unwrap();
    assert!(output > 0, "output must be created only after verification");
}

#[test]
fn independent_verifier_records_exact_t64_root_provider_bindings() {
    let verifier = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("scripts/verify-ubuntu-debian-evidence.py"),
    )
    .unwrap();
    for required in [
        "root-bindings.tsv",
        "ambiguous requested root binding",
        "provided_version",
        "libatk-bridge2.0-0",
        "libatk1.0-0",
        "libcups2",
    ] {
        assert!(
            verifier.contains(required),
            "missing independent root binding contract: {required}"
        );
    }
}

#[test]
fn accepts_complete_canonical_arm64_closure() {
    let fixture = Fixture::new();
    let output = fixture.verify();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_wrong_signing_key_fingerprint() {
    let fixture = Fixture::new();
    let path = fixture.root().join("provenance.env");
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace(FINGERPRINT, "0000000000000000000000000000000000000000");
    fs::write(path, text).unwrap();
    fixture.assert_rejected("fingerprint");
}

#[test]
fn rejects_invalid_inrelease_signature() {
    let fixture = Fixture::new();
    fs::write(fixture.root().join("gpgv"), "#!/bin/sh\nexit 1\n").unwrap();
    fixture.assert_rejected("signature");
}

#[test]
fn rejects_package_payload_hash_mismatch() {
    let fixture = Fixture::new();
    fs::write(fixture.root().join("repository/pool/dep.deb"), b"corrupt").unwrap();
    fixture.assert_rejected("payload hash/size");
}

#[test]
fn rejects_non_arm64_package() {
    let fixture = Fixture::new();
    let path = fixture.root().join("repository/Packages");
    let text = fs::read_to_string(&path).unwrap().replacen(
        "Architecture: arm64",
        "Architecture: amd64",
        1,
    );
    rewrite_packages(&fixture, &path, text);
    let manifest = fixture.root().join("package-manifest.tsv");
    let text =
        fs::read_to_string(&manifest)
            .unwrap()
            .replacen("dep\t1.0\tarm64", "dep\t1.0\tamd64", 1);
    fs::write(manifest, text).unwrap();
    fixture.assert_rejected("architecture");
}

#[test]
fn rejects_missing_dependency() {
    let fixture = Fixture::new();
    let path = fixture.root().join("package-manifest.tsv");
    let text = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with("dep\t"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(path, text).unwrap();
    fixture.assert_rejected("dependency");
}

#[test]
fn rejects_version_ambiguity() {
    let fixture = Fixture::new();
    let path = fixture.root().join("repository/Packages");
    let mut text = fs::read_to_string(&path).unwrap();
    let dep = text
        .split("\n\n")
        .find(|stanza| stanza.starts_with("Package: dep\n"))
        .unwrap()
        .to_owned();
    text.push_str(&dep);
    text.push_str("\n\n");
    rewrite_packages(&fixture, &path, text);
    fixture.assert_rejected("ambiguous");
}

#[test]
fn accepts_identical_package_metadata_republished_in_another_signed_index() {
    let fixture = Fixture::new();
    let root = fixture.root();
    let packages = fs::read(root.join("repository/Packages")).unwrap();
    let source = root.join("signed-indexes/fixture/main/binary-arm64/Packages.xz");
    let destination = root.join("signed-indexes/fixture/universe/binary-arm64/Packages.xz");
    fs::create_dir_all(destination.parent().unwrap()).unwrap();
    fs::copy(&source, &destination).unwrap();
    let inrelease = root.join("signed-releases/fixture/InRelease");
    let mut release = fs::read_to_string(&inrelease).unwrap();
    release.push_str(&format!(
        " {} {} universe/binary-arm64/Packages\n {} {} universe/binary-arm64/Packages.xz\n",
        sha_bytes(&packages),
        packages.len(),
        sha(&destination),
        fs::metadata(&destination).unwrap().len()
    ));
    fs::write(inrelease, release).unwrap();

    let output = fixture.verify();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_inclusion_of_recommends() {
    let fixture = Fixture::new();
    let packages = fs::read_to_string(fixture.root().join("repository/Packages")).unwrap();
    let stanza = packages
        .split("\n\n")
        .find(|s| s.starts_with("Package: recommended"))
        .unwrap();
    let fields = stanza
        .lines()
        .map(|line| line.split_once(": ").unwrap())
        .collect::<std::collections::HashMap<_, _>>();
    let path = fixture.root().join("package-manifest.tsv");
    let mut manifest = fs::read_to_string(&path).unwrap();
    manifest.push_str(&format!(
        "recommended\t{}\t{}\t{}\t{}\t{}\n",
        fields["Version"],
        fields["Architecture"],
        fields["Filename"],
        fields["SHA256"],
        fields["Size"]
    ));
    let mut lines = manifest.lines().collect::<Vec<_>>();
    lines.sort_unstable();
    fs::write(path, lines.join("\n") + "\n").unwrap();
    fixture.assert_rejected("chosen dependency edge");
}

#[test]
fn rejects_nondeterministic_manifest_ordering() {
    let fixture = Fixture::new();
    let path = fixture.root().join("package-manifest.tsv");
    let mut lines = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    lines.reverse();
    fs::write(path, lines.join("\n") + "\n").unwrap();
    fixture.assert_rejected("canonical order");
}

#[test]
fn rejects_valid_unrelated_release_with_forged_local_packages_and_deb() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root().join("repository/pool/root.deb"),
        b"forged bytes",
    )
    .unwrap();
    let forged = sha(fixture.root().join("repository/pool/root.deb"));
    let manifest = fixture.root().join("package-manifest.tsv");
    let text = fs::read_to_string(&manifest)
        .unwrap()
        .lines()
        .map(|line| {
            if line.starts_with("root\t") {
                format!("root\t9.9\tarm64\tpool/root.deb\t{forged}\t12")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(manifest, text).unwrap();
    fixture.assert_rejected("absent from Packages metadata");
}

#[test]
fn rejects_packages_index_not_covered_by_signed_release() {
    let fixture = Fixture::new();
    let index = fixture
        .root()
        .join("signed-indexes/fixture/main/binary-arm64/Packages.xz");
    fs::write(index, b"not the signed index").unwrap();
    fixture.assert_rejected("compressed Packages hash/size");
}

#[test]
fn rejects_missing_chosen_pre_depends_edge() {
    let fixture = Fixture::new();
    let path = fixture.root().join("dependency-edges.tsv");
    let text = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .filter(|line| !line.contains("\tPre-Depends\t"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(path, text).unwrap();
    fixture.assert_rejected("chosen dependency edge");
}

#[test]
fn rejects_missing_chosen_depends_edge() {
    let fixture = Fixture::new();
    rewrite_edges(&fixture, |line| !line.contains("\tDepends\t"));
    fixture.assert_rejected("chosen dependency edge");
}

#[test]
fn rejects_changed_version_or_arch_qualified_requirement() {
    let fixture = Fixture::new();
    let path = fixture.root().join("dependency-edges.tsv");
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace("(>= 1.0) [arm64]", "(>= 9.0) [amd64]");
    fs::write(path, text).unwrap();
    fixture.assert_rejected("chosen dependency edge");
}

#[test]
fn rejects_chosen_multi_arch_target_not_in_exact_selection() {
    let fixture = Fixture::new();
    let path = fixture.root().join("dependency-edges.tsv");
    let text =
        fs::read_to_string(&path)
            .unwrap()
            .replacen("dep\t1.0\tarm64\n", "dep\t1.0\tamd64\n", 1);
    fs::write(path, text).unwrap();
    fixture.assert_rejected("unselected package");
}

#[test]
fn rejects_ineligible_first_alternative_when_later_provider_is_eligible() {
    let fixture = Fixture::new();
    let path = fixture.root().join("dependency-edges.tsv");
    let text = fs::read_to_string(&path).unwrap().replacen(
        "\tprovider\t3.0\tarm64\n",
        "\tdep\t1.0\tarm64\n",
        1,
    );
    fs::write(path, text).unwrap();
    fixture.assert_rejected("architecture eligibility");
}

#[test]
fn accepts_normalized_debian_semantics_and_multiple_selected_alternatives() {
    let fixture = Fixture::new();
    let output = fixture.verify();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let requirements =
        fs::read_to_string(fixture.root().join("dependency-requirements.tsv")).unwrap();
    assert!(requirements.contains("Pre-Depends"));
    assert!(requirements.contains("dep:any (>= 1.0) [arm64] | virtual-dep"));
}

#[test]
fn workflow_separates_read_only_production_from_revalidated_publication() {
    let workflow = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../.github/workflows/workspace-bundles.yml"),
    )
    .unwrap();
    assert!(workflow.contains("contents: read"));
    assert!(workflow.contains("contents: write"));
    assert!(workflow.contains("persist-credentials: false"));
    assert!(workflow.contains("needs: ubuntu-packages-linux-arm64"));
    assert!(workflow.contains("expected_sha="));
    assert!(workflow.contains("validate-workspace-bundle"));
    assert!(workflow.contains("validation-receipt.tsv"));
    assert!(workflow.contains("needs: validate-ubuntu-packages-linux-arm64"));
    assert!(!workflow.contains("actions/checkout@v"));
    assert!(!workflow.contains("actions/upload-artifact@v"));
    assert!(!workflow.contains("actions/download-artifact@v"));
}

#[test]
fn independent_recomputation_rejects_deleted_depends_and_pre_depends_selection() {
    let fixture = Fixture::new();
    for name in [
        "package-manifest.tsv",
        "dependency-requirements.tsv",
        "dependency-edges.tsv",
    ] {
        let path = fixture.root().join(name);
        let text = fs::read_to_string(&path)
            .unwrap()
            .lines()
            .filter(|line| {
                !line.starts_with("dep\t")
                    && !line.contains("\tDepends\t")
                    && !line.contains("\tPre-Depends\t")
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(path, text).unwrap();
    }
    fixture.assert_rejected("independent");
}

#[test]
fn independent_offline_apt_result_cannot_be_altered() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root().join("offline-apt-check.tsv"),
        "apt-simulation\tforged\n",
    )
    .unwrap();
    fixture.assert_rejected("offline APT");
}

#[test]
fn independent_roots_reject_deleted_reviewed_root() {
    let fixture = Fixture::new();
    fs::write(fixture.root().join("roots.txt"), "root\n").unwrap();
    fixture.assert_rejected("roots");
}

fn rewrite_packages(fixture: &Fixture, path: &Path, text: String) {
    fs::write(path, &text).unwrap();
    sign_packages(fixture.root(), &text);
}

fn sign_packages(root: &Path, text: &str) {
    let plain = root.join("signed-indexes/fixture/main/binary-arm64/Packages");
    let compressed = root.join("signed-indexes/fixture/main/binary-arm64/Packages.xz");
    fs::write(&plain, text).unwrap();
    let output = Command::new("xz")
        .args(["--check=crc32", "--stdout"])
        .arg(&plain)
        .output()
        .unwrap();
    assert!(output.status.success());
    fs::write(&compressed, output.stdout).unwrap();
    fs::remove_file(&plain).unwrap();
    fs::write(
        root.join("signed-releases/fixture/InRelease"),
        format!(
            "SHA256:\n {} {} main/binary-arm64/Packages\n {} {} main/binary-arm64/Packages.xz\n",
            sha_bytes(text.as_bytes()),
            text.len(),
            sha(&compressed),
            fs::metadata(&compressed).unwrap().len()
        ),
    )
    .unwrap();
}

fn rewrite_edges(fixture: &Fixture, keep: impl Fn(&str) -> bool) {
    let path = fixture.root().join("dependency-edges.tsv");
    let text = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .filter(|line| keep(line))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(path, text).unwrap();
}

fn sha_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
