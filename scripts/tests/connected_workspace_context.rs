use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Output},
    thread,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use gascan_image_tools::{ReviewedInputKind, parse_dockerfile_copies, reviewed_input_kind_allowed};
use sha2::{Digest, Sha256, Sha512};
use tempfile::TempDir;

const REVIEWED_GASCAMP_REVISION: &str = "f6b248c5926240856dbea83d1d2c5c90ea1c1456";

const REQUIRED: [&str; 17] = [
    "Dockerfile",
    ".artifacts/mise-linux-arm64",
    ".artifacts/playwright-chromium-reviewed",
    ".artifacts/expected-tool-versions.json",
    "images/workspace/bin",
    "images/workspace/libexec",
    "images/workspace/etc",
    "images/workspace/tests",
    "images/workspace/versions.lock",
    "workstation/claude-native.tgz",
    "workstation/glab.tar.gz",
    "workstation/herdr",
    "workstation/neovim.tar.gz",
    "workstation/npm-cache",
    "workstation/package-lock.json",
    "workstation/package.json",
    "tests/image/system-tools.txt",
];

const NATIVE_FIXTURES: [(&str, &[u8], &str, &str); 4] = [
    (
        "claude-native.tgz",
        b"claude native fixture\n",
        "npm_tgz",
        "registry.npmjs.org",
    ),
    ("glab.tar.gz", b"glab fixture\n", "tar_gz", "gitlab.com"),
    ("herdr", b"herdr fixture\n", "raw_binary", "github.com"),
    ("neovim.tar.gz", b"neovim fixture\n", "tar_gz", "github.com"),
];
const NPM_FIXTURES: [(&str, &[u8]); 4] = [
    ("claude", b"claude npm fixture\n"),
    ("codex", b"codex npm fixture\n"),
    ("pi", b"pi npm fixture\n"),
    ("claude-native", b"claude native fixture\n"),
];

struct Fixture {
    temporary: TempDir,
    repository: PathBuf,
    cache: PathBuf,
    lock: PathBuf,
    context: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir_in("/tmp").unwrap();
        let repository = temporary.path().join("repository");
        let cache = temporary.path().join("cache");
        fs::create_dir_all(repository.join("images/workspace/bin")).unwrap();
        fs::create_dir_all(repository.join("images/workspace/etc")).unwrap();
        fs::create_dir_all(repository.join("images/workspace/libexec")).unwrap();
        fs::create_dir_all(repository.join("images/workspace/tests")).unwrap();
        fs::create_dir_all(repository.join("tests/image")).unwrap();
        fs::create_dir_all(cache.join("playwright-chromium-reviewed/chrome-linux")).unwrap();
        let real_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let dockerfile = fs::read_to_string(real_root.join("images/workspace/Dockerfile")).unwrap();
        fs::write(repository.join("images/workspace/Dockerfile"), &dockerfile).unwrap();
        for copy in parse_dockerfile_copies(&dockerfile)
            .unwrap()
            .into_iter()
            .filter(|copy| !copy.from_stage)
        {
            for source in copy.sources {
                if source.starts_with(".artifacts/") {
                    continue;
                }
                let target = repository.join(&source);
                fs::create_dir_all(target.parent().unwrap()).unwrap();
                fs::copy(real_root.join(&source), target).unwrap();
            }
        }
        fs::write(cache.join("mise-linux-arm64"), "mise\n").unwrap();
        fs::write(cache.join("expected-tool-versions.json"), "{}\n").unwrap();
        fs::write(
            cache.join("playwright-chromium-reviewed/chrome-linux/chrome"),
            "chromium\n",
        )
        .unwrap();
        fs::create_dir_all(cache.join("workstation")).unwrap();
        for (name, bytes, _, _) in NATIVE_FIXTURES {
            fs::write(cache.join("workstation").join(name), bytes).unwrap();
        }
        let package = b"{\"name\":\"fixture\",\"private\":true,\"version\":\"0.0.0\"}\n";
        let mut package_records = "\"\":{\"name\":\"fixture\",\"version\":\"0.0.0\"}".to_owned();
        for (name, bytes) in NPM_FIXTURES {
            let integrity = format!("sha512-{}", BASE64.encode(Sha512::digest(bytes)));
            let npm_cache_path = npm_cache_path(&cache.join("workstation/npm-cache"), &integrity);
            fs::create_dir_all(npm_cache_path.parent().unwrap()).unwrap();
            fs::write(npm_cache_path, bytes).unwrap();
            let url = if name == "claude-native" {
                "https://registry.npmjs.org/fixture/claude-native.tgz".to_owned()
            } else {
                format!("https://registry.npmjs.org/{name}/-/{name}-1.0.0.tgz")
            };
            package_records.push_str(&format!(
                ",\"node_modules/{name}\":{{\"integrity\":\"{integrity}\",\"resolved\":\"{url}\",\"version\":\"1.0.0\"}}"
            ));
        }
        let package_lock = format!(
            "{{\"lockfileVersion\":3,\"name\":\"fixture\",\"packages\":{{{package_records}}}}}\n"
        );
        fs::write(
            repository.join("images/workspace/workstation-package.json"),
            package,
        )
        .unwrap();
        fs::write(
            repository.join("images/workspace/workstation-package-lock.json"),
            package_lock.as_bytes(),
        )
        .unwrap();
        let lock = repository.join("images/workspace/versions.lock");
        let lock_bytes = connected_lock("connected", package, package_lock.as_bytes());
        fs::write(&lock, &lock_bytes).unwrap();
        fs::write(
            cache.join("workstation/prefetch-lock.sha256"),
            format!("{:x}\n", Sha256::digest(lock_bytes.as_bytes())),
        )
        .unwrap();
        let context = temporary.path().join("connected-workspace-context");
        Self {
            temporary,
            repository,
            cache,
            lock,
            context,
        }
    }

    fn run(&self) -> Output {
        self.command().output().unwrap()
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_prepare-workspace-context"));
        command
            .args(["--mode", "connected", "--replace"])
            .arg(&self.repository)
            .arg(&self.lock)
            .arg(&self.cache)
            .arg(&self.context);
        command
    }

    fn verify(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_prepare-workspace-context"))
            .arg("--verify-connected")
            .arg(&self.repository)
            .arg(&self.lock)
            .arg(&self.cache)
            .arg(&self.context)
            .output()
            .unwrap()
    }

    fn npm_tarball(&self) -> PathBuf {
        paths(&self.cache.join("workstation/npm-cache"))
            .into_iter()
            .map(|path| self.cache.join("workstation/npm-cache").join(path))
            .find(|path| path.is_file())
            .unwrap()
    }

    fn refresh_lock_receipt(&self) {
        fs::write(
            self.cache.join("workstation/prefetch-lock.sha256"),
            format!("{:x}\n", Sha256::digest(fs::read(&self.lock).unwrap())),
        )
        .unwrap();
    }

    fn publish(&self, staging: &Path, destination: &Path) -> Output {
        Command::new(env!("CARGO_BIN_EXE_prepare-workspace-context"))
            .arg("--publish-workstation-cache")
            .arg(&self.lock)
            .arg(
                self.repository
                    .join("images/workspace/workstation-package.json"),
            )
            .arg(
                self.repository
                    .join("images/workspace/workstation-package-lock.json"),
            )
            .arg(staging)
            .arg(destination)
            .output()
            .unwrap()
    }
}

#[test]
fn every_local_dockerfile_copy_source_is_sealed_with_exact_bytes_and_mode() {
    let fixture = Fixture::new();
    assert!(fixture.run().status.success());
    let dockerfile =
        fs::read_to_string(fixture.repository.join("images/workspace/Dockerfile")).unwrap();
    for copy in parse_dockerfile_copies(&dockerfile)
        .unwrap()
        .into_iter()
        .filter(|copy| !copy.from_stage)
    {
        for source in copy.sources {
            if source.starts_with(".artifacts/") {
                assert!(matches!(
                    source.as_str(),
                    ".artifacts/mise-linux-arm64"
                        | ".artifacts/expected-tool-versions.json"
                        | ".artifacts/playwright-chromium-reviewed"
                ));
                assert!(fixture.context.join(source).exists());
                continue;
            }
            let original = fixture.repository.join(&source);
            let sealed = fixture.context.join(&source);
            assert_sealed_tree(&original, &sealed);
        }
    }
}

#[test]
fn docker_copy_parser_is_structural_and_fail_closed() {
    let parsed = parse_dockerfile_copies("  copy --chmod=0555 a b /dest\nCOPY --from=builder /out /dest\nCOPY name--from=value /dest\n").unwrap();
    assert_eq!(parsed[0].sources, ["a", "b"]);
    assert_eq!(parsed[0].chmod, Some(0o555));
    assert!(parsed[1].from_stage);
    assert_eq!(parsed[2].sources, ["name--from=value"]);
    for invalid in [
        "\tCOPY a b",
        "COPY [\"a\",\"b\"]",
        "COPY a \\",
        "COPY --unknown=x a b",
        "COPY 'a' b",
        "# escape=`\nCOPY safe /dest",
        "  # EsCaPe=\\\nCOPY safe /dest",
        "\t# escape=`\nCOPY safe /dest",
        "\x0c# escape=\\\nCOPY safe /dest",
    ] {
        assert!(parse_dockerfile_copies(invalid).is_err());
    }
}

fn assert_sealed_tree(original: &Path, sealed: &Path) {
    let metadata = fs::symlink_metadata(original).unwrap();
    assert!(!metadata.file_type().is_symlink());
    if metadata.is_dir() {
        let mut names: Vec<_> = fs::read_dir(original)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        let mut sealed_names: Vec<_> = fs::read_dir(sealed)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        sealed_names.sort();
        assert_eq!(names, sealed_names);
        for name in names {
            assert_sealed_tree(&original.join(&name), &sealed.join(name));
        }
    } else {
        assert!(metadata.is_file());
        assert_eq!(fs::read(original).unwrap(), fs::read(sealed).unwrap());
        let expected = if metadata.permissions().mode() & 0o111 != 0 {
            0o555
        } else {
            0o444
        };
        assert_eq!(
            fs::metadata(sealed).unwrap().permissions().mode() & 0o777,
            expected
        );
    }
}

fn append_directory_copy(fixture: &Fixture) -> PathBuf {
    let source = fixture
        .repository
        .join("images/workspace/tests/directory-source");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("plain"), "plain\n").unwrap();
    fs::write(source.join("nested/executable"), "#!/bin/sh\n").unwrap();
    fs::set_permissions(
        source.join("nested/executable"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let dockerfile = fixture.repository.join("images/workspace/Dockerfile");
    let mut text = fs::read_to_string(&dockerfile).unwrap();
    text.push_str("COPY images/workspace/tests/directory-source /opt/directory-source\n");
    fs::write(dockerfile, text).unwrap();
    source
}

#[test]
fn repository_directory_copy_is_recursively_sealed_and_unsafe_descendants_rejected() {
    let fixture = Fixture::new();
    let source = append_directory_copy(&fixture);
    assert!(fixture.run().status.success());
    assert_sealed_tree(
        &source,
        &fixture
            .context
            .join("images/workspace/tests/directory-source"),
    );

    for kind in ["symlink", "socket", "token"] {
        let fixture = Fixture::new();
        let source = append_directory_copy(&fixture);
        match kind {
            "symlink" => std::os::unix::fs::symlink("plain", source.join("nested/bad")).unwrap(),
            "socket" => {
                assert!(
                    Command::new("mkfifo")
                        .arg(source.join("nested/bad"))
                        .status()
                        .unwrap()
                        .success()
                );
                assert!(!fixture.run().status.success());
                assert!(!fixture.context.exists());
                continue;
            }
            "token" => fs::write(source.join("nested/github-token"), "secret").unwrap(),
            _ => unreachable!(),
        }
        assert!(!fixture.run().status.success(), "accepted {kind}");
        assert!(!fixture.context.exists());
    }
}

#[test]
fn escape_directive_cannot_hide_an_unsealed_multiline_copy() {
    for directive in [
        "# escape=`",
        "  # EsCaPe=\\",
        "\t# escape=`",
        "\t# EsCaPe=\\",
    ] {
        let fixture = Fixture::new();
        let path = fixture.repository.join("images/workspace/Dockerfile");
        let text = format!(
            "{directive}\nCOPY unsealed `\n /tmp/unsealed\n{}",
            fs::read_to_string(&path).unwrap()
        );
        fs::write(path, text).unwrap();
        assert!(!fixture.run().status.success());
        assert!(!fixture.context.exists());
    }
}

#[test]
fn unsealed_hypothetical_local_copy_is_rejected() {
    let fixture = Fixture::new();
    fs::write(fixture.repository.join("unsealed-local"), "not reviewed\n").unwrap();
    let path = fixture.repository.join("images/workspace/Dockerfile");
    let mut dockerfile = fs::read_to_string(&path).unwrap();
    dockerfile.push_str("COPY unsealed-local /tmp/unsealed-local\n");
    fs::write(path, dockerfile).unwrap();
    assert!(!fixture.run().status.success());
    assert!(!fixture.context.exists());
}

fn connected_lock(mode: &str, package: &[u8], package_lock: &[u8]) -> String {
    let mut records = String::new();
    for (name, bytes, kind, host) in NATIVE_FIXTURES {
        let record = if name == "claude-native.tgz" {
            format!(
                "\n[workstation_npm.claude_native]\npackage = \"@anthropic-ai/claude-code-linux-arm64\"\nversion = \"1.0.0\"\nurl = \"https://{host}/fixture/{name}\"\nintegrity = \"sha512-{}\"\nsha256 = \"{:x}\"\nsize = {}\nbinary_path = \"package/claude\"\nbinary_sha256 = \"{}\"\nbinary_size = 1\nplatform = \"linux-arm64\"\n",
                BASE64.encode(Sha512::digest(bytes)),
                Sha256::digest(bytes),
                bytes.len(),
                "d".repeat(64),
            )
        } else {
            let key = name.split('.').next().unwrap();
            format!(
                "\n[workstation_artifacts.{key}]\nversion = \"1.0.0\"\nurl = \"https://{host}/fixture/{name}\"\nsha256 = \"{:x}\"\nsize = {}\nplatform = \"linux-arm64\"\nkind = \"{kind}\"\n",
                Sha256::digest(bytes),
                bytes.len(),
            )
        };
        records.push_str(&record);
    }
    for name in ["claude", "codex", "pi"] {
        let bytes = NPM_FIXTURES
            .iter()
            .find_map(|(candidate, bytes)| (*candidate == name).then_some(*bytes))
            .unwrap();
        records.push_str(&format!(
            "\n[workstation_artifacts.{name}]\nversion = \"1.0.0\"\nurl = \"https://registry.npmjs.org/{name}/-/{name}-1.0.0.tgz\"\nsha256 = \"{:x}\"\nsize = {}\nplatform = \"linux-arm64\"\nkind = \"npm_tgz\"\n",
            Sha256::digest(bytes),
            bytes.len(),
        ));
    }
    format!(
        "base_image = \"ubuntu@sha256:{}\"\nworkspace_build_mode = \"{mode}\"\n[mise]\nurl = \"https://example.invalid/mise\"\nsha256 = \"{}\"\n[playwright_chromium]\nurl = \"https://example.invalid/chromium\"\nsha256 = \"{}\"\n[gascamp]\nrevision = \"{REVIEWED_GASCAMP_REVISION}\"\n[workspace_bundles]\nmedia_type = \"application/vnd.gascan.workspace-bundle.v1+tar.zstd\"\nplatform = \"linux/arm64\"\npublication = \"pending\"\n{records}\n[workstation_npm]\nscripts = \"disabled\"\nnpm_version = \"11.12.1\"\npackage_manifest_sha256 = \"{:x}\"\npackage_lock_sha256 = \"{:x}\"\n",
        "a".repeat(64),
        "b".repeat(64),
        "c".repeat(64),
        Sha256::digest(package),
        Sha256::digest(package_lock),
    )
}

fn npm_cache_path(root: &Path, integrity: &str) -> PathBuf {
    let digest = BASE64
        .decode(integrity.strip_prefix("sha512-").unwrap())
        .unwrap();
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    root.join("_cacache/content-v2/sha512")
        .join(&hex[..2])
        .join(&hex[2..4])
        .join(&hex[4..])
}

fn paths(root: &Path) -> Vec<String> {
    fn visit(root: &Path, directory: &Path, found: &mut Vec<String>) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            found.push(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            );
            if path.is_dir() {
                visit(root, &path, found);
            }
        }
    }
    let mut found = Vec::new();
    visit(root, root, &mut found);
    found.sort();
    found
}

#[test]
fn connected_context_is_the_exact_public_allowlist_and_prints_digest() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.repository.join("bundles/private")).unwrap();
    fs::write(
        fixture.repository.join("bundles/private/archive"),
        "private",
    )
    .unwrap();
    fs::create_dir(fixture.repository.join(".git")).unwrap();
    fs::write(fixture.repository.join(".git/config"), "secret").unwrap();
    fs::write(fixture.repository.join("GASCAMP_READ_TOKEN_FILE"), "secret").unwrap();
    fs::write(fixture.repository.join("outside-allowlist"), "nope").unwrap();

    let output = fixture.run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.trim().len(), 64);
    assert!(
        stdout
            .trim()
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    );
    let actual = paths(&fixture.context);
    for required in REQUIRED {
        assert!(
            actual
                .iter()
                .any(|path| path == required || path.starts_with(&format!("{required}/"))),
            "missing {required}"
        );
    }
    for forbidden in [
        "bundles",
        ".git",
        "GASCAMP_READ_TOKEN_FILE",
        "outside-allowlist",
    ] {
        assert!(
            !actual
                .iter()
                .any(|path| path == forbidden || path.starts_with(&format!("{forbidden}/"))),
            "published {forbidden}"
        );
    }
    assert!(actual.iter().any(|path| path == "context-manifest.tsv"));
    let _keep_alive = &fixture.temporary;
}

#[test]
fn connected_context_can_be_reverified_with_its_pending_lock() {
    let fixture = Fixture::new();
    let created = fixture.run();
    assert!(created.status.success());
    let verified = fixture.verify();
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    assert_eq!(created.stdout, verified.stdout);
}

#[test]
fn workstation_cache_rejects_missing_extra_linked_or_corrupt_content() {
    for mutation in [
        "missing-native",
        "extra-native",
        "symlink-native",
        "hardlink-native",
        "corrupt-native",
        "missing-npm",
        "extra-npm",
        "symlink-npm",
        "hardlink-npm",
        "corrupt-npm",
    ] {
        let fixture = Fixture::new();
        let native = fixture.cache.join("workstation/herdr");
        let npm = fixture.npm_tarball();
        match mutation {
            "missing-native" => fs::remove_file(native).unwrap(),
            "extra-native" => fs::write(fixture.cache.join("workstation/extra"), b"extra").unwrap(),
            "symlink-native" => {
                fs::remove_file(&native).unwrap();
                std::os::unix::fs::symlink(&fixture.lock, native).unwrap();
            }
            "hardlink-native" => {
                let peer = fixture.cache.join("hardlink-peer");
                fs::hard_link(&native, peer).unwrap();
                assert_eq!(fs::metadata(native).unwrap().nlink(), 2);
            }
            "corrupt-native" => fs::write(native, b"wrong").unwrap(),
            "missing-npm" => fs::remove_file(npm).unwrap(),
            "extra-npm" => fs::write(
                fixture.cache.join("workstation/npm-cache/unlocked"),
                b"extra",
            )
            .unwrap(),
            "symlink-npm" => {
                fs::remove_file(&npm).unwrap();
                std::os::unix::fs::symlink(&fixture.lock, npm).unwrap();
            }
            "hardlink-npm" => {
                let peer = fixture.cache.join("npm-hardlink-peer");
                fs::hard_link(&npm, peer).unwrap();
                assert_eq!(fs::metadata(npm).unwrap().nlink(), 2);
            }
            "corrupt-npm" => fs::write(npm, b"wrong").unwrap(),
            _ => unreachable!(),
        }
        let output = fixture.run();
        assert!(!output.status.success(), "accepted {mutation}");
        assert!(!fixture.context.exists(), "published after {mutation}");
    }
}

#[test]
fn workstation_lock_change_after_prefetch_and_traversal_are_rejected() {
    let changed_after_prefetch = Fixture::new();
    fs::write(
        &changed_after_prefetch.lock,
        format!(
            "{}# changed after prefetch\n",
            fs::read_to_string(&changed_after_prefetch.lock).unwrap()
        ),
    )
    .unwrap();
    assert!(!changed_after_prefetch.run().status.success());

    let traversal = Fixture::new();
    let package_lock_path = traversal
        .repository
        .join("images/workspace/workstation-package-lock.json");
    let malformed = fs::read_to_string(&package_lock_path)
        .unwrap()
        .replace("node_modules/claude", "../fixture");
    fs::write(&package_lock_path, &malformed).unwrap();
    let lock = fs::read_to_string(&traversal.lock).unwrap();
    let old_hash = lock
        .lines()
        .find_map(|line| line.strip_prefix("package_lock_sha256 = \""))
        .unwrap()
        .trim_end_matches('"');
    fs::write(
        &traversal.lock,
        lock.replacen(
            old_hash,
            &format!("{:x}", Sha256::digest(malformed.as_bytes())),
            1,
        ),
    )
    .unwrap();
    traversal.refresh_lock_receipt();
    assert!(!traversal.run().status.success());
    assert!(!traversal.context.exists());
}

#[test]
fn repository_npm_bytes_changed_after_validation_cannot_be_published() {
    let fixture = Fixture::new();
    let validated = fs::read(
        fixture
            .repository
            .join("images/workspace/workstation-package.json"),
    )
    .unwrap();
    let mut child = fixture.command().spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let staging_exists = fs::read_dir(fixture.context.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".connected-workspace-context-")
            });
        if staging_exists {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "context staging was never created"
        );
        assert!(
            child.try_wait().unwrap().is_none(),
            "assembler exited before race"
        );
        thread::sleep(Duration::from_millis(1));
    }
    fs::write(
        fixture
            .repository
            .join("images/workspace/workstation-package.json"),
        b"{\"name\":\"mutated-after-validation\"}\n",
    )
    .unwrap();
    let status = child.wait().unwrap();
    assert!(
        status.success(),
        "assembler did not preserve validated bytes"
    );
    assert_eq!(
        fs::read(fixture.context.join("workstation/package.json")).unwrap(),
        validated
    );
}

#[test]
fn workstation_cache_publication_preserves_old_on_failure_and_exchanges_valid_tree() {
    let invalid = Fixture::new();
    let staging = invalid.temporary.path().join("invalid-staging");
    fs::create_dir(&staging).unwrap();
    fs::rename(
        invalid.cache.join("workstation"),
        staging.join("workstation"),
    )
    .unwrap();
    fs::remove_file(staging.join("workstation/herdr")).unwrap();
    let destination = invalid.temporary.path().join("published");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("old-marker"), b"old").unwrap();
    assert!(!invalid.publish(&staging, &destination).status.success());
    assert_eq!(fs::read(destination.join("old-marker")).unwrap(), b"old");

    let valid = Fixture::new();
    let staging = valid.temporary.path().join("valid-staging");
    fs::create_dir(&staging).unwrap();
    fs::rename(valid.cache.join("workstation"), staging.join("workstation")).unwrap();
    let destination = valid.temporary.path().join("published");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("old-marker"), b"old").unwrap();
    let output = valid.publish(&staging, &destination);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!destination.join("old-marker").exists());
    assert_eq!(
        fs::read(destination.join("herdr")).unwrap(),
        b"herdr fixture\n"
    );
}

#[test]
fn workstation_cache_exchange_uses_the_instrumented_dual_parent_durability_sequence() {
    let source = include_str!("../src/bin/prepare-workspace-context.rs");
    for required in [
        "PublicationAction::Exchange",
        "PublicationAction::SyncSourceParent",
        "PublicationAction::SyncDestinationParent",
        "PublicationAction::RemoveOld",
        "run_publication_actions",
    ] {
        assert!(
            source.contains(required),
            "missing durability action {required}"
        );
    }
}

#[test]
fn unsafe_allowlisted_inputs_fail_before_publication() {
    for kind in ["symlink", "token"] {
        let fixture = Fixture::new();
        match kind {
            "symlink" => {
                fs::remove_file(
                    fixture
                        .repository
                        .join("images/workspace/bin/gascan-entrypoint"),
                )
                .unwrap();
                std::os::unix::fs::symlink(
                    &fixture.lock,
                    fixture
                        .repository
                        .join("images/workspace/bin/gascan-entrypoint"),
                )
                .unwrap();
            }
            "token" => fs::write(
                fixture.repository.join("images/workspace/etc/github-token"),
                "secret",
            )
            .unwrap(),
            _ => unreachable!(),
        }
        let output = fixture.run();
        assert!(!output.status.success(), "accepted {kind}");
        assert!(!fixture.context.exists());
    }
}

#[test]
fn connected_boundary_rejects_socket_like_and_other_special_file_kinds() {
    assert!(reviewed_input_kind_allowed(ReviewedInputKind::Directory));
    assert!(reviewed_input_kind_allowed(ReviewedInputKind::RegularFile));
    assert!(!reviewed_input_kind_allowed(ReviewedInputKind::Other));
}

#[test]
fn connected_mode_and_lock_must_match_exactly() {
    for lock_mode in ["offline", "CONNECTED"] {
        let fixture = Fixture::new();
        let changed = fs::read_to_string(&fixture.lock).unwrap().replace(
            "workspace_build_mode = \"connected\"",
            &format!("workspace_build_mode = \"{lock_mode}\""),
        );
        fs::write(&fixture.lock, changed).unwrap();
        assert!(!fixture.run().status.success());
        assert!(!fixture.context.exists());
    }
}

#[test]
fn connected_gascamp_revision_must_match_the_reviewed_revision() {
    let exact = Fixture::new();
    assert!(exact.run().status.success());

    let changed = Fixture::new();
    let lock = fs::read_to_string(&changed.lock).unwrap().replace(
        REVIEWED_GASCAMP_REVISION,
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    );
    fs::write(&changed.lock, lock).unwrap();
    let output = changed.run();
    assert!(!output.status.success());
    assert!(!changed.context.exists());
}

#[test]
fn published_tree_is_read_only() {
    let fixture = Fixture::new();
    assert!(fixture.run().status.success());
    for path in std::iter::once(fixture.context.clone()).chain(
        paths(&fixture.context)
            .into_iter()
            .map(|path| fixture.context.join(path)),
    ) {
        assert_eq!(
            fs::symlink_metadata(path).unwrap().permissions().mode() & 0o222,
            0
        );
    }
}

#[test]
fn connected_prefetch_uses_the_reviewed_public_acquisition_boundary() {
    let script = include_str!("../prefetch-connected-workspace-image.sh");
    for required in [
        "prepare-workspace-context --connected-lock",
        "prepare-workspace-context --workstation-lock",
        "prepare-workspace-context --publish-workstation-cache",
        "fetch-image-artifact mise",
        "fetch-image-artifact chromium",
        "workstation-github",
        "workstation-gitlab",
        "workstation-npm",
        "prefetch-lock.sha256",
        "extract-reviewed-chromium",
        "validate-tool-versions",
        "container image pull",
        "validate-image-inspect",
        "prepare-workspace-context --mode connected --replace",
    ] {
        assert!(script.contains(required), "missing safeguard {required}");
    }
    assert!(!script.contains("GASCAMP_READ_TOKEN_FILE"));
    assert!(!script.contains("curl "));
    assert!(!script.contains("npm "));
    assert!(!script.contains("node "));
    assert!(
        !script.contains("rm -rf \"$workstation\""),
        "failed refresh must preserve the previously published cache"
    );
}

#[test]
fn connected_prefetch_pulls_exact_linux_arm64_digest_then_inspects_it() {
    let fixture = tempfile::tempdir_in("/tmp").unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::create_dir_all(root.join("images/workspace/etc/mise")).unwrap();
    fs::create_dir_all(root.join(".artifacts")).unwrap();
    fs::write(
        root.join("scripts/prefetch-connected-workspace-image.sh"),
        include_str!("../prefetch-connected-workspace-image.sh"),
    )
    .unwrap();
    fs::write(root.join("images/workspace/versions.lock"), "fixture\n").unwrap();
    fs::write(
        root.join("images/workspace/etc/mise/config.toml"),
        "fixture\n",
    )
    .unwrap();
    let bin = root.join("bin");
    fs::create_dir(&bin).unwrap();
    fs::write(
        bin.join("cargo"),
        format!(r#"#!/usr/bin/env bash
set -eu
last=''
previous=''
for arg in "$@"; do previous=$last; last=$arg; done
case "$*" in
  *'prepare-workspace-context -- --connected-lock'*)
    printf '%s\n%s\n%s\n%s\n%s\n' 'ubuntu@sha256:{digest}' 'https://example.invalid/mise' '{mise}' 'https://example.invalid/chromium' '{chromium}' ;;
  *'prepare-workspace-context -- --workstation-lock'*)
    printf '%s\n%s\n%s\n' \
      'receipt	prefetch-lock.sha256	{receipt}' \
      'native	herdr	workstation-github	https://github.com/example/herdr	{native}	1' \
      'npm	_cacache/content-v2/sha512/aa/bb/cc	workstation-npm	https://registry.npmjs.org/example/-/example.tgz	sha512-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==	209715200' ;;
  *'prepare-workspace-context -- --publish-workstation-cache'*)
    mv "$previous/workstation" "$last" ;;
  *'fetch-image-artifact'*)
    destination=$last
    case "$last" in *[!0-9]*) ;; *) destination=$previous ;; esac
    mkdir -p "$(dirname "$destination")"; : >"$destination" ;;
  *'extract-reviewed-chromium'*) mkdir -p "$last/chrome-linux"; : >"$last/chrome-linux/chrome" ;;
  *'validate-tool-versions'*) printf '{{}}\n' ;;
  *'validate-image-inspect'*) cat >/dev/null; printf 'sha256:{digest}\n' ;;
  *'prepare-workspace-context -- --mode connected --replace'*) printf '{manifest}\n' ;;
  *) exit 91 ;;
esac
"#,
            digest = "a".repeat(64),
            mise = "b".repeat(64),
            chromium = "c".repeat(64),
            manifest = "d".repeat(64),
            receipt = "e".repeat(64),
            native = "f".repeat(64),
        ),
    ).unwrap();
    fs::write(
        bin.join("container"),
        r#"#!/usr/bin/env bash
set -eu
printf '%s\n' "$*" >>"$CONTAINER_CALLS"
case "$*" in
  'image pull --platform linux/arm64 ubuntu@sha256:'*) exit 0 ;;
  'image inspect ubuntu@sha256:'*) printf '[{}]\n' ;;
  *) exit 92 ;;
esac
"#,
    )
    .unwrap();
    for executable in [bin.join("cargo"), bin.join("container")] {
        fs::set_permissions(executable, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let calls = root.join("container-calls");
    let output = Command::new("bash")
        .arg(root.join("scripts/prefetch-connected-workspace-image.sh"))
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("CONTAINER_CALLS", &calls)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{}\n", "d".repeat(64))
    );
    let calls = fs::read_to_string(calls).unwrap();
    assert!(calls.contains(&format!(
        "image pull --platform linux/arm64 ubuntu@sha256:{}\n",
        "a".repeat(64)
    )));
    assert!(calls.contains(&format!("image inspect ubuntu@sha256:{}\n", "a".repeat(64))));
}
