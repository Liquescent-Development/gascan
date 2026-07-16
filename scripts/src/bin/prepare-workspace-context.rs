use std::{
    collections::BTreeSet,
    env,
    error::Error,
    ffi::OsStr,
    fs,
    io::{Read, Write},
    os::fd::AsFd,
    os::unix::fs::PermissionsExt,
    path::{Component, Path, PathBuf},
    process::ExitCode,
};

use cap_primitives::fs::{FollowSymlinks, OpenOptions, PermissionsExt as CapPermissionsExt};
use cap_std::{ambient_authority, fs::Dir};
use gascan_image_tools::bundle::{validate_bundle, PublishedBundleLocks};
use gascan_image_tools::{parse_dockerfile_copies, reviewed_input_kind_allowed, ReviewedInputKind};
use serde::Deserialize;
use sha2::{Digest, Sha256};

type DynError = Box<dyn Error>;

const REPOSITORY_FILES: [&str; 10] = [
    "images/workspace/Dockerfile",
    "images/workspace/bin/gascan-entrypoint",
    "images/workspace/bin/select-gascamp",
    "images/workspace/bin/migrate-workspace-identity",
    "images/workspace/libexec/migrate-workspace-identity-core",
    "images/workspace/etc/mise/config.toml",
    "images/workspace/etc/profile.d/mise.sh",
    "images/workspace/etc/sudoers.d/workspace",
    "images/workspace/tests/playwright-smoke.mjs",
    "tests/image/system-tools.txt",
];
const CACHE_FILES: [&str; 2] = ["mise-linux-arm64", "expected-tool-versions.json"];
const RECORDS: [&str; 3] = ["ubuntu_packages", "mise_runtimes", "gascamp_source_vendor"];

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Offline,
    Connected,
}

#[derive(Deserialize)]
struct ConnectedLock {
    base_image: String,
    workspace_build_mode: String,
    mise: ConnectedArtifact,
    playwright_chromium: ConnectedArtifact,
    gascamp: ConnectedGascamp,
    workspace_bundles: ConnectedBundles,
}

#[derive(Deserialize)]
struct ConnectedGascamp {
    revision: String,
}

#[derive(Deserialize)]
struct ConnectedArtifact {
    url: String,
    sha256: String,
}

#[derive(Deserialize)]
struct ConnectedBundles {
    publication: String,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("prepare-workspace-context: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), DynError> {
    let mut arguments = env::args_os().skip(1);
    let first = arguments.next();
    if first.as_deref() == Some(OsStr::new("--connected-lock")) {
        let lock_path = required(&mut arguments, "LOCK_FILE")?;
        if arguments.next().is_some() {
            return Err("usage: prepare-workspace-context --connected-lock LOCK_FILE".into());
        }
        let lock = parse_connected_lock(&fs::read_to_string(lock_path)?)?;
        for value in [
            &lock.base_image,
            &lock.mise.url,
            &lock.mise.sha256,
            &lock.playwright_chromium.url,
            &lock.playwright_chromium.sha256,
        ] {
            println!("{value}");
        }
        return Ok(());
    }
    if first.as_deref() == Some(OsStr::new("--mode")) {
        let selected = required(&mut arguments, "MODE")?;
        if selected != Path::new("connected") {
            return Err("connected context mode must be exactly 'connected'".into());
        }
        if arguments.next().as_deref() != Some(OsStr::new("--replace")) {
            return Err("usage: prepare-workspace-context --mode connected --replace REPOSITORY_ROOT LOCK_FILE CACHE_DIRECTORY CONTEXT_DIRECTORY".into());
        }
        let repository = required(&mut arguments, "REPOSITORY_ROOT")?;
        let lock = required(&mut arguments, "LOCK_FILE")?;
        let cache = required(&mut arguments, "CACHE_DIRECTORY")?;
        let context = required(&mut arguments, "CONTEXT_DIRECTORY")?;
        if arguments.next().is_some() {
            return Err("usage: prepare-workspace-context --mode connected --replace REPOSITORY_ROOT LOCK_FILE CACHE_DIRECTORY CONTEXT_DIRECTORY".into());
        }
        validate_connected_lock(&repository, &lock)?;
        replace_context(&repository, &cache, &context, Mode::Connected)?;
        let manifest = verify_context(&context, Mode::Connected)?;
        println!("{:x}", Sha256::digest(&manifest));
        return Ok(());
    }
    if first.as_deref() == Some(OsStr::new("--verify-connected")) {
        let repository = required(&mut arguments, "REPOSITORY_ROOT")?;
        let lock = required(&mut arguments, "LOCK_FILE")?;
        let _cache = required(&mut arguments, "CACHE_DIRECTORY")?;
        let context = required(&mut arguments, "CONTEXT_DIRECTORY")?;
        if arguments.next().is_some() {
            return Err("usage: prepare-workspace-context --verify-connected REPOSITORY_ROOT LOCK_FILE CACHE_DIRECTORY CONTEXT_DIRECTORY".into());
        }
        validate_connected_lock(&repository, &lock)?;
        let manifest = verify_context(&context, Mode::Connected)?;
        println!("{:x}", Sha256::digest(&manifest));
        return Ok(());
    }
    if first.as_deref() == Some(OsStr::new("--verify")) {
        let repository = required(&mut arguments, "REPOSITORY_ROOT")?;
        let lock = required(&mut arguments, "LOCK_FILE")?;
        let cache = required(&mut arguments, "CACHE_DIRECTORY")?;
        let context = required(&mut arguments, "CONTEXT_DIRECTORY")?;
        if arguments.next().is_some() {
            return Err(
                "usage: prepare-workspace-context --verify REPOSITORY_ROOT LOCK_FILE CACHE_DIRECTORY CONTEXT_DIRECTORY".into(),
            );
        }
        let locks = PublishedBundleLocks::from_toml(&fs::read_to_string(lock)?)?;
        let actual = verify_context(&context, Mode::Offline)?;
        let temporary = tempfile::tempdir()?;
        let expected = temporary.path().join("expected");
        assemble_offline(&repository, &cache, &expected, &locks)?;
        let expected_manifest = verify_context(&expected, Mode::Offline)?;
        make_tree_owner_writable(&expected)?;
        if actual != expected_manifest {
            return Err("context differs from the current locked inputs".into());
        }
        println!("{:x}", Sha256::digest(&actual));
        return Ok(());
    }
    if first.as_deref() == Some(OsStr::new("--replace")) {
        let repository = required(&mut arguments, "REPOSITORY_ROOT")?;
        let lock = required(&mut arguments, "LOCK_FILE")?;
        let cache = required(&mut arguments, "CACHE_DIRECTORY")?;
        let context = required(&mut arguments, "CONTEXT_DIRECTORY")?;
        if arguments.next().is_some() {
            return Err("usage: prepare-workspace-context --replace REPOSITORY_ROOT LOCK_FILE CACHE_DIRECTORY CONTEXT_DIRECTORY".into());
        }
        let locks = PublishedBundleLocks::from_toml(&fs::read_to_string(lock)?)?;
        return replace_context_offline(&repository, &cache, &context, &locks);
    }
    let mut arguments = env::args_os().skip(1);
    let repository = required(&mut arguments, "REPOSITORY_ROOT")?;
    let lock_path = required(&mut arguments, "LOCK_FILE")?;
    let cache = required(&mut arguments, "CACHE_DIRECTORY")?;
    let destination = required(&mut arguments, "CONTEXT_DIRECTORY")?;
    if arguments.next().is_some() {
        return Err("usage: prepare-workspace-context REPOSITORY_ROOT LOCK_FILE CACHE_DIRECTORY CONTEXT_DIRECTORY".into());
    }
    let locks = PublishedBundleLocks::from_toml(&fs::read_to_string(lock_path)?)?;
    assemble_offline(&repository, &cache, &destination, &locks)
}

fn validate_connected_lock(repository: &Path, lock_path: &Path) -> Result<(), DynError> {
    let expected_path = repository.join("images/workspace/versions.lock");
    let contents = fs::read_to_string(lock_path)?;
    if fs::read(&expected_path)? != contents.as_bytes() {
        return Err("connected lock must be the repository versions.lock".into());
    }
    parse_connected_lock(&contents)?;
    Ok(())
}

fn parse_connected_lock(contents: &str) -> Result<ConnectedLock, DynError> {
    let lock: ConnectedLock = toml::from_str(contents)?;
    if lock.workspace_build_mode != "connected" {
        return Err("workspace_build_mode must be exactly 'connected'".into());
    }
    if lock.workspace_bundles.publication != "pending" {
        return Err("connected mode requires deferred workspace_bundles publication".into());
    }
    if lock.gascamp.revision != "f6b248c5926240856dbea83d1d2c5c90ea1c1456" {
        return Err("Gascamp revision differs from the reviewed connected revision".into());
    }
    if !lock.base_image.starts_with("ubuntu@sha256:")
        || !lower_hex(lock.base_image.trim_start_matches("ubuntu@sha256:"), 64)
    {
        return Err("connected base image must be an immutable Ubuntu digest".into());
    }
    for artifact in [&lock.mise, &lock.playwright_chromium] {
        if !artifact.url.starts_with("https://") || !lower_hex(&artifact.sha256, 64) {
            return Err("connected artifact lock is invalid".into());
        }
    }
    Ok(lock)
}

fn replace_context_offline(
    repository: &Path,
    cache: &Path,
    destination: &Path,
    locks: &PublishedBundleLocks,
) -> Result<(), DynError> {
    replace_context_inner(repository, cache, destination, Mode::Offline, Some(locks))
}

fn replace_context(
    repository: &Path,
    cache: &Path,
    destination: &Path,
    mode: Mode,
) -> Result<(), DynError> {
    replace_context_inner(repository, cache, destination, mode, None)
}

fn replace_context_inner(
    repository: &Path,
    cache: &Path,
    destination: &Path,
    mode: Mode,
    locks: Option<&PublishedBundleLocks>,
) -> Result<(), DynError> {
    let parent_path = destination
        .parent()
        .ok_or("context destination has no parent")?;
    let destination_name = single_name(destination)?;
    recover_owned_replacements(parent_path, destination_name);
    match fs::symlink_metadata(destination) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err("existing context is not a real directory".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return assemble_mode(repository, cache, destination, mode, locks);
        }
        Err(error) => return Err(error.into()),
    }
    let old_sha = verify_context(destination, mode)
        .map(|manifest| format!("{:x}", Sha256::digest(&manifest)))
        .unwrap_or_else(|_| "-".to_owned());
    let token = random_token()?;
    let replacement_name = format!(
        ".{}.replacement-{}",
        destination_name.to_string_lossy(),
        token
    );
    let replacement = parent_path.join(&replacement_name);
    assemble_mode(repository, cache, &replacement, mode, locks)?;
    let new_manifest = verify_context(&replacement, mode)?;
    let new_sha = format!("{:x}", Sha256::digest(&new_manifest));
    let receipt_name = format!("{replacement_name}.receipt");
    let receipt_path = parent_path.join(&receipt_name);
    let receipt = format!("replacement receipt v1\t{token}\t{old_sha}\t{new_sha}\n");
    let mut receipt_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&receipt_path)?;
    receipt_file.write_all(receipt.as_bytes())?;
    receipt_file.sync_all()?;
    fs::set_permissions(&receipt_path, fs::Permissions::from_mode(0o444))?;
    let parent = Dir::open_ambient_dir(parent_path, ambient_authority())?;
    if let Err(error) = rustix::fs::renameat_with(
        parent.as_fd(),
        replacement_name.as_str(),
        parent.as_fd(),
        destination_name,
        rustix::fs::RenameFlags::EXCHANGE,
    ) {
        let _ignored = make_tree_owner_writable(&replacement)
            .and_then(|()| fs::remove_dir_all(&replacement).map_err(Into::into));
        let _ignored = fs::remove_file(receipt_path);
        return Err(error.into());
    }
    let _ignored = make_tree_owner_writable(&replacement)
        .and_then(|()| fs::remove_dir_all(&replacement).map_err(Into::into));
    let _ignored = fs::remove_file(receipt_path);
    Ok(())
}

fn recover_owned_replacements(parent: &Path, destination_name: &OsStr) {
    let prefix = format!(".{}.replacement-", destination_name.to_string_lossy());
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(base) = name.strip_suffix(".receipt") else {
            continue;
        };
        let Some(token) = base.strip_prefix(&prefix) else {
            continue;
        };
        if !lower_hex(token, 64) {
            continue;
        }
        let Ok(receipt) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let fields: Vec<_> = receipt.trim_end().split('\t').collect();
        if fields.len() != 4
            || fields[0] != "replacement receipt v1"
            || fields[1] != token
            || (fields[2] != "-" && !lower_hex(fields[2], 64))
            || !lower_hex(fields[3], 64)
        {
            continue;
        }
        let replacement = parent.join(base);
        if !replacement.exists() {
            let _ignored = fs::remove_file(entry.path());
            continue;
        }
        let Ok(manifest) = verify_context_any(&replacement) else {
            continue;
        };
        let digest = format!("{:x}", Sha256::digest(&manifest));
        if digest != fields[2] && digest != fields[3] {
            continue;
        }
        if make_tree_owner_writable(&replacement)
            .and_then(|()| fs::remove_dir_all(&replacement).map_err(Into::into))
            .is_ok()
        {
            let _ignored = fs::remove_file(entry.path());
        }
    }
}

fn random_token() -> Result<String, DynError> {
    let mut bytes = [0_u8; 32];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn verify_context_any(root: &Path) -> Result<Vec<u8>, DynError> {
    verify_context(root, Mode::Offline).or_else(|_| verify_context(root, Mode::Connected))
}

fn verify_context(root: &Path, mode: Mode) -> Result<Vec<u8>, DynError> {
    let root_metadata = fs::symlink_metadata(root)?;
    if !root_metadata.is_dir()
        || root_metadata.file_type().is_symlink()
        || root_metadata.permissions().mode() & 0o7777 != 0o555
    {
        return Err("context root must be a real read-only 0555 directory".into());
    }
    let allowed: BTreeSet<&str> = if mode == Mode::Connected {
        [
            ".artifacts",
            "Dockerfile",
            "context-manifest.tsv",
            "images",
            "tests",
        ]
        .into_iter()
        .collect()
    } else {
        [
            ".artifacts",
            "Dockerfile",
            "bundles",
            "context-manifest.tsv",
            "images",
            "tests",
        ]
        .into_iter()
        .collect()
    };
    let mut top_level = BTreeSet::new();
    for entry in fs::read_dir(root)? {
        top_level.insert(
            entry?
                .file_name()
                .into_string()
                .map_err(|_| "context path is not UTF-8")?,
        );
    }
    if top_level
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>()
        != allowed
    {
        return Err("context top-level allowlist does not match".into());
    }
    if mode == Mode::Offline {
        for record in RECORDS {
            if !root.join("bundles").join(record).is_dir() {
                return Err(format!("context is missing bundle {record}").into());
            }
        }
    }
    for required in [
        "Dockerfile",
        ".artifacts/mise-linux-arm64",
        ".artifacts/expected-tool-versions.json",
        ".artifacts/playwright-chromium-reviewed/chrome-linux/chrome",
    ] {
        if !root.join(required).is_file() {
            return Err(format!("context is missing {required}").into());
        }
    }
    let expected = fs::read(root.join("context-manifest.tsv"))?;
    let mut rows = Vec::new();
    for path in all_paths(root)? {
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(format!("context contains a symlink: {}", path.display()).into());
        }
        if metadata.permissions().mode() & 0o222 != 0 {
            return Err(format!("context path is writable: {}", path.display()).into());
        }
        let relative = path
            .strip_prefix(root)?
            .to_str()
            .ok_or("context path is not UTF-8")?;
        if metadata.is_dir() {
            rows.push(format!(
                "{relative}\tdirectory\t{:04o}\n",
                metadata.permissions().mode() & 0o7777
            ));
            continue;
        }
        if !metadata.is_file() {
            return Err(format!("context contains a special file: {}", path.display()).into());
        }
        if path.ends_with("context-manifest.tsv") {
            continue;
        }
        let mut file = fs::File::open(&path)?;
        let mut hasher = Sha256::new();
        let size = std::io::copy(&mut file, &mut hasher)?;
        rows.push(format!(
            "{relative}\tfile\t{:04o}\t{size}\t{:x}\n",
            metadata.permissions().mode() & 0o7777,
            hasher.finalize()
        ));
    }
    rows.sort();
    let actual = rows.concat().into_bytes();
    if actual != expected {
        return Err("context manifest does not exactly match context bytes".into());
    }
    Ok(expected)
}

fn assemble_mode(
    repository_path: &Path,
    cache_path: &Path,
    destination: &Path,
    mode: Mode,
    locks: Option<&PublishedBundleLocks>,
) -> Result<(), DynError> {
    match mode {
        Mode::Offline => assemble_offline(
            repository_path,
            cache_path,
            destination,
            locks.ok_or("offline mode requires bundle locks")?,
        ),
        Mode::Connected => assemble_connected(repository_path, cache_path, destination),
    }
}

fn assemble_offline(
    repository_path: &Path,
    cache_path: &Path,
    destination: &Path,
    locks: &PublishedBundleLocks,
) -> Result<(), DynError> {
    let repository = Dir::open_ambient_dir(repository_path, ambient_authority())?;
    let cache = Dir::open_ambient_dir(cache_path, ambient_authority())?;
    let parent_path = destination
        .parent()
        .ok_or("context destination has no parent")?;
    fs::create_dir_all(parent_path)?;
    let destination_name = single_name(destination)?;
    let parent = Dir::open_ambient_dir(parent_path, ambient_authority())?;
    if parent.symlink_metadata(destination_name).is_ok() {
        return Err("context destination already exists".into());
    }
    let temporary = tempfile::Builder::new()
        .prefix(".workspace-context-")
        .tempdir_in(parent_path)?;
    let staging = temporary.path();

    for source in REPOSITORY_FILES {
        let target = if source == "images/workspace/Dockerfile" {
            "Dockerfile"
        } else {
            source
        };
        copy_regular(&repository, Path::new(source), staging.join(target))?;
    }
    for source in CACHE_FILES {
        copy_regular(
            &cache,
            Path::new(source),
            staging.join(".artifacts").join(source),
        )?;
    }
    copy_tree(
        &cache,
        Path::new("playwright-chromium-reviewed"),
        &staging.join(".artifacts/playwright-chromium-reviewed"),
    )?;

    fs::create_dir_all(staging.join("bundles"))?;
    for record in RECORDS {
        let archive = cache_path.join(format!("bundles/{record}.tar.zst"));
        validate_bundle(
            locks.named(record)?,
            &archive,
            &staging.join("bundles").join(record),
        )?;
    }
    make_tree_read_only(staging)?;
    write_manifest(staging)?;
    fs::set_permissions(staging, fs::Permissions::from_mode(0o555))?;
    let staging_name = single_name(staging)?.to_owned();
    let kept = temporary.keep();
    parent.rename(&staging_name, &parent, destination_name)?;
    drop(kept);
    Ok(())
}

fn assemble_connected(
    repository_path: &Path,
    cache_path: &Path,
    destination: &Path,
) -> Result<(), DynError> {
    let repository = Dir::open_ambient_dir(repository_path, ambient_authority())?;
    let cache = Dir::open_ambient_dir(cache_path, ambient_authority())?;
    let parent_path = destination
        .parent()
        .ok_or("context destination has no parent")?;
    fs::create_dir_all(parent_path)?;
    let destination_name = single_name(destination)?;
    let parent = Dir::open_ambient_dir(parent_path, ambient_authority())?;
    if parent.symlink_metadata(destination_name).is_ok() {
        return Err("context destination already exists".into());
    }
    let temporary = tempfile::Builder::new()
        .prefix(".connected-workspace-context-")
        .tempdir_in(parent_path)?;
    let staging = temporary.path();
    copy_regular(
        &repository,
        Path::new("images/workspace/Dockerfile"),
        staging.join("Dockerfile"),
    )?;
    for source in [
        "images/workspace/bin",
        "images/workspace/etc",
        "images/workspace/libexec",
        "images/workspace/tests",
    ] {
        copy_tree_reviewed(&repository, Path::new(source), &staging.join(source))?;
    }
    for source in [
        "images/workspace/versions.lock",
        "tests/image/system-tools.txt",
    ] {
        copy_regular(&repository, Path::new(source), staging.join(source))?;
    }
    for source in ["mise-linux-arm64", "expected-tool-versions.json"] {
        copy_regular(
            &cache,
            Path::new(source),
            staging.join(".artifacts").join(source),
        )?;
    }
    copy_tree_reviewed(
        &cache,
        Path::new("playwright-chromium-reviewed"),
        &staging.join(".artifacts/playwright-chromium-reviewed"),
    )?;
    validate_dockerfile_copy_sources(staging)?;
    make_tree_read_only(staging)?;
    write_manifest(staging)?;
    fs::set_permissions(staging, fs::Permissions::from_mode(0o555))?;
    let staging_name = single_name(staging)?.to_owned();
    let kept = temporary.keep();
    parent.rename(&staging_name, &parent, destination_name)?;
    drop(kept);
    Ok(())
}

fn validate_dockerfile_copy_sources(staging: &Path) -> Result<(), DynError> {
    let dockerfile = fs::read_to_string(staging.join("Dockerfile"))?;
    for copy in parse_dockerfile_copies(&dockerfile).map_err(|error| error.to_string())? {
        if copy.from_stage {
            continue;
        }
        for source_text in copy.sources {
            let source = Path::new(&source_text);
            if source.is_absolute()
                || source
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_)))
            {
                return Err("unsafe Dockerfile COPY source".into());
            }
            if fs::symlink_metadata(staging.join(source)).is_err() {
                return Err(format!(
                    "Dockerfile COPY source was not sealed: {}",
                    source.display()
                )
                .into());
            }
        }
    }
    Ok(())
}

fn token_like(name: &OsStr) -> bool {
    let name = name.to_string_lossy().to_ascii_lowercase();
    name.contains("token") || name.contains("credential") || name.contains("secret")
}

fn copy_tree_reviewed(
    source_root: &Dir,
    source: &Path,
    destination: &Path,
) -> Result<(), DynError> {
    portable_relative(source)?;
    let directory = source_root.open_dir(source)?;
    fs::create_dir_all(destination)?;
    let mut entries = directory.entries()?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        if token_like(&name) {
            return Err(format!(
                "reviewed input contains a token-like filename: {}",
                source.join(&name).display()
            )
            .into());
        }
        let relative = source.join(&name);
        let target = destination.join(&name);
        let metadata = entry.metadata()?;
        let kind = if metadata.is_dir() {
            ReviewedInputKind::Directory
        } else if metadata.is_file() {
            ReviewedInputKind::RegularFile
        } else {
            ReviewedInputKind::Other
        };
        if !reviewed_input_kind_allowed(kind) {
            return Err(format!(
                "reviewed input contains a symlink or special file: {}",
                relative.display()
            )
            .into());
        }
        match kind {
            ReviewedInputKind::Directory => copy_tree_reviewed(source_root, &relative, &target)?,
            ReviewedInputKind::RegularFile => copy_regular(source_root, &relative, target)?,
            ReviewedInputKind::Other => unreachable!(),
        }
    }
    Ok(())
}

fn copy_regular(source_root: &Dir, source: &Path, destination: PathBuf) -> Result<(), DynError> {
    portable_relative(source)?;
    let mut options = OpenOptions::new();
    options.read(true)._cap_fs_ext_follow(FollowSymlinks::No);
    let mut input = source_root.open_with(source, &options)?;
    if !input.metadata()?.is_file() {
        return Err(format!("reviewed input is not a regular file: {}", source.display()).into());
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)?;
    std::io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    let executable = input.metadata()?.permissions().mode() & 0o111 != 0;
    fs::set_permissions(
        destination,
        fs::Permissions::from_mode(if executable { 0o555 } else { 0o444 }),
    )?;
    Ok(())
}

fn copy_tree(source_root: &Dir, source: &Path, destination: &Path) -> Result<(), DynError> {
    portable_relative(source)?;
    let directory = source_root.open_dir(source)?;
    fs::create_dir_all(destination)?;
    let mut entries = directory.entries()?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        let relative = source.join(&name);
        let target = destination.join(&name);
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            copy_tree(source_root, &relative, &target)?;
        } else if metadata.is_file() {
            copy_regular(source_root, &relative, target)?;
        } else {
            return Err(format!(
                "reviewed input contains a symlink or special file: {}",
                relative.display()
            )
            .into());
        }
    }
    Ok(())
}

fn make_tree_read_only(root: &Path) -> Result<(), DynError> {
    for path in all_paths(root)? {
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(format!("context contains a symlink: {}", path.display()).into());
        }
        let mode = metadata.permissions().mode();
        let readonly = if metadata.is_dir() || mode & 0o111 != 0 {
            0o555
        } else {
            0o444
        };
        fs::set_permissions(path, fs::Permissions::from_mode(readonly))?;
    }
    Ok(())
}

fn write_manifest(root: &Path) -> Result<(), DynError> {
    let mut rows = Vec::new();
    for path in all_paths(root)? {
        let metadata = fs::symlink_metadata(&path)?;
        let relative = path
            .strip_prefix(root)?
            .to_str()
            .ok_or("context path is not UTF-8")?;
        if metadata.is_dir() {
            rows.push(format!(
                "{relative}\tdirectory\t{:04o}\n",
                metadata.permissions().mode() & 0o7777
            ));
            continue;
        }
        if !metadata.is_file() {
            return Err(format!("context contains a special file: {}", path.display()).into());
        }
        let mut file = fs::File::open(&path)?;
        let mut hasher = Sha256::new();
        let size = std::io::copy(&mut file, &mut hasher)?;
        rows.push(format!(
            "{relative}\tfile\t{:04o}\t{size}\t{:x}\n",
            metadata.permissions().mode() & 0o7777,
            hasher.finalize()
        ));
    }
    rows.sort();
    let manifest = root.join("context-manifest.tsv");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&manifest)?;
    for row in rows {
        file.write_all(row.as_bytes())?;
    }
    file.sync_all()?;
    fs::set_permissions(manifest, fs::Permissions::from_mode(0o444))?;
    Ok(())
}

fn make_tree_owner_writable(root: &Path) -> Result<(), DynError> {
    let paths = all_paths(root)?;
    for path in paths.into_iter().rev() {
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        } else if metadata.is_file() {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
    }
    fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn all_paths(root: &Path) -> Result<Vec<PathBuf>, DynError> {
    fn visit(directory: &Path, paths: &mut Vec<PathBuf>) -> Result<(), DynError> {
        let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            paths.push(path.clone());
            if metadata.is_dir() {
                visit(&path, paths)?;
            }
        }
        Ok(())
    }
    let mut paths = Vec::new();
    visit(root, &mut paths)?;
    Ok(paths)
}

fn portable_relative(path: &Path) -> Result<(), DynError> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(format!("path is not a portable relative path: {}", path.display()).into());
    }
    Ok(())
}

fn single_name(path: &Path) -> Result<&OsStr, DynError> {
    let name = path.file_name().ok_or("path has no final component")?;
    if Path::new(name).components().count() != 1 {
        return Err("path does not have one final component".into());
    }
    Ok(name)
}

fn required(
    arguments: &mut impl Iterator<Item = std::ffi::OsString>,
    name: &str,
) -> Result<PathBuf, DynError> {
    arguments
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("missing {name}").into())
}
