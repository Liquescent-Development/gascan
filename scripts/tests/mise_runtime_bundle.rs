use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const CURRENT: &str = r#"{"elixir":"1.20.2-otp-29","erlang":"29.0.3","go":"1.26.5","java":"25.0.2","node":"24.18.0","python":"3.14.6","ruby":"3.4.10","rust":"1.97.0"}"#;

fn producer() -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("scripts/produce-mise-runtime-bundle.sh"),
    )
    .unwrap()
}

#[test]
fn production_installs_exact_erlang_before_user_facing_runtimes() {
    let producer = producer();
    let erlang = producer
        .find(r#""$mise" install --yes erlang@29.0.3 2>"$work/logs/erlang.log""#)
        .unwrap();
    let runtimes = producer
        .find("tools=(go java node python ruby rust)")
        .unwrap();
    let elixir = producer
        .find(r#""$mise" exec erlang@29.0.3 -- "$mise" install --yes elixir@1.20.2-otp-29"#)
        .unwrap();
    assert!(erlang < elixir && elixir < runtimes);
}

#[test]
fn native_verifier_executes_erlang_and_requires_exact_otp_major() {
    let producer = producer();
    assert!(producer.contains(r#""erlang":["-noshell","-eval""#));
    assert!(producer.contains(r#""erlang":output=="29""#));
}

#[test]
fn producer_uses_supported_mise_ls_and_strict_normalization() {
    let producer = producer();
    assert!(producer.contains("ls --current --installed --json"));
    assert!(!producer.contains("current --json"));
    assert!(producer.contains("invalid mise ls record"));
    assert!(producer.contains(".value|length)!=1"));
    assert!(producer.contains(".value[0].installed != true"));
    assert!(producer.contains(".value[0].active != true"));
}

struct Fixture {
    temp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for (tool, version, executable) in [
            ("elixir", "1.20.2-otp-29", "elixir"),
            ("erlang", "29.0.3", "erl"),
            ("go", "1.26.5", "go"),
            ("java", "25.0.2", "java"),
            ("node", "24.18.0", "node"),
            ("python", "3.14.6", "python"),
            ("ruby", "3.4.10", "ruby"),
            ("rust", "1.97.0", "rustc"),
        ] {
            let binary = root.join(format!(
                "tree/opt/gascan/mise/installs/{tool}/{version}/bin/{executable}"
            ));
            fs::create_dir_all(binary.parent().unwrap()).unwrap();
            let mut elf = vec![0_u8; 128];
            elf[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
            elf[16..18].copy_from_slice(&2_u16.to_le_bytes());
            elf[18..20].copy_from_slice(&183_u16.to_le_bytes());
            fs::write(&binary, elf).unwrap();
            fs::set_permissions(binary, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(root.join("mise-current.json"), CURRENT).unwrap();
        fs::write(root.join("provenance.env"), "PLATFORM=linux/arm64\nMISE_VERSION=2026.5.0\nMISE_SHA256=fba7c8a383cf3c59eb5a9995d5299fd2c78eba7eb1daace48d75fe491362f79a\nCONFIG_SHA256=b72f66102d09e065b3778c0d6dd52c77a3ef404c2687d910c943d5682cb3063f\nBASE_IMAGE=ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab\n").unwrap();
        fs::create_dir_all(root.join("downloads")).unwrap();
        let downloads = [
            ("elixir", "1.20.2-otp-29"),
            ("erlang", "29.0.3"),
            ("go", "1.26.5"),
            ("java", "25.0.2"),
            ("node", "24.18.0"),
            ("python", "3.14.6"),
            ("ruby", "3.4.10"),
            ("rust", "1.97.0"),
        ]
        .into_iter()
        .map(|(tool, version)| {
            let body = format!("real-shaped-upstream-artifact:{tool}:{version}\n");
            fs::write(root.join(format!("downloads/{tool}.artifact")), &body).unwrap();
            use sha2::{Digest, Sha256};
            format!(
                "{tool}\t{version}\tcore\thttps://github.com/upstream/{tool}/releases/download/v{version}/{tool}-linux-arm64\t{:x}\t{}\tdownloads/{tool}.artifact\t/opt/gascan/mise/downloads/{tool}/{version}/{tool}-linux-arm64",
                Sha256::digest(body.as_bytes()), body.len()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
            + "\n";
        fs::write(root.join("upstream-artifacts.tsv"), downloads).unwrap();
        let mut lock = String::from("[tools]\n");
        for (tool, version) in [
            ("elixir", "1.20.2-otp-29"),
            ("erlang", "29.0.3"),
            ("go", "1.26.5"),
            ("java", "25.0.2"),
            ("node", "24.18.0"),
            ("python", "3.14.6"),
            ("ruby", "3.4.10"),
            ("rust", "1.97.0"),
        ] {
            let body = fs::read(root.join(format!("downloads/{tool}.artifact"))).unwrap();
            use sha2::{Digest, Sha256};
            lock.push_str(&format!("{tool} = [{{ version = \"{version}\", backend = \"core\", platforms = {{ linux-arm64 = {{ url = \"https://github.com/upstream/{tool}/releases/download/v{version}/{tool}-linux-arm64\", checksum = \"sha256:{:x}\" }} }} }}]\n", Sha256::digest(body)));
        }
        fs::write(root.join("mise.lock"), lock).unwrap();
        fs::create_dir_all(root.join("mise-install-logs")).unwrap();
        for (tool, version) in [
            ("elixir", "1.20.2-otp-29"),
            ("erlang", "29.0.3"),
            ("go", "1.26.5"),
            ("java", "25.0.2"),
            ("node", "24.18.0"),
            ("python", "3.14.6"),
            ("ruby", "3.4.10"),
            ("rust", "1.97.0"),
        ] {
            fs::write(root.join(format!("mise-install-logs/{tool}.log")),format!("DEBUG GET Downloading https://github.com/upstream/{tool}/releases/download/v{version}/{tool}-linux-arm64 to /opt/gascan/mise/downloads/{tool}/{version}/{tool}-linux-arm64\n")).unwrap();
        }
        fs::write(root.join("base-attestation.env"), "WORKFLOW_COMMIT=1111111111111111111111111111111111111111\nIMAGE_DIGEST=ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab\nIMAGE_ID=sha256:2222222222222222222222222222222222222222222222222222222222222222\nPLATFORM=linux/arm64\nINVOCATION=docker-run-read-only-attestation-v1\nUBUNTU_BUNDLE_SHA256=3333333333333333333333333333333333333333333333333333333333333333\n").unwrap();
        use sha2::{Digest, Sha256};
        let attestation = fs::read(root.join("base-attestation.env")).unwrap();
        let provenance = fs::read_to_string(root.join("provenance.env")).unwrap();
        fs::write(
            root.join("provenance.env"),
            format!(
                "{provenance}BASE_ATTESTATION_SHA256={:x}\n",
                Sha256::digest(attestation)
            ),
        )
        .unwrap();
        Self::refresh(root);
        Self { temp }
    }
    fn root(&self) -> &Path {
        self.temp.path()
    }
    fn refresh(root: &Path) {
        let tree = root.join("tree");
        let output = Command::new("python3").arg("-c").arg(r#"import hashlib,os,stat,sys
from pathlib import Path
r=Path(sys.argv[1]); rows=[]
for p in sorted(r.rglob('*')):
 q=p.relative_to(r).as_posix(); s=p.lstat(); mode=stat.S_IMODE(s.st_mode)
 if p.is_symlink(): rows.append(f'{q}\tsymlink\t{mode:04o}\t0\t0\t0\t-\t{os.readlink(p)}')
 elif p.is_dir(): rows.append(f'{q}\tdirectory\t{mode:04o}\t0\t0\t0\t-\t-')
 elif p.is_file():
  b=p.read_bytes(); rows.append(f'{q}\tfile\t{mode:04o}\t0\t0\t{len(b)}\t{hashlib.sha256(b).hexdigest()}\t-')
(r.parent/'mise-runtimes-linux-arm64.manifest.tsv').write_text('\n'.join(rows)+'\n')
"#).arg(&tree).output().unwrap();
        assert!(output.status.success());
        let output = fs::File::create(root.join("bundle.tar")).unwrap();
        let mut archive = tar::Builder::new(output);
        archive.mode(tar::HeaderMode::Deterministic);
        archive.append_dir_all("opt", tree.join("opt")).unwrap();
        archive.finish().unwrap();
        let input = fs::File::open(root.join("bundle.tar")).unwrap();
        let output = fs::File::create(root.join("mise-runtimes-linux-arm64.tar.zst")).unwrap();
        zstd::stream::copy_encode(input, output, 1).unwrap();
        let archive = fs::read(root.join("mise-runtimes-linux-arm64.tar.zst")).unwrap();
        use sha2::{Digest, Sha256};
        fs::write(
            root.join("mise-runtimes-linux-arm64.tar.zst.sha256"),
            format!("{:x}\n", Sha256::digest(&archive)),
        )
        .unwrap();
        fs::write(
            root.join("mise-runtimes-linux-arm64.tar.zst.size"),
            format!("{}\n", archive.len()),
        )
        .unwrap();
    }
    fn verify(&self) -> std::process::Output {
        Command::new(script())
            .arg("--verify-evidence")
            .arg(self.root())
            .output()
            .unwrap()
    }
    fn reject(&self, needle: &str) {
        let o = self.verify();
        assert!(!o.status.success());
        let e = String::from_utf8_lossy(&o.stderr);
        assert!(e.contains(needle), "expected {needle:?} in {e:?}");
    }
    fn replace(&self, file: &str, from: &str, to: &str) {
        let p = self.root().join(file);
        fs::write(&p, fs::read_to_string(&p).unwrap().replace(from, to)).unwrap();
    }
}

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("produce-mise-runtime-bundle.sh")
}

#[test]
fn accepts_exact_runtime_evidence() {
    let f = Fixture::new();
    let o = f.verify();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}
#[test]
fn rejects_wrong_platform() {
    let f = Fixture::new();
    f.replace("provenance.env", "linux/arm64", "linux/amd64");
    f.reject("platform");
}
#[test]
fn rejects_wrong_mise_version() {
    let f = Fixture::new();
    f.replace("provenance.env", "2026.5.0", "2026.5.1");
    f.reject("mise version");
}
#[test]
fn rejects_wrong_config_digest() {
    let f = Fixture::new();
    f.replace(
        "provenance.env",
        "b72f66102d09e065b3778c0d6dd52c77a3ef404c2687d910c943d5682cb3063f",
        &"0".repeat(64),
    );
    f.reject("config digest");
}
#[test]
fn rejects_wrong_mise_digest() {
    let f = Fixture::new();
    f.replace(
        "provenance.env",
        "fba7c8a383cf3c59eb5a9995d5299fd2c78eba7eb1daace48d75fe491362f79a",
        &"0".repeat(64),
    );
    f.reject("mise digest");
}
#[test]
fn rejects_missing_mise_lock_provenance() {
    let f = Fixture::new();
    fs::remove_file(f.root().join("mise.lock")).unwrap();
    f.reject("mise.lock");
}
#[test]
fn rejects_provenance_not_bound_to_mise_lock() {
    let f = Fixture::new();
    f.replace(
        "mise.lock",
        "https://github.com/upstream/node/",
        "https://github.com/forged/node/",
    );
    f.reject("mise lock provenance");
}
#[test]
fn rejects_missing_base_attestation() {
    let f = Fixture::new();
    fs::remove_file(f.root().join("base-attestation.env")).unwrap();
    f.reject("base-attestation.env");
}
#[test]
fn rejects_tampered_captured_download() {
    let f = Fixture::new();
    fs::write(f.root().join("downloads/node.artifact"), b"forged").unwrap();
    f.reject("downloaded artifact");
}
#[test]
fn rejects_absent_actual_download_event() {
    let f = Fixture::new();
    fs::write(f.root().join("mise-install-logs/node.log"), b"").unwrap();
    f.reject("actual download event");
}
#[test]
fn rejects_lock_consistent_provenance_with_mismatched_actual_event() {
    let f = Fixture::new();
    f.replace(
        "mise-install-logs/node.log",
        "https://github.com/upstream/node/",
        "https://github.com/other/node/",
    );
    f.reject("actual download event");
}
#[test]
fn rejects_nondeterministic_prefix_in_sanitized_event_evidence() {
    let f = Fixture::new();
    f.replace(
        "mise-install-logs/node.log",
        "DEBUG GET",
        "2026-07-14T12:00:00Z DEBUG GET",
    );
    f.reject("sanitized actual download event");
}
#[test]
fn rejects_tiny_executable_stub() {
    let f = Fixture::new();
    fs::write(
        f.root()
            .join("tree/opt/gascan/mise/installs/node/24.18.0/bin/node"),
        b"#!/bin/sh\n",
    )
    .unwrap();
    Fixture::refresh(f.root());
    f.reject("executable format");
}
#[test]
fn rejects_missing_tool() {
    let f = Fixture::new();
    f.replace("mise-current.json", r#","rust":"1.97.0""#, "");
    f.reject("exact seven runtimes and Erlang dependency");
}
#[test]
fn rejects_extra_tool() {
    let f = Fixture::new();
    f.replace(
        "mise-current.json",
        r#""rust":"1.97.0"}"#,
        r#""rust":"1.97.0","zig":"1.0"}"#,
    );
    f.reject("exact seven runtimes and Erlang dependency");
}
#[test]
fn rejects_wrong_tool_version() {
    let f = Fixture::new();
    f.replace("mise-current.json", "24.18.0", "24.18.1");
    f.reject("tool version");
}
#[test]
fn rejects_missing_executable() {
    let f = Fixture::new();
    fs::remove_file(
        f.root()
            .join("tree/opt/gascan/mise/installs/node/24.18.0/bin/node"),
    )
    .unwrap();
    Fixture::refresh(f.root());
    f.reject("executable");
}
#[test]
fn rejects_writable_tree_entry() {
    let f = Fixture::new();
    f.replace(
        "mise-runtimes-linux-arm64.manifest.tsv",
        "0755\t0\t0\t128",
        "0777\t0\t0\t128",
    );
    f.reject("writable");
}
#[test]
fn rejects_non_root_ownership_evidence() {
    let f = Fixture::new();
    f.replace(
        "mise-runtimes-linux-arm64.manifest.tsv",
        "0755\t0\t0\t128",
        "0755\t1000\t0\t128",
    );
    f.reject("ownership");
}
#[test]
fn rejects_unsorted_manifest() {
    let f = Fixture::new();
    let p = f.root().join("mise-runtimes-linux-arm64.manifest.tsv");
    let mut l = fs::read_to_string(&p)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    l.reverse();
    fs::write(p, l.join("\n") + "\n").unwrap();
    f.reject("canonical order");
}
#[test]
fn rejects_unsafe_archive_entry() {
    let f = Fixture::new();
    let tar = f.root().join("unsafe.tar");
    let s=Command::new("python3").arg("-c").arg("import tarfile,sys; t=tarfile.open(sys.argv[1],'w'); i=tarfile.TarInfo('../escape'); i.size=1; import io; t.addfile(i,io.BytesIO(b'x')); t.close()").arg(&tar).status().unwrap();
    assert!(s.success());
    zstd::stream::copy_encode(
        fs::File::open(tar).unwrap(),
        fs::File::create(f.root().join("mise-runtimes-linux-arm64.tar.zst")).unwrap(),
        1,
    )
    .unwrap();
    f.reject("unsafe archive");
}

#[test]
fn workflow_is_connected_arm64_and_privilege_separated() {
    let text = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../.github/workflows/workspace-bundles.yml"),
    )
    .unwrap();
    assert!(text.contains("mise-runtimes-linux-arm64"));
    assert!(text.contains("runs-on: ubuntu-24.04-arm"));
    assert!(text.contains("docker run --rm --platform linux/arm64"));
    assert!(text.contains(
        "ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab"
    ));
    assert!(text.contains("--verify-evidence"));
    assert!(text.contains("cmp --silent"));
    assert!(text.contains("contents: read"));
    assert!(!text.contains("publish-mise-runtimes-linux-arm64"));
    let mise_jobs = text.split("  ubuntu-packages-linux-arm64:").next().unwrap();
    assert!(!mise_jobs.contains("apt-get install"));
    for required in [
        "file:/ubuntu-evidence/repository",
        "Dir::Bin::Methods::http=/bin/false",
        "Dir::Bin::Methods::https=/bin/false",
        "--no-download --no-install-recommends install",
        "dpkg --audit",
        "package-manifest.tsv",
        "find . -type f -print0",
        "find . -mindepth 1 -printf",
        "sort -z | xargs -0 sha256sum",
    ] {
        assert!(
            mise_jobs.contains(required),
            "missing workflow guard: {required}"
        );
    }
    let producer = fs::read_to_string(script()).unwrap();
    assert!(producer.contains("MISE_DATA_DIR=/opt/gascan/mise"));
}
