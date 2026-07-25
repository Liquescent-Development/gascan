use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const FINGERPRINT: &str = "F6ECB3762474EDA9D21B7022871920D1991BC93C";

struct Fixture {
    temp: tempfile::TempDir,
}

struct RuntimeCommandFixture {
    _temp: tempfile::TempDir,
    evidence: PathBuf,
    bin: PathBuf,
    dpkg_query: PathBuf,
}

impl RuntimeCommandFixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let evidence = temp.path().join("evidence");
        let bin = temp.path().join("bin");
        fs::create_dir(&evidence).unwrap();
        fs::create_dir(&bin).unwrap();
        let mappings = [
            ("dig", "bind9-dnsutils"),
            ("file", "file"),
            ("ifconfig", "net-tools"),
            ("ip", "iproute2"),
            ("nano", "nano"),
            ("netstat", "net-tools"),
            ("nslookup", "bind9-dnsutils"),
            ("pico", "nano"),
            ("ping", "iputils-ping"),
            ("ps", "procps"),
            ("pstree", "psmisc"),
            ("ss", "iproute2"),
            ("top", "procps"),
        ];
        for (command, _) in mappings {
            if command == "pico" {
                continue;
            }
            let path = bin.join(command);
            fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
            let mut permissions = fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions).unwrap();
        }
        std::os::unix::fs::symlink(bin.join("nano"), bin.join("pico")).unwrap();
        let evidence_lines = mappings
            .iter()
            .map(|(command, package)| {
                format!("{command}\t{package}\t{}", bin.join(command).display())
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(evidence.join("command-providers.tsv"), evidence_lines).unwrap();
        let packages = mappings
            .iter()
            .map(|(_, package)| *package)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .map(|package| format!("{package}\t1.0\tarm64\tpool/{package}.deb\tfixture\t1"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(evidence.join("package-manifest.tsv"), packages).unwrap();
        let dpkg_query = temp.path().join("dpkg-query");
        fs::write(
            &dpkg_query,
            r#"#!/bin/sh
case "$1" in
 -W) printf '1.0\tarm64' ;;
 -S)
  if [ "${MERGED_USR_FIXTURE:-}" = 1 ]; then
   case "$2" in
    /usr/sbin/ifconfig) exit 1 ;;
    /sbin/ifconfig) printf 'net-tools: %s\n' "$2"; exit 0 ;;
   esac
  fi
  case "$(basename "$2")" in
   dig|nslookup) package=bind9-dnsutils ;;
   file) package=file ;;
   ip|ss) package=iproute2 ;;
   ping) package=iputils-ping ;;
   ifconfig|netstat) package=net-tools ;;
   ps|top) package=procps ;;
   pstree) package=psmisc ;;
   nano) package=nano ;;
   *) exit 1 ;;
  esac
  printf '%s: %s\n' "$package" "$2"
  ;;
 *) exit 2 ;;
esac
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&dpkg_query).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&dpkg_query, permissions).unwrap();
        Self {
            _temp: temp,
            evidence,
            bin,
            dpkg_query,
        }
    }

    fn verify(&self) -> std::process::Output {
        Command::new(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("verify-ubuntu-command-evidence.sh"),
        )
        .arg(&self.evidence)
        .env("DPKG_QUERY", &self.dpkg_query)
        .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
        .output()
        .unwrap()
    }

    fn write(&self) -> std::process::Output {
        Command::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("write-ubuntu-command-evidence.sh"))
            .arg(&self.evidence)
            .env("DPKG_QUERY", &self.dpkg_query)
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .output()
            .unwrap()
    }

    fn write_with_readlink(&self, readlink: &Path) -> std::process::Output {
        Command::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("write-ubuntu-command-evidence.sh"))
            .arg(&self.evidence)
            .env("DPKG_QUERY", &self.dpkg_query)
            .env("READLINK", readlink)
            .env("MERGED_USR_FIXTURE", "1")
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .output()
            .unwrap()
    }
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
        fs::create_dir(root.join("python")).unwrap();
        fs::write(root.join("python/apt_pkg.py"), "def init_system(): pass\n").unwrap();
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
        fs::write(
            root.join("command-providers.tsv"),
            "dig\tbind9-dnsutils\t/usr/bin/dig\nfile\tfile\t/usr/bin/file\nifconfig\tnet-tools\t/usr/sbin/ifconfig\nip\tiproute2\t/usr/sbin/ip\nnano\tnano\t/usr/bin/nano\nnetstat\tnet-tools\t/usr/bin/netstat\nnslookup\tbind9-dnsutils\t/usr/bin/nslookup\npico\tnano\t/usr/bin/pico\nping\tiputils-ping\t/usr/bin/ping\nps\tprocps\t/usr/bin/ps\npstree\tpsmisc\t/usr/bin/pstree\nss\tiproute2\t/usr/bin/ss\ntop\tprocps\t/usr/bin/top\n",
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
            format!("SNAPSHOT=2026-07-13T00:00:00Z\nBASE_IMAGE=ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab\nSIGNING_KEY_FINGERPRINT={FINGERPRINT}\nARCHITECTURE=arm64\nINSTALL_RECOMMENDS=false\nSYSTEM_PACKAGES_PATH=tests/image/system-tools.txt\nSYSTEM_PACKAGES_SHA256=a17fcdf2d9a54e9287711cca394a37b82d742aae570b5c51da9fa110ba925624\n"),
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
        let command_verifier = root.join("command-verifier");
        fs::write(&command_verifier, "#!/bin/sh\nexit 0\n").unwrap();
        let mut mode = fs::metadata(&command_verifier).unwrap().permissions();
        mode.set_mode(0o755);
        fs::set_permissions(command_verifier, mode).unwrap();
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
            .env(
                "COMMAND_EVIDENCE_VERIFIER",
                self.root().join("command-verifier"),
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

    fn assert_independent_rejected(&self, needle: &str) {
        let verifier =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("verify-ubuntu-debian-evidence.py");
        let output = Command::new("python3")
            .arg(verifier)
            .arg("--verify")
            .arg(self.root())
            .env("PYTHONPATH", self.root().join("python"))
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "independent verifier unexpectedly passed"
        );
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
fn advertised_file_command_is_an_explicit_locked_ubuntu_root() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let roots = fs::read_to_string(root.join("tests/image/system-tools.txt")).unwrap();
    let exact = roots.lines().filter(|package| *package == "file").count();
    assert_eq!(exact, 1, "the file provider must be one exact package root");
}

#[test]
fn runtime_command_evidence_binds_file_to_its_exact_package_provider() {
    let runtime = RuntimeCommandFixture::new();
    let output = runtime.write();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let evidence = fs::read_to_string(runtime.evidence.join("command-providers.tsv")).unwrap();
    assert!(
        evidence
            .lines()
            .any(|line| line == format!("file\tfile\t{}", runtime.bin.join("file").display())),
        "file command provider evidence is absent: {evidence}"
    );
    let verified = runtime.verify();
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
}

#[test]
fn structural_bundle_validator_binds_file_to_its_exact_package_provider() {
    let producer = fs::read_to_string(script()).unwrap();
    assert!(
        producer.contains(r#""file":"file""#),
        "the sealed bundle validator must require file's exact provider"
    );
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
        "ubuntu-package-cache.py\" stage",
        "ubuntu-package-cache.py\" publish",
    ] {
        assert!(
            producer.contains(required),
            "missing safe package cache contract: {required}"
        );
    }
    assert!(!producer.contains("cp -- \"$package_cache\"/*.deb"));
    assert!(!producer.contains("cp -- \"$work/apt/cache/archives\"/*.deb"));
    let signed = producer
        .find("destination=\"$work/evidence/signed-releases/$suite/InRelease\"")
        .unwrap();
    let stage = producer.find("ubuntu-package-cache.py\" stage").unwrap();
    let download = producer
        .find("--download-only --no-install-recommends install")
        .unwrap();
    let publish = producer.find("ubuntu-package-cache.py\" publish").unwrap();
    assert!(
        signed < stage && stage < download && download < publish,
        "cache must be staged only after signed metadata and published only after private download"
    );
}

#[test]
fn poisoned_shared_cache_entry_is_not_staged_or_modified() {
    let fixture = Fixture::new();
    let shared = fixture.root().join("cache-shared");
    let private = fixture.root().join("cache-private");
    fs::create_dir(&shared).unwrap();
    fs::create_dir(&private).unwrap();
    fs::write(shared.join("dep_1.0_arm64.deb"), b"poisoned").unwrap();

    let output = run_cache_helper(&fixture, "stage", &shared, &private, false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!private.join("dep_1.0_arm64.deb").exists());
    assert_eq!(
        fs::read(shared.join("dep_1.0_arm64.deb")).unwrap(),
        b"poisoned"
    );
}

#[test]
fn failed_private_download_validation_does_not_populate_shared_cache() {
    let fixture = Fixture::new();
    let shared = fixture.root().join("cache-shared");
    let private = fixture.root().join("cache-private");
    fs::create_dir(&shared).unwrap();
    fs::create_dir(&private).unwrap();
    fs::write(private.join("dep_1.0_arm64.deb"), b"invalid download").unwrap();

    let output = run_cache_helper(&fixture, "publish", &shared, &private, false);
    assert!(!output.status.success());
    assert!(fs::read_dir(&shared).unwrap().next().is_none());
}

#[test]
fn valid_shared_cache_entry_is_reused_only_through_private_staging() {
    let fixture = Fixture::new();
    let shared = fixture.root().join("cache-shared");
    let private = fixture.root().join("cache-private");
    fs::create_dir(&shared).unwrap();
    fs::create_dir(&private).unwrap();
    let payload = fs::read(fixture.root().join("repository/pool/dep.deb")).unwrap();
    fs::write(shared.join("dep_1.0_arm64.deb"), &payload).unwrap();

    let output = run_cache_helper(&fixture, "stage", &shared, &private, false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read(private.join("dep_1.0_arm64.deb")).unwrap(),
        payload
    );
}

#[test]
fn interrupted_atomic_cache_publish_leaves_no_destination_or_temporary_file() {
    let fixture = Fixture::new();
    let shared = fixture.root().join("cache-shared");
    let private = fixture.root().join("cache-private");
    fs::create_dir(&shared).unwrap();
    fs::create_dir(&private).unwrap();
    let payload = fs::read(fixture.root().join("repository/pool/dep.deb")).unwrap();
    fs::write(private.join("dep_1.0_arm64.deb"), payload).unwrap();

    let output = run_cache_helper(&fixture, "publish", &shared, &private, true);
    assert!(!output.status.success());
    assert!(
        fs::read_dir(&shared).unwrap().next().is_none(),
        "interrupted publication leaked a destination or temporary file"
    );
}

#[test]
fn conflicted_signed_identity_cannot_enter_cache_staging_or_publication() {
    for mode in ["stage", "publish"] {
        let fixture = Fixture::new();
        let packages = fs::read_to_string(fixture.root().join("repository/Packages")).unwrap();
        let conflicted = packages.replace(
            "Package: dep\n",
            "Package: dep\nX-Gascan-Conflict: selected\n",
        );
        add_signed_index(fixture.root(), "universe", &conflicted);
        let shared = fixture.root().join("cache-shared");
        let private = fixture.root().join("cache-private");
        fs::create_dir(&shared).unwrap();
        fs::create_dir(&private).unwrap();
        let payload = fs::read(fixture.root().join("repository/pool/dep.deb")).unwrap();
        let candidate = if mode == "stage" { &shared } else { &private };
        fs::write(candidate.join("dep_1.0_arm64.deb"), payload).unwrap();
        let output = run_cache_helper(&fixture, mode, &shared, &private, false);
        assert!(!output.status.success(), "{mode} accepted conflicted PVA");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("conflicting signed package metadata"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if mode == "stage" {
            assert!(fs::read_dir(&private).unwrap().next().is_none());
        } else {
            assert!(fs::read_dir(&shared).unwrap().next().is_none());
        }
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
    assert!(
        verifier
            .contains(r#"return candidate_arch in ("arm64", "all") or multi_arch == "foreign""#)
    );
    assert!(
        !verifier.contains(
            r#"return candidate_arch in (source_arch, "all") or multi_arch == "foreign""#
        )
    );
}

#[test]
fn producer_verification_is_explicitly_fail_closed_and_binds_virtual_roots() {
    let producer = fs::read_to_string(script()).unwrap();
    assert!(producer.contains("<<'PY' || return 1"));
    assert!(producer.contains("\"$debian_verifier\" --verify \"$evidence\" || return 1"));
    assert!(producer.contains("\"$command_verifier\" \"$evidence\" || return 1"));
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
    let command_root = producer
        .find("configure_command_rootfs \"$work/evidence\"")
        .unwrap();
    let verify = producer
        .find("verify_evidence_structure \"$work/evidence\"")
        .unwrap();
    let output = producer[verify..].find("mkdir -- \"$output\"").unwrap();
    assert!(
        command_root < verify,
        "runtime command proof must precede final structural verification"
    );
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
fn rejects_missing_command_provider_evidence() {
    let fixture = Fixture::new();
    let path = fixture.root().join("command-providers.tsv");
    let text = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with("dig\t"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(path, text).unwrap();
    fixture.assert_rejected("command provider");
}

#[test]
fn rejects_wrong_command_provider_or_path_evidence() {
    let fixture = Fixture::new();
    let path = fixture.root().join("command-providers.tsv");
    let text = fs::read_to_string(&path).unwrap().replace(
        "pico\tnano\t/usr/bin/pico",
        "pico\tprocps\t/usr/bin/not-pico",
    );
    fs::write(path, text).unwrap();
    fixture.assert_rejected("command provider");
}

#[test]
fn independent_runtime_command_verifier_rejects_missing_pico_alternative() {
    let runtime = RuntimeCommandFixture::new();
    fs::remove_file(runtime.bin.join("pico")).unwrap();
    let output = runtime.verify();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("pico"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn independent_runtime_command_verifier_accepts_exact_configured_closure() {
    let runtime = RuntimeCommandFixture::new();
    let output = runtime.verify();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn producer_command_writer_behaviorally_records_exact_configured_closure() {
    let runtime = RuntimeCommandFixture::new();
    let expected = fs::read(runtime.evidence.join("command-providers.tsv")).unwrap();
    fs::remove_file(runtime.evidence.join("command-providers.tsv")).unwrap();
    let output = runtime.write();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read(runtime.evidence.join("command-providers.tsv")).unwrap(),
        expected
    );
}

#[test]
fn producer_command_writer_accepts_validated_merged_usr_owner_alias() {
    let runtime = RuntimeCommandFixture::new();
    let readlink = runtime._temp.path().join("readlink");
    fs::write(
        &readlink,
        "#!/bin/sh\nfor path do :; done\ncase \"$path\" in */ifconfig) printf '%s\\n' /usr/sbin/ifconfig ;; /sbin|/usr/sbin) printf '%s\\n' /merged/sbin ;; *) /usr/bin/readlink \"$@\" ;; esac\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&readlink).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&readlink, permissions).unwrap();

    let output = runtime.write_with_readlink(&readlink);
    assert!(
        output.status.success(),
        "writer rejected merged-/usr ownership alias:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn producer_command_writer_rejects_unvalidated_usr_owner_alias() {
    let runtime = RuntimeCommandFixture::new();
    let readlink = runtime._temp.path().join("readlink");
    fs::write(
        &readlink,
        "#!/bin/sh\nfor path do :; done\ncase \"$path\" in */ifconfig) printf '%s\\n' /usr/sbin/ifconfig ;; *) /usr/bin/readlink \"$@\" ;; esac\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&readlink).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&readlink, permissions).unwrap();

    let output = runtime.write_with_readlink(&readlink);
    assert!(
        !output.status.success(),
        "writer accepted an ownership alias without merged-/usr validation"
    );
}

#[test]
fn independent_runtime_command_verifier_rejects_missing_or_wrong_evidence() {
    for mutation in ["missing", "wrong-path"] {
        let runtime = RuntimeCommandFixture::new();
        let path = runtime.evidence.join("command-providers.tsv");
        let text = fs::read_to_string(&path).unwrap();
        let mutated = if mutation == "missing" {
            text.lines()
                .filter(|line| !line.starts_with("dig\t"))
                .collect::<Vec<_>>()
                .join("\n")
                + "\n"
        } else {
            text.replace(
                &format!("pico\tnano\t{}", runtime.bin.join("pico").display()),
                "pico\tnano\t/usr/bin/not-pico",
            )
        };
        fs::write(path, mutated).unwrap();
        let output = runtime.verify();
        assert!(!output.status.success(), "{mutation} unexpectedly passed");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("evidence differs"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
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
    fixture.assert_rejected("same signed index");
}

#[test]
fn accepts_identical_package_metadata_republished_in_another_signed_index() {
    let fixture = Fixture::new();
    let root = fixture.root();
    let packages = fs::read_to_string(root.join("repository/Packages")).unwrap();
    add_signed_index(root, "universe", &packages);

    let output = fixture.verify();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn accepts_signed_package_stanza_with_empty_unknown_field() {
    let fixture = Fixture::new();
    let path = fixture.root().join("repository/Packages");
    let text = fs::read_to_string(&path).unwrap().replacen(
        "Package: dep\n",
        "Package: dep\nX-Cargo-Built-Using:\n",
        1,
    );
    rewrite_packages(&fixture, &path, text);
    let output = fixture.verify();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_same_index_duplicate_package_version_architecture_with_changed_payload_identity() {
    let fixture = Fixture::new();
    let path = fixture.root().join("repository/Packages");
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str(
        "Package: dep\nVersion: 1.0\nArchitecture: arm64\nFilename: pool/other.deb\nSHA256: 0000000000000000000000000000000000000000000000000000000000000000\nSize: 10\nMulti-Arch: same\n\n",
    );
    rewrite_packages(&fixture, &path, text);
    fixture.assert_rejected("same signed index");
    fixture.assert_independent_rejected("same signed index");
}

#[test]
fn rejects_cross_index_changed_depends() {
    assert_cross_index_mutation_rejected(
        "Depends: dep:any (>= 1.0) [arm64] | virtual-dep",
        "Depends: dep (= 1.0)",
    );
}

#[test]
fn rejects_cross_index_changed_provides() {
    assert_cross_index_mutation_rejected(
        "Provides: virtual-dep (= 3.0)",
        "Provides: virtual-dep (= 9.0)",
    );
}

#[test]
fn rejects_cross_index_changed_multi_arch() {
    assert_cross_index_mutation_rejected("Multi-Arch: same", "Multi-Arch: foreign");
}

#[test]
fn rejects_cross_index_changed_unknown_field() {
    assert_cross_index_mutation_rejected(
        "Package: dep\n",
        "Package: dep\nX-Gascan-Review: conflicting\n",
    );
}

#[test]
fn accepts_unselected_cross_index_component_migration() {
    let fixture = Fixture::new();
    let packages = fs::read_to_string(fixture.root().join("repository/Packages")).unwrap();
    let migrated = packages
        .replace(
            "Package: recommended\n",
            "Package: recommended\nSection: main/utils\n",
        )
        .replace(
            "Filename: pool/recommended.deb",
            "Filename: pool/main/recommended.deb",
        );
    assert_ne!(packages, migrated);
    add_signed_index(fixture.root(), "universe", &migrated);
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
fn workflow_recomputes_command_evidence_in_pinned_networkless_arm64_container() {
    let workflow = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../.github/workflows/workspace-bundles.yml"),
    )
    .unwrap();
    let start = workflow
        .rfind("\n  validate-ubuntu-packages-linux-arm64:")
        .unwrap();
    let validation = &workflow[start..];
    for required in [
        "ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab",
        "docker pull \"$image\"",
        "arm64/linux",
        "timeout --signal=KILL 300s docker run --rm --network none --platform linux/arm64",
        "source=$GITHUB_WORKSPACE,target=/src,readonly",
        "source=$RUNNER_TEMP/validated-evidence,target=/evidence,readonly",
        "Acquire::http::Proxy=false",
        "Acquire::https::Proxy=false",
        "Dir::Bin::Methods::http=/bin/false",
        "Dir::Bin::Methods::https=/bin/false",
        "--no-install-recommends install",
        "dpkg --audit",
        "/src/scripts/verify-ubuntu-command-evidence.sh /evidence",
    ] {
        assert!(
            validation.contains(required),
            "missing pinned offline command validation contract: {required}"
        );
    }
    for forbidden in [
        "--privileged",
        "--cap-add",
        "--device",
        "/var/run/docker.sock",
        "GITHUB_TOKEN",
        "GASCAMP_READ_TOKEN",
    ] {
        assert!(
            !validation.contains(forbidden),
            "forbidden validation container authority: {forbidden}"
        );
    }
}

#[test]
fn producer_command_proof_uses_pristine_pinned_rootfs_before_bootstrap() {
    let producer = fs::read_to_string(script()).unwrap();
    for required in [
        "UBUNTU_COMMAND_ROOTFS",
        "timeout --signal=KILL 300s chroot",
        "policy-rc.d",
        "write-ubuntu-command-evidence.sh",
        "verify-ubuntu-command-evidence.sh",
        "dpkg --audit",
    ] {
        assert!(
            producer.contains(required),
            "missing pristine command-root contract: {required}"
        );
    }
    assert!(
        !producer.contains("install_offline_closure \"$work/evidence\" \"$work/offline-apt\""),
        "producer must not configure the closure in its bootstrap-contaminated live root"
    );

    let workflow = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../.github/workflows/workspace-bundles.yml"),
    )
    .unwrap();
    let start = workflow.find("\n  ubuntu-packages-linux-arm64:").unwrap();
    let end = workflow[start + 1..]
        .find("\n  publish-ubuntu-packages-linux-arm64:")
        .map(|offset| start + 1 + offset)
        .unwrap();
    let producer_job = &workflow[start..end];
    let preserve = producer_job.find("pristine-root").unwrap();
    let bootstrap = producer_job
        .find("apt-get install --yes --no-install-recommends")
        .unwrap();
    let invocation = producer_job.find("UBUNTU_COMMAND_ROOTFS").unwrap();
    assert!(
        preserve < bootstrap && bootstrap < invocation,
        "pinned root must be preserved before producer tooling changes the live container"
    );
    assert!(producer_job.contains(
        "ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab"
    ));
    assert!(producer_job.contains("--platform linux/arm64"));
    let remove_devices = producer_job
        .find("rm -f \"/pristine-root/dev/$name\"")
        .expect("copied device nodes must be removed before replacement");
    let create_devices = producer_job.find("mknod -m 0666").unwrap();
    assert!(remove_devices < create_devices);
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

fn add_signed_index(root: &Path, component: &str, text: &str) {
    let directory = root
        .join("signed-indexes/fixture")
        .join(component)
        .join("binary-arm64");
    fs::create_dir_all(&directory).unwrap();
    let plain = directory.join("Packages");
    let compressed = directory.join("Packages.xz");
    fs::write(&plain, text).unwrap();
    let output = Command::new("xz")
        .args(["--check=crc32", "--stdout"])
        .arg(&plain)
        .output()
        .unwrap();
    assert!(output.status.success());
    fs::write(&compressed, output.stdout).unwrap();
    fs::remove_file(&plain).unwrap();
    let mut release = fs::read_to_string(root.join("signed-releases/fixture/InRelease")).unwrap();
    release.push_str(&format!(
        " {} {} {component}/binary-arm64/Packages\n {} {} {component}/binary-arm64/Packages.xz\n",
        sha_bytes(text.as_bytes()),
        text.len(),
        sha(&compressed),
        fs::metadata(&compressed).unwrap().len()
    ));
    fs::write(root.join("signed-releases/fixture/InRelease"), release).unwrap();
}

fn assert_cross_index_mutation_rejected(original: &str, replacement: &str) {
    let fixture = Fixture::new();
    let packages = fs::read_to_string(fixture.root().join("repository/Packages")).unwrap();
    let mutated = packages.replacen(original, replacement, 1);
    assert_ne!(packages, mutated, "mutation did not change signed metadata");
    add_signed_index(fixture.root(), "universe", &mutated);
    fixture.assert_rejected("conflicting signed package metadata");
    fixture.assert_independent_rejected("conflicting signed package metadata");
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

fn run_cache_helper(
    fixture: &Fixture,
    mode: &str,
    shared: &Path,
    private: &Path,
    interrupt: bool,
) -> std::process::Output {
    let dpkg_deb = fixture.root().join("fake-dpkg-deb");
    fs::write(
        &dpkg_deb,
        "#!/bin/sh\nfor argument do path=$argument; done\ncase \"$(basename \"$path\")\" in\n dep_1.0_arm64.deb) printf 'dep\\t1.0\\tarm64\\n' ;;\n root_2.0_arm64.deb) printf 'root\\t2.0\\tarm64\\n' ;;\n provider_3.0_arm64.deb) printf 'provider\\t3.0\\tarm64\\n' ;;\n recommended_1.0_arm64.deb) printf 'recommended\\t1.0\\tarm64\\n' ;;\n *) exit 19 ;;\nesac\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&dpkg_deb).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&dpkg_deb, permissions).unwrap();
    let mut command =
        Command::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("ubuntu-package-cache.py"));
    command
        .arg(mode)
        .arg(fixture.root())
        .arg(shared)
        .arg(private)
        .env("DPKG_DEB", dpkg_deb);
    if interrupt {
        command.env("UBUNTU_CACHE_INTERRUPT_AFTER_COPY", "1");
    }
    command.output().unwrap()
}
