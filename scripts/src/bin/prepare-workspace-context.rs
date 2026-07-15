use std::{
    collections::BTreeSet,
    env,
    error::Error,
    ffi::OsStr,
    fs,
    io::Write,
    os::fd::AsFd,
    os::unix::fs::PermissionsExt,
    path::{Component, Path, PathBuf},
    process::ExitCode,
};

use cap_primitives::fs::{FollowSymlinks, OpenOptions, PermissionsExt as CapPermissionsExt};
use cap_std::{ambient_authority, fs::Dir};
use gascan_image_tools::bundle::{PublishedBundleLocks, validate_bundle};
use sha2::{Digest, Sha256};

type DynError = Box<dyn Error>;

const REPOSITORY_FILES: [&str; 8] = [
    "images/workspace/Dockerfile",
    "images/workspace/bin/gascan-entrypoint",
    "images/workspace/bin/select-gascamp",
    "images/workspace/etc/mise/config.toml",
    "images/workspace/etc/profile.d/mise.sh",
    "images/workspace/etc/sudoers.d/workspace",
    "images/workspace/tests/playwright-smoke.mjs",
    "tests/image/system-tools.txt",
];
const CACHE_FILES: [&str; 2] = ["mise-linux-arm64", "expected-tool-versions.json"];
const RECORDS: [&str; 3] = ["ubuntu_packages", "mise_runtimes", "gascamp_source_vendor"];

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
    let mode = arguments.next();
    if mode.as_deref() == Some(OsStr::new("--verify")) {
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
        let actual = verify_context(&context)?;
        let temporary = tempfile::tempdir()?;
        let expected = temporary.path().join("expected");
        assemble(&repository, &cache, &expected, &locks)?;
        let expected_manifest = verify_context(&expected)?;
        make_tree_owner_writable(&expected)?;
        if actual != expected_manifest {
            return Err("context differs from the current locked inputs".into());
        }
        return Ok(());
    }
    if mode.as_deref() == Some(OsStr::new("--replace")) {
        let repository = required(&mut arguments, "REPOSITORY_ROOT")?;
        let lock = required(&mut arguments, "LOCK_FILE")?;
        let cache = required(&mut arguments, "CACHE_DIRECTORY")?;
        let context = required(&mut arguments, "CONTEXT_DIRECTORY")?;
        if arguments.next().is_some() {
            return Err("usage: prepare-workspace-context --replace REPOSITORY_ROOT LOCK_FILE CACHE_DIRECTORY CONTEXT_DIRECTORY".into());
        }
        let locks = PublishedBundleLocks::from_toml(&fs::read_to_string(lock)?)?;
        return replace_context(&repository, &cache, &context, &locks);
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
    assemble(&repository, &cache, &destination, &locks)
}

fn replace_context(
    repository: &Path,
    cache: &Path,
    destination: &Path,
    locks: &PublishedBundleLocks,
) -> Result<(), DynError> {
    match fs::symlink_metadata(destination) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err("existing context is not a real directory".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return assemble(repository, cache, destination, locks);
        }
        Err(error) => return Err(error.into()),
    }
    let parent_path = destination
        .parent()
        .ok_or("context destination has no parent")?;
    let destination_name = single_name(destination)?;
    let replacement_name = format!(
        ".{}.replacement-{}",
        destination_name.to_string_lossy(),
        std::process::id()
    );
    let replacement = parent_path.join(&replacement_name);
    if replacement.exists() {
        return Err("context replacement path already exists".into());
    }
    assemble(repository, cache, &replacement, locks)?;
    let parent = Dir::open_ambient_dir(parent_path, ambient_authority())?;
    if let Err(error) = rustix::fs::renameat_with(
        parent.as_fd(),
        replacement_name.as_str(),
        parent.as_fd(),
        destination_name,
        rustix::fs::RenameFlags::EXCHANGE,
    ) {
        make_tree_owner_writable(&replacement)?;
        fs::remove_dir_all(&replacement)?;
        return Err(error.into());
    }
    make_tree_owner_writable(&replacement)?;
    fs::remove_dir_all(replacement)?;
    Ok(())
}

fn verify_context(root: &Path) -> Result<Vec<u8>, DynError> {
    let root_metadata = fs::symlink_metadata(root)?;
    if !root_metadata.is_dir()
        || root_metadata.file_type().is_symlink()
        || root_metadata.permissions().mode() & 0o7777 != 0o555
    {
        return Err("context root must be a real read-only 0555 directory".into());
    }
    let allowed: BTreeSet<&str> = [
        ".artifacts",
        "Dockerfile",
        "bundles",
        "context-manifest.tsv",
        "images",
        "tests",
    ]
    .into_iter()
    .collect();
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
    for record in RECORDS {
        if !root.join("bundles").join(record).is_dir() {
            return Err(format!("context is missing bundle {record}").into());
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

fn assemble(
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
