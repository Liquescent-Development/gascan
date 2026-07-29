use std::{
    collections::HashSet,
    error::Error,
    fs::{self, File},
    io,
    os::fd::AsFd,
    os::unix::ffi::OsStrExt,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};

use cap_std::{ambient_authority, fs::Dir};
use sha2::{Digest, Sha256};
use zip::ZipArchive;

type DynError = Box<dyn Error + Send + Sync>;

fn main() -> Result<(), DynError> {
    let mut args = std::env::args_os().skip(1);
    let archive_path = PathBuf::from(args.next().ok_or("missing Chromium archive path")?);
    let output_path = PathBuf::from(args.next().ok_or("missing extraction output path")?);
    if args.next().is_some() {
        return Err("unexpected Chromium extractor argument".into());
    }

    let entries = validate_archive(&archive_path)?;
    extract_atomically(&archive_path, &output_path, &entries)
}

fn validate_archive(path: &Path) -> Result<Vec<PathBuf>, DynError> {
    let file = fs::File::open(path)?;
    let mut archive = ZipArchive::new(file)?;
    let mut entries = Vec::with_capacity(archive.len());
    let mut seen = HashSet::with_capacity(archive.len());
    let mut chrome_found = false;

    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        let path = reviewed_path(entry.name())?;
        if !seen.insert(path.clone()) {
            return Err("Chromium archive contains a duplicate path".into());
        }
        let mode = entry.unix_mode().unwrap_or(0);
        let file_type = mode & 0o170000;
        if file_type == 0o120000 {
            return Err("Chromium archive symlinks are forbidden".into());
        }
        if file_type != 0 && file_type != 0o040000 && file_type != 0o100000 {
            return Err("Chromium archive contains an unsupported entry type".into());
        }
        if path == Path::new("chrome-linux/chrome") && !entry.is_dir() {
            chrome_found = true;
        }
        entries.push(path);
    }
    if !chrome_found {
        return Err("Chromium archive is missing chrome-linux/chrome".into());
    }
    Ok(entries)
}

fn reviewed_path(name: &str) -> Result<PathBuf, DynError> {
    if name.is_empty() || name.contains('\\') || name.starts_with('/') {
        return Err("Chromium archive path is not portable and relative".into());
    }
    let path = PathBuf::from(name);
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("Chromium archive path traversal is forbidden".into());
    }
    if path
        .components()
        .next()
        .and_then(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        != Some("chrome-linux")
    {
        return Err("Chromium archive has an unexpected top-level layout".into());
    }
    Ok(path)
}

fn extract_atomically(
    archive_path: &Path,
    output: &Path,
    entries: &[PathBuf],
) -> Result<(), DynError> {
    let parent = output
        .parent()
        .ok_or("Chromium output has no parent directory")?;
    fs::create_dir_all(parent)?;
    let output_digest = format!("{:x}", Sha256::digest(output.as_os_str().as_bytes()));
    let _output_lock = lock_output(parent, &output_digest)?;
    let staging_prefix = format!(".chromium-staging-{output_digest}-");
    recover_stale(parent, &staging_prefix);
    let staging = tempfile::Builder::new()
        .prefix(&staging_prefix)
        .tempdir_in(parent)?;
    let file = fs::File::open(archive_path)?;
    let mut archive = ZipArchive::new(file)?;
    for (index, relative) in entries.iter().enumerate() {
        let mut entry = archive.by_index(index)?;
        let destination = staging.path().join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&destination)?;
            continue;
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut target = fs::File::create(&destination)?;
        io::copy(&mut entry, &mut target)?;
        if let Some(mode) = entry.unix_mode() {
            fs::set_permissions(&destination, fs::Permissions::from_mode(mode & 0o777))?;
        }
    }
    let staging_path = staging.keep();
    let staging_name = staging_path
        .file_name()
        .ok_or("Chromium staging path has no name")?;
    let output_name = output.file_name().ok_or("Chromium output has no name")?;
    let new_digest = tree_digest(&staging_path)?;
    let old_digest = match fs::symlink_metadata(output) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            tree_digest(output)?
        }
        Ok(_) => return Err("Chromium output is not a real directory".into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => "-".to_owned(),
        Err(error) => return Err(error.into()),
    };
    let receipt_path = parent.join(format!("{}.receipt", staging_name.to_string_lossy()));
    let receipt = format!(
        "chromium exchange receipt v1\t{}\t{old_digest}\t{new_digest}\n",
        staging_name.to_string_lossy()
    );
    let mut receipt_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&receipt_path)?;
    use std::io::Write as _;
    receipt_file.write_all(receipt.as_bytes())?;
    receipt_file.sync_all()?;
    fs::set_permissions(&receipt_path, fs::Permissions::from_mode(0o444))?;
    let parent_dir = Dir::open_ambient_dir(parent, ambient_authority())?;
    match fs::symlink_metadata(output) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            rustix::fs::renameat_with(
                parent_dir.as_fd(),
                staging_name,
                parent_dir.as_fd(),
                output_name,
                rustix::fs::RenameFlags::EXCHANGE,
            )?;
            if remove_safe_tree(&staging_path).is_ok() {
                let _ignored = fs::remove_file(&receipt_path);
            }
        }
        Ok(_) => return Err("Chromium output is not a real directory".into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            parent_dir.rename(staging_name, &parent_dir, output_name)?;
            let _ignored = fs::remove_file(&receipt_path);
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn lock_output(parent: &Path, output_digest: &str) -> Result<File, DynError> {
    let lock_path = parent.join(format!(".chromium-extraction-{output_digest}.lock"));
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(&lock_path)?;
    let metadata = lock.metadata()?;
    if !metadata.is_file()
        || metadata.mode() & 0o777 != 0o600
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.nlink() != 1
    {
        return Err("Chromium output lock is not a safely permissioned regular file".into());
    }
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive)?;
    let path_metadata = fs::symlink_metadata(&lock_path)?;
    if path_metadata.file_type().is_symlink()
        || !path_metadata.is_file()
        || path_metadata.mode() & 0o777 != 0o600
        || path_metadata.uid() != rustix::process::geteuid().as_raw()
        || path_metadata.nlink() != 1
        || path_metadata.dev() != metadata.dev()
        || path_metadata.ino() != metadata.ino()
    {
        return Err("Chromium output lock path changed while acquiring the lock".into());
    }
    Ok(lock)
}

fn recover_stale(parent: &Path, staging_prefix: &str) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(staging_name) = name.strip_suffix(".receipt") else {
            continue;
        };
        if !staging_name.starts_with(staging_prefix) {
            continue;
        }
        let Ok(receipt) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let fields: Vec<_> = receipt.trim_end().split('\t').collect();
        if fields.len() != 4
            || fields[0] != "chromium exchange receipt v1"
            || fields[1] != staging_name
        {
            continue;
        }
        let staging = parent.join(staging_name);
        if !staging.exists() {
            let _ignored = fs::remove_file(entry.path());
            continue;
        }
        let Ok(digest) = tree_digest(&staging) else {
            continue;
        };
        if digest != fields[2] && digest != fields[3] {
            continue;
        }
        if remove_safe_tree(&staging).is_ok() {
            let _ignored = fs::remove_file(entry.path());
        }
    }
}

fn tree_digest(root: &Path) -> Result<String, DynError> {
    fn visit(base: &Path, directory: &Path, rows: &mut Vec<String>) -> Result<(), DynError> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            let relative = path
                .strip_prefix(base)?
                .to_str()
                .ok_or("Chromium path is not UTF-8")?;
            if metadata.file_type().is_symlink() || (!metadata.is_dir() && !metadata.is_file()) {
                return Err("Chromium tree contains a link or special file".into());
            }
            if metadata.is_dir() {
                rows.push(format!(
                    "{relative}\tdirectory\t{:04o}\n",
                    metadata.permissions().mode() & 0o7777
                ));
                visit(base, &path, rows)?;
            } else {
                let bytes = fs::read(&path)?;
                rows.push(format!(
                    "{relative}\tfile\t{:04o}\t{}\t{:x}\n",
                    metadata.permissions().mode() & 0o7777,
                    bytes.len(),
                    Sha256::digest(bytes)
                ));
            }
        }
        Ok(())
    }
    let metadata = fs::symlink_metadata(root)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("Chromium tree is not a real directory".into());
    }
    let mut rows = Vec::new();
    visit(root, root, &mut rows)?;
    rows.sort();
    Ok(format!("{:x}", Sha256::digest(rows.concat().as_bytes())))
}

fn remove_safe_tree(root: &Path) -> Result<(), DynError> {
    tree_digest(root)?;
    fs::remove_dir_all(root)?;
    Ok(())
}
