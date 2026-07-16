use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use gascan_image_tools::{reviewed_input_kind_allowed, ReviewedInputKind};
use tempfile::TempDir;

const REVIEWED_GASCAMP_REVISION: &str = "f6b248c5926240856dbea83d1d2c5c90ea1c1456";

const REQUIRED: [&str; 10] = [
    "Dockerfile",
    ".artifacts/mise-linux-arm64",
    ".artifacts/playwright-chromium-reviewed",
    ".artifacts/expected-tool-versions.json",
    "images/workspace/bin",
    "images/workspace/libexec",
    "images/workspace/etc",
    "images/workspace/tests",
    "images/workspace/versions.lock",
    "tests/image/system-tools.txt",
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
        for line in dockerfile
            .lines()
            .filter(|line| line.starts_with("COPY ") && !line.contains("--from="))
        {
            let fields: Vec<_> = line.split_whitespace().collect();
            let source = fields[fields.len() - 2];
            if source.starts_with(".artifacts/") {
                continue;
            }
            let target = repository.join(source);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::copy(real_root.join(source), target).unwrap();
        }
        fs::write(cache.join("mise-linux-arm64"), "mise\n").unwrap();
        fs::write(cache.join("expected-tool-versions.json"), "{}\n").unwrap();
        fs::write(
            cache.join("playwright-chromium-reviewed/chrome-linux/chrome"),
            "chromium\n",
        )
        .unwrap();
        let lock = repository.join("images/workspace/versions.lock");
        fs::write(&lock, connected_lock("connected")).unwrap();
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
        Command::new(env!("CARGO_BIN_EXE_prepare-workspace-context"))
            .args(["--mode", "connected", "--replace"])
            .arg(&self.repository)
            .arg(&self.lock)
            .arg(&self.cache)
            .arg(&self.context)
            .output()
            .unwrap()
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
}

#[test]
fn every_local_dockerfile_copy_source_is_sealed_with_exact_bytes_and_mode() {
    let fixture = Fixture::new();
    assert!(fixture.run().status.success());
    let dockerfile =
        fs::read_to_string(fixture.repository.join("images/workspace/Dockerfile")).unwrap();
    for line in dockerfile
        .lines()
        .filter(|line| line.starts_with("COPY ") && !line.contains("--from="))
    {
        let fields: Vec<_> = line.split_whitespace().collect();
        let source = fields[fields.len() - 2];
        if source.starts_with(".artifacts/") {
            assert!(matches!(
                source,
                ".artifacts/mise-linux-arm64"
                    | ".artifacts/expected-tool-versions.json"
                    | ".artifacts/playwright-chromium-reviewed/chrome-linux"
            ));
            assert!(fixture.context.join(source).exists());
            continue;
        }
        let original = fixture.repository.join(source);
        let sealed = fixture.context.join(source);
        assert_eq!(
            fs::read(&sealed).unwrap(),
            fs::read(&original).unwrap(),
            "COPY source bytes differ: {source}"
        );
        let expected_mode = fields
            .iter()
            .find_map(|field| field.strip_prefix("--chmod="))
            .map(|mode| u32::from_str_radix(mode, 8).unwrap())
            .unwrap_or(0o444);
        assert_eq!(
            fs::metadata(&sealed).unwrap().permissions().mode() & 0o777,
            expected_mode,
            "COPY source mode differs: {source}"
        );
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

fn connected_lock(mode: &str) -> String {
    format!(
        "base_image = \"ubuntu@sha256:{}\"\nworkspace_build_mode = \"{mode}\"\n[mise]\nurl = \"https://example.invalid/mise\"\nsha256 = \"{}\"\n[playwright_chromium]\nurl = \"https://example.invalid/chromium\"\nsha256 = \"{}\"\n[gascamp]\nrevision = \"{REVIEWED_GASCAMP_REVISION}\"\n[workspace_bundles]\nmedia_type = \"application/vnd.gascan.workspace-bundle.v1+tar.zstd\"\nplatform = \"linux/arm64\"\npublication = \"pending\"\n",
        "a".repeat(64),
        "b".repeat(64),
        "c".repeat(64)
    )
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
    assert!(stdout
        .trim()
        .bytes()
        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
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
fn unsafe_allowlisted_inputs_fail_before_publication() {
    for kind in ["symlink", "token"] {
        let fixture = Fixture::new();
        match kind {
            "symlink" => {
                fs::remove_file(fixture.repository.join("images/workspace/bin/entrypoint"))
                    .unwrap();
                std::os::unix::fs::symlink(
                    &fixture.lock,
                    fixture.repository.join("images/workspace/bin/entrypoint"),
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
        fs::write(&fixture.lock, connected_lock(lock_mode)).unwrap();
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
        "fetch-image-artifact mise",
        "fetch-image-artifact chromium",
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
for arg in "$@"; do last=$arg; done
case "$*" in
  *'prepare-workspace-context -- --connected-lock'*)
    printf '%s\n%s\n%s\n%s\n%s\n' 'ubuntu@sha256:{digest}' 'https://example.invalid/mise' '{mise}' 'https://example.invalid/chromium' '{chromium}' ;;
  *'fetch-image-artifact'*) mkdir -p "$(dirname "$last")"; : >"$last" ;;
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
