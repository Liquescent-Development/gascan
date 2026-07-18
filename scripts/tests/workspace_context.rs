use std::{
    fs,
    os::unix::fs::PermissionsExt,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const RECORDS: [&str; 3] = ["ubuntu_packages", "mise_runtimes", "gascamp_source_vendor"];

struct Fixture {
    temporary: TempDir,
    lock: PathBuf,
    cache: PathBuf,
    context: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let cache = temporary.path().join("cache");
        fs::create_dir_all(cache.join("bundles")).unwrap();
        fs::create_dir_all(cache.join("playwright-chromium-reviewed/chrome-linux")).unwrap();
        fs::write(cache.join("mise-linux-arm64"), b"mise fixture\n").unwrap();
        fs::write(cache.join("expected-tool-versions.json"), b"{}\n").unwrap();
        fs::write(
            cache.join("playwright-chromium-reviewed/chrome-linux/chrome"),
            b"chromium fixture\n",
        )
        .unwrap();

        let mut records = String::new();
        for name in RECORDS {
            let archive = bundle(name.as_bytes());
            let path = cache.join(format!("bundles/{name}.tar.zst"));
            fs::write(path, &archive).unwrap();
            records.push_str(&format!(
                "\n[workspace_bundles.{name}]\nurl = \"https://github.com/Liquescent-Development/gascan/releases/download/fixture/{name}.tar.zst\"\nsha256 = \"{:x}\"\nsize = {}\nmedia_type = \"application/vnd.gascan.workspace-bundle.v1+tar.zstd\"\nplatform = \"linux/arm64\"\n",
                Sha256::digest(&archive), archive.len()
            ));
        }
        let lock = temporary.path().join("versions.lock");
        fs::write(
            &lock,
            format!(
                "[workspace_bundles]\nmedia_type = \"application/vnd.gascan.workspace-bundle.v1+tar.zstd\"\nplatform = \"linux/arm64\"\npublication = \"published\"\n{records}"
            ),
        )
        .unwrap();
        let context = temporary.path().join("workspace-context");
        Self {
            temporary,
            lock,
            cache,
            context,
        }
    }

    fn run(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_prepare-workspace-context"))
            .arg(repository_root())
            .arg(&self.lock)
            .arg(&self.cache)
            .arg(&self.context)
            .output()
            .unwrap()
    }

    fn verify(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_prepare-workspace-context"))
            .arg("--verify")
            .arg(repository_root())
            .arg(&self.lock)
            .arg(&self.cache)
            .arg(&self.context)
            .output()
            .unwrap()
    }

    fn replace(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_prepare-workspace-context"))
            .arg("--replace")
            .arg(repository_root())
            .arg(&self.lock)
            .arg(&self.cache)
            .arg(&self.context)
            .output()
            .unwrap()
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned()
}

fn bundle(payload: &[u8]) -> Vec<u8> {
    let manifest = serde_json::to_vec(&json!({
        "version": 1,
        "platform": "linux/arm64",
        "files": [{"path":"payload.txt","kind":"file","size":payload.len(),"sha256":format!("{:x}", Sha256::digest(payload))}]
    }))
    .unwrap();
    let mut tar_bytes = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_bytes);
        append(&mut builder, "bundle-manifest.json", &manifest);
        append(&mut builder, "payload.txt", payload);
        builder.finish().unwrap();
    }
    zstd::stream::encode_all(tar_bytes.as_slice(), 1).unwrap()
}

fn append(builder: &mut tar::Builder<&mut Vec<u8>>, path: &str, body: &[u8]) {
    let mut header = tar::Header::new_gnu();
    header.set_path(path).unwrap();
    header.set_size(body.len() as u64);
    header.set_mode(0o444);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_cksum();
    builder.append(&header, body).unwrap();
}

fn paths(root: &Path) -> Vec<String> {
    fn visit(base: &Path, directory: &Path, paths: &mut Vec<String>) {
        let mut entries: Vec<_> = fs::read_dir(directory)
            .unwrap()
            .map(Result::unwrap)
            .collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let relative = path
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            paths.push(relative);
            if path.is_dir() {
                visit(base, &path, paths);
            }
        }
    }
    let mut result = Vec::new();
    visit(root, root, &mut result);
    result
}

fn remove_read_only_tree(root: &Path) {
    for path in paths(root).into_iter().rev() {
        let path = root.join(path);
        if path.is_dir() {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    fs::set_permissions(root, fs::Permissions::from_mode(0o755)).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pending_bundle_lock_fails_before_context_publication() {
    let fixture = Fixture::new();
    fs::write(
        &fixture.lock,
        "[workspace_bundles]\nmedia_type = \"application/vnd.gascan.workspace-bundle.v1+tar.zstd\"\nplatform = \"linux/arm64\"\npublication = \"pending\"\n",
    )
    .unwrap();
    let output = fixture.run();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("publication state is not published"));
    assert!(!fixture.context.exists());
}

#[test]
fn missing_or_corrupt_cached_bundle_never_publishes_context() {
    for corrupt in [false, true] {
        let fixture = Fixture::new();
        let archive = fixture.cache.join("bundles/mise_runtimes.tar.zst");
        if corrupt {
            fs::write(archive, b"corrupt").unwrap();
        } else {
            fs::remove_file(archive).unwrap();
        }
        assert!(!fixture.run().status.success());
        assert!(!fixture.context.exists());
    }
}

#[test]
fn wrong_locked_size_hash_or_platform_is_rejected() {
    for replacement in [
        "size = 1",
        &format!("sha256 = \"{}\"", "0".repeat(64)),
        "platform = \"linux/amd64\"",
    ] {
        let fixture = Fixture::new();
        let text = fs::read_to_string(&fixture.lock).unwrap();
        let changed = if replacement.starts_with("size") {
            let start = text.find("size = ").unwrap();
            let end = text[start..].find('\n').unwrap() + start;
            format!("{}{}{}", &text[..start], replacement, &text[end..])
        } else if replacement.starts_with("sha256") {
            let start = text.find("sha256 = ").unwrap();
            let end = text[start..].find('\n').unwrap() + start;
            format!("{}{}{}", &text[..start], replacement, &text[end..])
        } else {
            text.replacen("platform = \"linux/arm64\"", replacement, 1)
        };
        fs::write(&fixture.lock, changed).unwrap();
        assert!(!fixture.run().status.success());
        assert!(!fixture.context.exists());
    }
}

#[test]
fn context_is_exact_allowlisted_read_only_and_deterministic() {
    let fixture = Fixture::new();
    fs::write(
        repository_root().join("task5-unlisted-user-file"),
        b"never copy me",
    )
    .unwrap();
    let output = fixture.run();
    fs::remove_file(repository_root().join("task5-unlisted-user-file")).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let first = fs::read(fixture.context.join("context-manifest.tsv")).unwrap();
    assert!(
        !paths(&fixture.context)
            .iter()
            .any(|path| path.contains("task5-unlisted"))
    );
    assert!(
        paths(&fixture.context)
            .iter()
            .any(|path| path == "bundles/ubuntu_packages/payload.txt")
    );
    assert!(
        fs::metadata(fixture.context.join("Dockerfile"))
            .unwrap()
            .permissions()
            .readonly()
    );
    remove_read_only_tree(&fixture.context);
    assert!(fixture.run().status.success());
    assert_eq!(
        first,
        fs::read(fixture.context.join("context-manifest.tsv")).unwrap()
    );
    let manifest = String::from_utf8(first).unwrap();
    let lines: Vec<_> = manifest.lines().collect();
    let sorted = {
        let mut value = lines.clone();
        value.sort_unstable();
        value
    };
    assert_eq!(lines, sorted);
    assert!(fixture.verify().status.success());
    fs::set_permissions(&fixture.context, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!fixture.verify().status.success());
    fs::set_permissions(&fixture.context, fs::Permissions::from_mode(0o555)).unwrap();
    fs::set_permissions(
        fixture.context.join("images"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    fs::create_dir(fixture.context.join("images/unlisted-empty-directory")).unwrap();
    fs::set_permissions(
        fixture.context.join("images/unlisted-empty-directory"),
        fs::Permissions::from_mode(0o555),
    )
    .unwrap();
    fs::set_permissions(
        fixture.context.join("images"),
        fs::Permissions::from_mode(0o555),
    )
    .unwrap();
    assert!(!fixture.verify().status.success());
    fs::set_permissions(
        fixture.context.join("images"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    fs::remove_dir(fixture.context.join("images/unlisted-empty-directory")).unwrap();
    fs::set_permissions(
        fixture.context.join("images"),
        fs::Permissions::from_mode(0o555),
    )
    .unwrap();
    fs::set_permissions(
        fixture.context.join("Dockerfile"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    fs::write(fixture.context.join("Dockerfile"), b"stale context\n").unwrap();
    fs::set_permissions(
        fixture.context.join("Dockerfile"),
        fs::Permissions::from_mode(0o444),
    )
    .unwrap();
    assert!(fixture.replace().status.success());
    assert!(fixture.verify().status.success());

    let before_failed_refresh = fs::read(fixture.context.join("context-manifest.tsv")).unwrap();
    let archive_path = fixture.cache.join("bundles/mise_runtimes.tar.zst");
    let archive = fs::read(&archive_path).unwrap();
    fs::remove_file(&archive_path).unwrap();
    assert!(!fixture.replace().status.success());
    assert_eq!(
        before_failed_refresh,
        fs::read(fixture.context.join("context-manifest.tsv")).unwrap()
    );
    fs::write(archive_path, archive).unwrap();
    fs::write(fixture.cache.join("mise-linux-arm64"), b"stale cache\n").unwrap();
    assert!(!fixture.verify().status.success());
    remove_read_only_tree(&fixture.context);
}

#[test]
fn symlink_in_reviewed_input_is_rejected() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.cache.join("expected-tool-versions.json")).unwrap();
    symlink(
        &fixture.lock,
        fixture.cache.join("expected-tool-versions.json"),
    )
    .unwrap();
    assert!(!fixture.run().status.success());
    assert!(!fixture.context.exists());
    let _keep_alive = &fixture.temporary;
}

#[test]
fn scripts_separate_network_prefetch_from_cache_only_build() {
    let root = repository_root();
    let prefetch = fs::read_to_string(root.join("scripts/prefetch-workspace-image.sh")).unwrap();
    let build = fs::read_to_string(root.join("scripts/build-offline-workspace-image.sh")).unwrap();
    for required in [
        "fetch-image-artifact",
        "container image pull \"$base_image\"",
        "validate-image-inspect",
        "prepare-workspace-context",
    ] {
        assert!(prefetch.contains(required), "prefetch missing {required}");
    }
    for forbidden in [
        "fetch-image-artifact",
        "curl ",
        "http://",
        "https://",
        "container image pull",
    ] {
        assert!(
            !build.contains(forbidden),
            "build contains network boundary {forbidden}"
        );
    }
    assert!(build.contains("prepare-workspace-context -- --verify"));
    assert!(build.contains("container image inspect --format json \"$base_image\""));
    let preparer =
        fs::read_to_string(root.join("scripts/src/bin/prepare-workspace-context.rs")).unwrap();
    assert!(!preparer.contains("std::process::id"));
    assert!(preparer.contains("/dev/urandom"));
    assert!(preparer.contains("replacement receipt"));
}

#[test]
fn pending_records_fail_before_any_apple_command() {
    let temporary = tempfile::tempdir().unwrap();
    let bin = temporary.path().join("bin");
    fs::create_dir(&bin).unwrap();
    fs::write(
        bin.join("container"),
        "#!/usr/bin/env bash\nprintf invoked >>\"$GASCAN_CALLS\"\nexit 99\n",
    )
    .unwrap();
    fs::set_permissions(bin.join("container"), fs::Permissions::from_mode(0o755)).unwrap();
    let calls = temporary.path().join("calls");
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let output = Command::new("bash")
        .arg(repository_root().join("scripts/build-workspace-image.sh"))
        .env("PATH", path)
        .env("GASCAN_CALLS", &calls)
        .env("CARGO_TARGET_DIR", temporary.path().join("cargo-target"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        !calls.exists(),
        "Apple CLI was invoked before pending lock rejection"
    );
}
