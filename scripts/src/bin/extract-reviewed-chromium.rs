use std::{
    collections::HashSet,
    error::Error,
    fs, io,
    path::{Component, Path, PathBuf},
};

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
    ensure_empty_or_absent(&output_path)?;
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

fn ensure_empty_or_absent(path: &Path) -> Result<(), DynError> {
    if path.exists() && fs::read_dir(path)?.next().is_some() {
        return Err("Chromium extraction output must be empty".into());
    }
    Ok(())
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
    let staging = tempfile::Builder::new()
        .prefix(".chromium-staging-")
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
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&destination, fs::Permissions::from_mode(mode & 0o777))?;
        }
    }
    if output.exists() {
        fs::remove_dir(output)?;
    }
    let staging_path = staging.keep();
    fs::rename(staging_path, output)?;
    Ok(())
}
