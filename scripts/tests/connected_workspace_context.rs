use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use tempfile::TempDir;

const REQUIRED: [&str; 9] = [
    "Dockerfile",
    ".artifacts/mise-linux-arm64",
    ".artifacts/playwright-chromium-reviewed",
    ".artifacts/expected-tool-versions.json",
    "images/workspace/bin",
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
        fs::create_dir_all(repository.join("images/workspace/tests")).unwrap();
        fs::create_dir_all(repository.join("tests/image")).unwrap();
        fs::create_dir_all(cache.join("playwright-chromium-reviewed/chrome-linux")).unwrap();
        for path in [
            "images/workspace/bin/entrypoint",
            "images/workspace/etc/config",
            "images/workspace/tests/smoke",
            "tests/image/system-tools.txt",
        ] {
            fs::write(repository.join(path), format!("{path}\n")).unwrap();
        }
        fs::write(
            repository.join("images/workspace/Dockerfile"),
            "FROM locked\n",
        )
        .unwrap();
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
}

fn connected_lock(mode: &str) -> String {
    format!(
        "base_image = \"ubuntu@sha256:{}\"\nworkspace_build_mode = \"{mode}\"\n[mise]\nurl = \"https://example.invalid/mise\"\nsha256 = \"{}\"\n[playwright_chromium]\nurl = \"https://example.invalid/chromium\"\nsha256 = \"{}\"\n[workspace_bundles]\nmedia_type = \"application/vnd.gascan.workspace-bundle.v1+tar.zstd\"\nplatform = \"linux/arm64\"\npublication = \"pending\"\n",
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
fn connected_boundary_explicitly_rejects_sockets_and_other_special_files() {
    let source = include_str!("../src/bin/prepare-workspace-context.rs");
    assert!(source.contains("symlink or special file"));
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
