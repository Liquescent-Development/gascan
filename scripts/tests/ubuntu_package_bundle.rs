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
        fs::write(root.join("repository/pool/dep.deb"), b"dependency").unwrap();
        fs::write(root.join("repository/pool/root.deb"), b"root package").unwrap();
        fs::write(root.join("repository/pool/recommended.deb"), b"recommended").unwrap();
        let dep_hash = sha(root.join("repository/pool/dep.deb"));
        let root_hash = sha(root.join("repository/pool/root.deb"));
        let rec_hash = sha(root.join("repository/pool/recommended.deb"));
        let packages = format!(
            "Package: dep\nVersion: 1.0\nArchitecture: arm64\nFilename: pool/dep.deb\nSHA256: {dep_hash}\n\nPackage: recommended\nVersion: 1.0\nArchitecture: arm64\nFilename: pool/recommended.deb\nSHA256: {rec_hash}\n\nPackage: root\nVersion: 2.0\nArchitecture: arm64\nFilename: pool/root.deb\nSHA256: {root_hash}\nDepends: dep (= 1.0)\nRecommends: recommended\n\n"
        );
        fs::write(root.join("repository/Packages"), &packages).unwrap();
        let packages_hash = sha(root.join("repository/Packages"));
        fs::write(
            root.join("InRelease"),
            format!(
                "SHA256:\n {packages_hash} {} repository/Packages\n",
                packages.len()
            ),
        )
        .unwrap();
        fs::write(root.join("archive-keyring.gpg"), b"fixture keyring").unwrap();
        fs::write(root.join("roots.txt"), "root\n").unwrap();
        fs::write(
            root.join("package-manifest.tsv"),
            format!(
                "dep\t1.0\tarm64\tpool/dep.deb\t{dep_hash}\nroot\t2.0\tarm64\tpool/root.deb\t{root_hash}\n"
            ),
        )
        .unwrap();
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
    fixture.assert_rejected("payload SHA-256");
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
    text.push_str(&text.replace("Version: 1.0", "Version: 1.1"));
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
        "recommended\t{}\t{}\t{}\t{}\n",
        fields["Version"], fields["Architecture"], fields["Filename"], fields["SHA256"]
    ));
    let mut lines = manifest.lines().collect::<Vec<_>>();
    lines.sort_unstable();
    fs::write(path, lines.join("\n") + "\n").unwrap();
    fixture.assert_rejected("Recommends");
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

fn rewrite_packages(fixture: &Fixture, path: &Path, text: String) {
    fs::write(path, &text).unwrap();
    let inrelease = fixture.root().join("InRelease");
    fs::write(
        inrelease,
        format!(
            "SHA256:\n {} {} repository/Packages\n",
            sha(path),
            text.len()
        ),
    )
    .unwrap();
}
