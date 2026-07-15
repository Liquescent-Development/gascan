use std::{
    collections::BTreeSet,
    env,
    error::Error,
    fs,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    process::ExitCode,
};

use cap_primitives::fs::{
    FollowSymlinks, MetadataExt as CapMetadataExt, OpenOptions, PermissionsExt as CapPermissionsExt,
};
use cap_std::{ambient_authority, fs::Dir};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

type DynError = Box<dyn Error>;
const SNAPSHOT_BASE: &str = "/var/tmp/gascan-workspace-build-contexts-v1";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u32,
    token: String,
    manifest_sha256: String,
    device: u64,
    inode: u64,
}

#[derive(Clone)]
enum Entry {
    Directory {
        path: String,
        mode: u32,
    },
    File {
        path: String,
        mode: u32,
        size: u64,
        sha256: String,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("snapshot-workspace-context: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), DynError> {
    let mut args = env::args_os().skip(1);
    let command = args.next().ok_or("missing command; use --help")?;
    if command == "--help" {
        println!(
            "snapshot-workspace-context create SOURCE MANIFEST_SHA256\nsnapshot-workspace-context path RECEIPT\nsnapshot-workspace-context finish RECEIPT"
        );
        return Ok(());
    }
    match command.to_str() {
        Some("create") => {
            let source = PathBuf::from(args.next().ok_or("missing SOURCE")?);
            let expected_manifest = args.next().ok_or("missing MANIFEST_SHA256")?;
            let expected_manifest = expected_manifest
                .to_str()
                .ok_or("MANIFEST_SHA256 is not UTF-8")?;
            if args.next().is_some() {
                return Err("unexpected create argument".into());
            }
            let receipt = create_snapshot(&source, expected_manifest, Path::new(SNAPSHOT_BASE), 0)?;
            println!("{}", serde_json::to_string(&receipt)?);
        }
        Some("path") => {
            let receipt = parse_receipt(args.next().ok_or("missing RECEIPT")?)?;
            if args.next().is_some() {
                return Err("unexpected path argument".into());
            }
            let path = validate_receipt(&receipt, Path::new(SNAPSHOT_BASE), 0)?;
            println!("{}", path.display());
        }
        Some("finish") => {
            let receipt = parse_receipt(args.next().ok_or("missing RECEIPT")?)?;
            if args.next().is_some() {
                return Err("unexpected finish argument".into());
            }
            finish_snapshot(&receipt, Path::new(SNAPSHOT_BASE), 0)?;
        }
        _ => return Err("unknown command; use --help".into()),
    }
    Ok(())
}

fn parse_receipt(value: std::ffi::OsString) -> Result<Receipt, DynError> {
    let text = value.to_str().ok_or("receipt is not UTF-8")?;
    Ok(serde_json::from_str(text)?)
}

fn create_snapshot(
    source_path: &Path,
    expected_manifest: &str,
    base: &Path,
    required_uid: u32,
) -> Result<Receipt, DynError> {
    ensure_base(base, required_uid)?;
    let source = Dir::open_ambient_dir(source_path, ambient_authority())?;
    let manifest = read_regular(&source, Path::new("context-manifest.tsv"))?;
    if !lower_hex(expected_manifest, 64)
        || format!("{:x}", Sha256::digest(&manifest)) != expected_manifest
    {
        return Err("source manifest does not match verified digest".into());
    }
    let entries = parse_manifest(&manifest)?;
    require_exact_source(&source, &entries)?;
    let token = random_token()?;
    let name = format!("snapshot-{token}");
    let destination_path = base.join(&name);
    fs::create_dir(&destination_path)?;
    let result = (|| {
        for entry in &entries {
            match entry {
                Entry::Directory { path, .. } => fs::create_dir(destination_path.join(path))?,
                Entry::File {
                    path,
                    mode,
                    size,
                    sha256,
                } => {
                    copy_verified_file(
                        &source,
                        path,
                        &destination_path.join(path),
                        *mode,
                        *size,
                        sha256,
                    )?;
                }
            }
        }
        fs::write(destination_path.join("context-manifest.tsv"), &manifest)?;
        fs::set_permissions(
            destination_path.join("context-manifest.tsv"),
            fs::Permissions::from_mode(0o444),
        )?;
        for entry in entries.iter().rev() {
            if let Entry::Directory { path, mode } = entry {
                fs::set_permissions(
                    destination_path.join(path),
                    fs::Permissions::from_mode(*mode),
                )?;
            }
        }
        fs::set_permissions(&destination_path, fs::Permissions::from_mode(0o555))?;
        let metadata = fs::symlink_metadata(&destination_path)?;
        let receipt = Receipt {
            version: 1,
            token,
            manifest_sha256: format!("{:x}", Sha256::digest(&manifest)),
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        validate_receipt(&receipt, base, required_uid)?;
        Ok(receipt)
    })();
    if result.is_err() {
        let _ignored = make_writable_and_remove(&destination_path);
    }
    result
}

fn ensure_base(base: &Path, required_uid: u32) -> Result<(), DynError> {
    match fs::symlink_metadata(base) {
        Ok(metadata) => {
            if !metadata.is_dir()
                || metadata.file_type().is_symlink()
                || metadata.uid() != required_uid
                || metadata.mode() & 0o022 != 0
            {
                return Err(
                    "snapshot base must be a real root-owned non-writable directory".into(),
                );
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(base)?;
            fs::set_permissions(base, fs::Permissions::from_mode(0o755))?;
            if fs::symlink_metadata(base)?.uid() != required_uid {
                return Err("snapshot helper has the wrong owner".into());
            }
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn validate_receipt(
    receipt: &Receipt,
    base: &Path,
    required_uid: u32,
) -> Result<PathBuf, DynError> {
    validate_token(&receipt.token)?;
    if receipt.version != 1 || receipt.manifest_sha256.len() != 64 {
        return Err("invalid snapshot receipt".into());
    }
    ensure_base(base, required_uid)?;
    let path = base.join(format!("snapshot-{}", receipt.token));
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != required_uid
        || metadata.dev() != receipt.device
        || metadata.ino() != receipt.inode
        || metadata.mode() & 0o7777 != 0o555
    {
        return Err("snapshot identity differs from receipt".into());
    }
    let root = Dir::open_ambient_dir(&path, ambient_authority())?;
    let manifest = read_regular(&root, Path::new("context-manifest.tsv"))?;
    if format!("{:x}", Sha256::digest(&manifest)) != receipt.manifest_sha256 {
        return Err("snapshot manifest differs from receipt".into());
    }
    let entries = parse_manifest(&manifest)?;
    require_exact_source(&root, &entries)?;
    for entry in &entries {
        match entry {
            Entry::Directory {
                path: relative,
                mode,
            } => {
                let metadata = root.symlink_metadata(relative)?;
                if !metadata.is_dir()
                    || metadata.uid() != required_uid
                    || metadata.mode() & 0o7777 != *mode
                {
                    return Err("snapshot directory identity invalid".into());
                }
            }
            Entry::File {
                path: relative,
                mode,
                size,
                sha256,
            } => {
                let bytes = read_regular(&root, Path::new(relative))?;
                let metadata = root.symlink_metadata(relative)?;
                if metadata.uid() != required_uid
                    || metadata.mode() & 0o7777 != *mode
                    || bytes.len() as u64 != *size
                    || format!("{:x}", Sha256::digest(&bytes)) != *sha256
                {
                    return Err("snapshot file identity invalid".into());
                }
            }
        }
    }
    Ok(path)
}

fn finish_snapshot(receipt: &Receipt, base: &Path, required_uid: u32) -> Result<(), DynError> {
    let path = validate_receipt(receipt, base, required_uid)?;
    make_writable_and_remove(&path)
}

fn copy_verified_file(
    source: &Dir,
    relative: &str,
    destination: &Path,
    mode: u32,
    size: u64,
    sha256: &str,
) -> Result<(), DynError> {
    let mut options = OpenOptions::new();
    options.read(true)._cap_fs_ext_follow(FollowSymlinks::No);
    let mut input = source.open_with(relative, &options)?;
    let metadata = input.metadata()?;
    if !metadata.is_file()
        || metadata.len() != size
        || metadata.permissions().mode() & 0o7777 != mode
    {
        return Err("source file identity differs from manifest".into());
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut copied = 0_u64;
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        copied += count as u64;
        if copied > size {
            return Err("source grew while snapshotting".into());
        }
        hasher.update(&buffer[..count]);
        output.write_all(&buffer[..count])?;
    }
    if copied != size || format!("{:x}", hasher.finalize()) != sha256 {
        return Err("source bytes differ from manifest".into());
    }
    output.sync_all()?;
    fs::set_permissions(destination, fs::Permissions::from_mode(mode))?;
    Ok(())
}

fn read_regular(root: &Dir, relative: &Path) -> Result<Vec<u8>, DynError> {
    let mut options = OpenOptions::new();
    options.read(true)._cap_fs_ext_follow(FollowSymlinks::No);
    let mut file = root.open_with(relative, &options)?;
    if !file.metadata()?.is_file() {
        return Err("expected regular file".into());
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn parse_manifest(bytes: &[u8]) -> Result<Vec<Entry>, DynError> {
    let text = std::str::from_utf8(bytes)?;
    if !text.ends_with('\n') {
        return Err("context manifest is not canonical".into());
    }
    let mut entries = Vec::new();
    let mut previous = None;
    for line in text.lines() {
        let fields: Vec<_> = line.split('\t').collect();
        let path = fields
            .first()
            .ok_or("manifest row missing path")?
            .to_string();
        safe_relative(&path)?;
        if previous
            .as_ref()
            .is_some_and(|value: &String| value >= &path)
        {
            return Err("manifest paths are not sorted and unique".into());
        }
        previous = Some(path.clone());
        match fields.as_slice() {
            [_, "directory", mode] => entries.push(Entry::Directory {
                path,
                mode: parse_mode(mode)?,
            }),
            [_, "file", mode, size, sha256] if lower_hex(sha256, 64) => entries.push(Entry::File {
                path,
                mode: parse_mode(mode)?,
                size: size.parse()?,
                sha256: (*sha256).to_owned(),
            }),
            _ => return Err("invalid context manifest row".into()),
        }
    }
    Ok(entries)
}

fn require_exact_source(root: &Dir, entries: &[Entry]) -> Result<(), DynError> {
    let mut expected: BTreeSet<String> = entries
        .iter()
        .map(|entry| match entry {
            Entry::Directory { path, .. } | Entry::File { path, .. } => path.clone(),
        })
        .collect();
    expected.insert("context-manifest.tsv".to_owned());
    let actual = collect_paths(root, Path::new(""))?;
    if actual != expected {
        return Err("snapshot source contains missing or extra paths".into());
    }
    Ok(())
}

fn collect_paths(root: &Dir, prefix: &Path) -> Result<BTreeSet<String>, DynError> {
    let directory = if prefix.as_os_str().is_empty() {
        root.try_clone()?
    } else {
        root.open_dir(prefix)?
    };
    let mut result = BTreeSet::new();
    for entry in directory.entries()? {
        let entry = entry?;
        let relative = prefix.join(entry.file_name());
        let text = relative
            .to_str()
            .ok_or("snapshot path is not UTF-8")?
            .to_owned();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() || (!file_type.is_dir() && !file_type.is_file()) {
            return Err("snapshot source contains a link or special file".into());
        }
        result.insert(text);
        if file_type.is_dir() {
            result.extend(collect_paths(root, &relative)?);
        }
    }
    Ok(result)
}

fn make_writable_and_remove(path: &Path) -> Result<(), DynError> {
    let mut paths = Vec::new();
    fn visit(path: &Path, paths: &mut Vec<PathBuf>) -> Result<(), DynError> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let child = entry.path();
            let metadata = fs::symlink_metadata(&child)?;
            if metadata.file_type().is_symlink() || (!metadata.is_dir() && !metadata.is_file()) {
                return Err("snapshot cleanup encountered unsafe entry".into());
            }
            if metadata.is_dir() {
                visit(&child, paths)?;
            }
            paths.push(child);
        }
        Ok(())
    }
    visit(path, &mut paths)?;
    for entry in paths.into_iter().rev() {
        let metadata = fs::symlink_metadata(&entry)?;
        fs::set_permissions(
            &entry,
            fs::Permissions::from_mode(if metadata.is_dir() { 0o700 } else { 0o600 }),
        )?;
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    fs::remove_dir_all(path)?;
    Ok(())
}

fn random_token() -> Result<String, DynError> {
    let mut bytes = [0_u8; 32];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn validate_token(token: &str) -> Result<(), DynError> {
    if !lower_hex(token, 64) {
        return Err("invalid snapshot token".into());
    }
    Ok(())
}

fn safe_relative(path: &str) -> Result<(), DynError> {
    let path = Path::new(path);
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("unsafe manifest path".into());
    }
    Ok(())
}

fn parse_mode(value: &str) -> Result<u32, DynError> {
    if value.len() != 4 {
        return Err("invalid manifest mode".into());
    }
    Ok(u32::from_str_radix(value, 8)?)
}

fn lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(root: &Path, body: &[u8]) {
        fs::create_dir(root).unwrap();
        fs::write(root.join("Dockerfile"), body).unwrap();
        fs::set_permissions(root.join("Dockerfile"), fs::Permissions::from_mode(0o444)).unwrap();
        fs::write(
            root.join("context-manifest.tsv"),
            format!(
                "Dockerfile\tfile\t0444\t{}\t{:x}\n",
                body.len(),
                Sha256::digest(body)
            ),
        )
        .unwrap();
        fs::set_permissions(
            root.join("context-manifest.tsv"),
            fs::Permissions::from_mode(0o444),
        )
        .unwrap();
        fs::set_permissions(root, fs::Permissions::from_mode(0o555)).unwrap();
    }

    #[test]
    fn source_exchange_after_create_cannot_change_snapshot_bytes() {
        let temporary = tempfile::tempdir().unwrap();
        let source_path = temporary.path().join("source");
        let base = temporary.path().join("snapshots");
        source(&source_path, b"verified\n");
        let uid = fs::symlink_metadata(temporary.path()).unwrap().uid();
        let expected = format!(
            "{:x}",
            Sha256::digest(fs::read(source_path.join("context-manifest.tsv")).unwrap())
        );
        let receipt = create_snapshot(&source_path, &expected, &base, uid).unwrap();
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o755)).unwrap();
        fs::rename(&source_path, temporary.path().join("old-source")).unwrap();
        source(&source_path, b"unverified\n");
        let snapshot = validate_receipt(&receipt, &base, uid).unwrap();
        assert_eq!(
            fs::read(snapshot.join("Dockerfile")).unwrap(),
            b"verified\n"
        );
        finish_snapshot(&receipt, &base, uid).unwrap();
    }

    #[test]
    fn mutated_snapshot_is_rejected_and_not_removed() {
        let temporary = tempfile::tempdir().unwrap();
        let source_path = temporary.path().join("source");
        let base = temporary.path().join("snapshots");
        source(&source_path, b"verified\n");
        let uid = fs::symlink_metadata(temporary.path()).unwrap().uid();
        let expected = format!(
            "{:x}",
            Sha256::digest(fs::read(source_path.join("context-manifest.tsv")).unwrap())
        );
        let receipt = create_snapshot(&source_path, &expected, &base, uid).unwrap();
        let snapshot = base.join(format!("snapshot-{}", receipt.token));
        fs::set_permissions(
            snapshot.join("Dockerfile"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        fs::write(snapshot.join("Dockerfile"), b"unverified\n").unwrap();
        assert!(validate_receipt(&receipt, &base, uid).is_err());
        assert!(finish_snapshot(&receipt, &base, uid).is_err());
        assert!(snapshot.exists());
        make_writable_and_remove(&snapshot).unwrap();
    }
}
