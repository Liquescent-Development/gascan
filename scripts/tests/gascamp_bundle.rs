use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const REVISION: &str = "f6b248c5926240856dbea83d1d2c5c90ea1c1456";

struct Fixture(tempfile::TempDir);

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("tree/.cargo")).unwrap();
        fs::create_dir_all(root.join("tree/source/src")).unwrap();
        fs::create_dir_all(root.join("tree/vendor/demo-1.0.0/src")).unwrap();
        fs::write(
            root.join("tree/source/Cargo.toml"),
            "[package]\nname='camp'\nversion='0.1.0'\n[dependencies]\ndemo='1'\n",
        )
        .unwrap();
        fs::write(root.join("tree/source/Cargo.lock"), "version = 4\n\n[[package]]\nname = \"camp\"\nversion = \"0.1.0\"\ndependencies = [\"demo\"]\n\n[[package]]\nname = \"demo\"\nversion = \"1.0.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\n").unwrap();
        fs::write(root.join("tree/source/src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(
            root.join("tree/vendor/demo-1.0.0/Cargo.toml"),
            "[package]\nname='demo'\nversion='1.0.0'\n",
        )
        .unwrap();
        fs::write(
            root.join("tree/vendor/demo-1.0.0/src/lib.rs"),
            "pub fn demo() {}\n",
        )
        .unwrap();
        let crate_checksum = format!(
            "{{\"files\":{{\"Cargo.toml\":\"{}\",\"src/lib.rs\":\"{}\"}},\"package\":\"{}\"}}\n",
            sha(root.join("tree/vendor/demo-1.0.0/Cargo.toml")),
            sha(root.join("tree/vendor/demo-1.0.0/src/lib.rs")),
            "a".repeat(64)
        );
        fs::write(
            root.join("tree/vendor/demo-1.0.0/.cargo-checksum.json"),
            crate_checksum,
        )
        .unwrap();
        fs::write(root.join("tree/.cargo/config.toml"), "[net]\noffline = true\n\n[source.crates-io]\nreplace-with = \"vendored-sources\"\n\n[source.vendored-sources]\ndirectory = \"vendor\"\n").unwrap();
        Self::refresh(root);
        Self(temp)
    }

    fn root(&self) -> &Path {
        self.0.path()
    }

    fn refresh(root: &Path) {
        manifest(root.join("tree/source"), root.join("source-tree.tsv"));
        manifest(root.join("tree/vendor"), root.join("vendor-tree.tsv"));
        let source_digest = sha(root.join("source-tree.tsv"));
        let vendor_digest = sha(root.join("vendor-tree.tsv"));
        let config_digest = sha(root.join("tree/.cargo/config.toml"));
        fs::write(root.join("provenance.env"), format!("REVISION={REVISION}\nFETCHED_HEAD={REVISION}\nGIT_TREE=71e706057023049b8d15839cedd1fcd0b4a85968\nSOURCE_MANIFEST_SHA256={source_digest}\nVENDOR_MANIFEST_SHA256={vendor_digest}\nCONFIG_SHA256={config_digest}\nCARGO_VENDOR_LOCKED=true\nPLATFORM=linux/arm64\nSUBMODULES=absent\n")).unwrap();
    }

    fn verify(&self) -> std::process::Output {
        Command::new(script())
            .arg("--verify-evidence")
            .arg(self.root())
            .output()
            .unwrap()
    }

    fn reject(&self, needle: &str) {
        let output = self.verify();
        assert!(!output.status.success(), "fixture unexpectedly passed");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(needle), "expected {needle:?} in {stderr:?}");
    }
}

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("produce-gascamp-bundle.sh")
}

fn sha(path: impl AsRef<Path>) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

fn manifest(root: PathBuf, output: PathBuf) {
    let mut rows = Vec::new();
    for entry in walkdir(&root) {
        let relative = entry
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let metadata = fs::metadata(&entry).unwrap();
        rows.push(format!(
            "{}\tfile\t{:04o}\t{}\t{}\t-",
            relative,
            metadata.permissions().mode() & 0o7777,
            metadata.len(),
            sha(&entry)
        ));
    }
    rows.sort();
    fs::write(output, rows.join("\n") + "\n").unwrap();
}

fn walkdir(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for item in fs::read_dir(root).unwrap() {
        let path = item.unwrap().path();
        if path.is_dir() {
            out.extend(walkdir(&path));
        } else {
            out.push(path);
        }
    }
    out
}

#[test]
fn accepts_exact_source_and_vendor_evidence() {
    let fixture = Fixture::new();
    let output = fixture.verify();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_commit_mismatch() {
    let fixture = Fixture::new();
    let path = fixture.root().join("provenance.env");
    fs::write(
        &path,
        fs::read_to_string(&path)
            .unwrap()
            .replace(REVISION, &"0".repeat(40)),
    )
    .unwrap();
    fixture.reject("revision");
}

#[test]
fn rejects_tree_mismatch() {
    let fixture = Fixture::new();
    let path = fixture.root().join("provenance.env");
    let text = fs::read_to_string(&path).unwrap();
    let text = text
        .lines()
        .map(|line| {
            if line.starts_with("GIT_TREE=") {
                "GIT_TREE=0000000000000000000000000000000000000000"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(&path, text).unwrap();
    fixture.reject("git tree");
}

#[test]
fn rejects_dirty_or_extra_source() {
    let fixture = Fixture::new();
    fs::write(fixture.root().join("tree/source/extra"), "dirty").unwrap();
    fixture.reject("source tree");
}

#[test]
fn rejects_submodule_ambiguity() {
    let fixture = Fixture::new();
    let path = fixture.root().join("provenance.env");
    fs::write(
        &path,
        fs::read_to_string(&path)
            .unwrap()
            .replace("SUBMODULES=absent", "SUBMODULES=present"),
    )
    .unwrap();
    fixture.reject("submodule");
}

#[test]
fn rejects_altered_vendored_crate() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root().join("tree/vendor/demo-1.0.0/src/lib.rs"),
        "forged",
    )
    .unwrap();
    fixture.reject("vendor tree");
}

#[test]
fn rejects_missing_vendored_crate() {
    let fixture = Fixture::new();
    fs::remove_dir_all(fixture.root().join("tree/vendor/demo-1.0.0")).unwrap();
    fixture.reject("vendor tree");
}

#[test]
fn rejects_unlocked_git_dependency() {
    let fixture = Fixture::new();
    let path = fixture.root().join("tree/source/Cargo.toml");
    fs::write(
        &path,
        fs::read_to_string(&path)
            .unwrap()
            .replace("demo='1'", "demo={git='https://example.invalid/demo'}"),
    )
    .unwrap();
    Fixture::refresh(fixture.root());
    fixture.reject("git dependency");
}

#[test]
fn rejects_absent_cargo_checksum() {
    let fixture = Fixture::new();
    fs::remove_file(
        fixture
            .root()
            .join("tree/vendor/demo-1.0.0/.cargo-checksum.json"),
    )
    .unwrap();
    Fixture::refresh(fixture.root());
    fixture.reject("cargo checksum");
}

#[test]
fn rejects_registry_or_network_enabled_config() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root().join("tree/.cargo/config.toml"),
        "[net]\noffline = false\n",
    )
    .unwrap();
    Fixture::refresh(fixture.root());
    fixture.reject("Cargo config");
}
