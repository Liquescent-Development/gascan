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
            "Package: dep\nVersion: 1.0\nArchitecture: arm64\nFilename: pool/dep.deb\nSHA256: {dep_hash}\nSize: 10\nMulti-Arch: same\n\nPackage: provider\nVersion: 3.0\nArchitecture: arm64\nFilename: pool/provider.deb\nSHA256: {provider_hash}\nSize: 8\nProvides: virtual-dep (= 3.0)\n\nPackage: recommended\nVersion: 1.0\nArchitecture: arm64\nFilename: pool/recommended.deb\nSHA256: {rec_hash}\nSize: 11\n\nPackage: root\nVersion: 2.0\nArchitecture: arm64\nFilename: pool/root.deb\nSHA256: {root_hash}\nSize: 12\nDepends: dep:any (>= 1.0) [arm64] | virtual-dep\nPre-Depends: dep (= 1.0)\nRecommends: recommended\n\n"
        );
        fs::write(root.join("repository/Packages"), &packages).unwrap();
        sign_packages(root, &packages);
        fs::write(root.join("archive-keyring.gpg"), b"fixture keyring").unwrap();
        fs::write(root.join("roots.txt"), "provider\nroot\n").unwrap();
        fs::write(
            root.join("package-manifest.tsv"),
            format!(
                "dep\t1.0\tarm64\tpool/dep.deb\t{dep_hash}\t10\nprovider\t3.0\tarm64\tpool/provider.deb\t{provider_hash}\t8\nroot\t2.0\tarm64\tpool/root.deb\t{root_hash}\t12\n"
            ),
        )
        .unwrap();
        fs::write(root.join("dependency-edges.tsv"), "root\t2.0\tarm64\tDepends\t0\tdep:any (>= 1.0) [arm64] | virtual-dep\tdep\t1.0\tarm64\nroot\t2.0\tarm64\tPre-Depends\t0\tdep (= 1.0)\tdep\t1.0\tarm64\n").unwrap();
        fs::write(root.join("dependency-requirements.tsv"), "root\t2.0\tarm64\tDepends\t0\tdep:any (>= 1.0) [arm64] | virtual-dep\nroot\t2.0\tarm64\tPre-Depends\t0\tdep (= 1.0)\n").unwrap();
        fs::write(
            root.join("provenance.env"),
            format!("SNAPSHOT=2026-07-13T00:00:00Z\nBASE_IMAGE=ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab\nSIGNING_KEY_FINGERPRINT={FINGERPRINT}\nARCHITECTURE=arm64\nINSTALL_RECOMMENDS=false\n"),
        ).unwrap();
        let gpgv = root.join("gpgv");
        fs::write(&gpgv, format!("#!/bin/sh\nprintf '%s\\n' '[GNUPG:] VALIDSIG {FINGERPRINT} 20260713 0 4 0 1 10 01 {FINGERPRINT}' >&2\n")).unwrap();
        let mut mode = fs::metadata(&gpgv).unwrap().permissions();
        mode.set_mode(0o755);
        fs::set_permissions(gpgv, mode).unwrap();
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
fn accepts_canonical_virtual_provider_as_chosen_alternative() {
    let fixture = Fixture::new();
    let path = fixture.root().join("dependency-edges.tsv");
    let text = fs::read_to_string(&path).unwrap().replacen(
        "\tdep\t1.0\tarm64\n",
        "\tprovider\t3.0\tarm64\n",
        1,
    );
    fs::write(path, text).unwrap();
    let output = fixture.verify();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
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
    assert!(!workflow.contains("actions/checkout@v"));
    assert!(!workflow.contains("actions/upload-artifact@v"));
    assert!(!workflow.contains("actions/download-artifact@v"));
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
