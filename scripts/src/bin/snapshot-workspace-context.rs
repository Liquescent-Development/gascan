use std::{
    collections::BTreeSet,
    env,
    error::Error,
    fs,
    io::{Read, Write},
    os::fd::AsFd,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};

use cap_primitives::fs::{
    FollowSymlinks, MetadataExt as CapMetadataExt, OpenOptions, PermissionsExt as CapPermissionsExt,
};
use cap_std::{ambient_authority, fs::Dir};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

type DynError = Box<dyn Error>;
const SNAPSHOT_BASE: &str = "/var/tmp/gascan-workspace-build-contexts-v1";
const MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ENTRIES: usize = 1_000_000;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u32,
    token: String,
    manifest_sha256: String,
    device: u64,
    inode: u64,
    caller_uid: u32,
    source_device: u64,
    source_inode: u64,
    source_path: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Incomplete {
    version: u32,
    token: String,
    caller_uid: u32,
    created: u64,
    device: Option<u64>,
    inode: Option<u64>,
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
    let first = args.next().ok_or("missing command; use --help")?;
    if first == "--help" {
        println!(
            "snapshot-workspace-context --self SHA256 DEVICE INODE create SOURCE MANIFEST_SHA256\nsnapshot-workspace-context --self SHA256 DEVICE INODE path RECEIPT\nsnapshot-workspace-context --self SHA256 DEVICE INODE finish RECEIPT"
        );
        return Ok(());
    }
    if first != "--self" {
        return Err("missing required --self identity".into());
    }
    let self_sha = args.next().ok_or("missing self SHA256")?;
    let self_device: u64 = args
        .next()
        .ok_or("missing self device")?
        .to_str()
        .ok_or("invalid self device")?
        .parse()?;
    let self_inode: u64 = args
        .next()
        .ok_or("missing self inode")?
        .to_str()
        .ok_or("invalid self inode")?
        .parse()?;
    verify_self(
        self_sha.to_str().ok_or("invalid self SHA256")?,
        self_device,
        self_inode,
    )?;
    let caller_uid: u32 = env::var("SUDO_UID")
        .map_err(|_| "missing SUDO_UID")?
        .parse()?;
    let command = args.next().ok_or("missing command; use --help")?;
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
            validate_caller_source(&source, caller_uid)?;
            let receipt = create_snapshot(
                &source,
                expected_manifest,
                Path::new(SNAPSHOT_BASE),
                0,
                caller_uid,
            )?;
            println!("{}", serde_json::to_string(&receipt)?);
        }
        Some("path") => {
            let receipt = parse_receipt(args.next().ok_or("missing RECEIPT")?)?;
            if args.next().is_some() {
                return Err("unexpected path argument".into());
            }
            if receipt.caller_uid != caller_uid {
                return Err("receipt belongs to another caller".into());
            }
            let path = validate_receipt(&receipt, Path::new(SNAPSHOT_BASE), 0)?;
            println!("{}", path.display());
        }
        Some("finish") => {
            let receipt = parse_receipt(args.next().ok_or("missing RECEIPT")?)?;
            if args.next().is_some() {
                return Err("unexpected finish argument".into());
            }
            if receipt.caller_uid != caller_uid {
                return Err("receipt belongs to another caller".into());
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
    caller_uid: u32,
) -> Result<Receipt, DynError> {
    ensure_base(base, required_uid)?;
    let _claim = acquire_claim(base, required_uid)?;
    recover_incomplete(base, required_uid, caller_uid, 3600)?;
    let source = open_absolute_dir_nofollow(source_path)?;
    let opened_source = source.dir_metadata()?;
    let named_source = fs::symlink_metadata(source_path)?;
    if opened_source.dev() != named_source.dev() || opened_source.ino() != named_source.ino() {
        return Err("source changed while opening".into());
    }
    let source_metadata = source.dir_metadata()?;
    if !source_metadata.is_dir() || source_metadata.uid() != caller_uid {
        return Err("opened source owner or type is invalid".into());
    }
    let manifest = read_regular_bounded(
        &source,
        Path::new("context-manifest.tsv"),
        MAX_MANIFEST_BYTES,
    )?;
    if !lower_hex(expected_manifest, 64)
        || format!("{:x}", Sha256::digest(&manifest)) != expected_manifest
    {
        return Err("source manifest does not match verified digest".into());
    }
    let entries = parse_manifest(&manifest)?;
    let total = entries.iter().try_fold(0_u64, |sum, entry| match entry {
        Entry::File { size, .. } => sum.checked_add(*size).ok_or("aggregate size overflow"),
        _ => Ok(sum),
    })?;
    if total > MAX_TOTAL_BYTES {
        return Err("snapshot aggregate exceeds limit".into());
    }
    require_exact_source(&source, &entries)?;
    let token = random_token()?;
    let name = format!("snapshot-{token}");
    let destination_path = base.join(&name);
    let marker_path = base.join(format!("incomplete-{token}.json"));
    let mut marker = Incomplete {
        version: 1,
        token: token.clone(),
        caller_uid,
        created: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        device: None,
        inode: None,
    };
    write_private_new(&marker_path, &serde_json::to_vec(&marker)?)?;
    fs::create_dir(&destination_path)?;
    let initial = fs::symlink_metadata(&destination_path)?;
    marker.device = Some(initial.dev());
    marker.inode = Some(initial.ino());
    fs::write(&marker_path, serde_json::to_vec(&marker)?)?;
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
            token: token.clone(),
            manifest_sha256: format!("{:x}", Sha256::digest(&manifest)),
            device: metadata.dev(),
            inode: metadata.ino(),
            caller_uid,
            source_device: source_metadata.dev(),
            source_inode: source_metadata.ino(),
            source_path: source_path
                .to_str()
                .ok_or("source path is not UTF-8")?
                .to_owned(),
        };
        let receipt_path = base.join(format!("receipt-{}.json", receipt.token));
        write_private_new(&receipt_path, &serde_json::to_vec(&receipt)?)?;
        validate_receipt(&receipt, base, required_uid)?;
        fs::remove_file(&marker_path)?;
        Ok(receipt)
    })();
    if result.is_err() {
        let _ignored = make_writable_and_remove(&destination_path);
        let _ignored = fs::remove_file(&marker_path);
        let _ignored = fs::remove_file(base.join(format!("receipt-{token}.json")));
    }
    result
}

fn acquire_claim(base: &Path, required_uid: u32) -> Result<fs::File, DynError> {
    let path = base.join(".create.lock");
    let claim = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    let metadata = claim.metadata()?;
    if !metadata.is_file() || metadata.uid() != required_uid {
        return Err("snapshot create claim identity invalid".into());
    }
    rustix::fs::flock(&claim, rustix::fs::FlockOperation::LockExclusive)?;
    Ok(claim)
}

fn recover_incomplete(
    base: &Path,
    required_uid: u32,
    caller_uid: u32,
    minimum_age: u64,
) -> Result<(), DynError> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    for entry in fs::read_dir(base)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with("incomplete-") || !name.ends_with(".json") {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.uid() != required_uid
            || metadata.mode() & 0o7777 != 0o600
        {
            continue;
        }
        let marker: Incomplete = match serde_json::from_slice(&fs::read(entry.path())?) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if marker.version != 1
            || marker.caller_uid != caller_uid
            || now.saturating_sub(marker.created) < minimum_age
            || validate_token(&marker.token).is_err()
        {
            continue;
        }
        let path = base.join(format!("snapshot-{}", marker.token));
        let snapshot = match fs::symlink_metadata(&path) {
            Ok(value) => value,
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && marker.device.is_none()
                    && marker.inode.is_none() =>
            {
                fs::remove_file(entry.path())?;
                continue;
            }
            Err(_) => continue,
        };
        if !snapshot.is_dir()
            || snapshot.file_type().is_symlink()
            || snapshot.uid() != required_uid
            || (marker.device.is_some() && Some(snapshot.dev()) != marker.device)
            || (marker.inode.is_some() && Some(snapshot.ino()) != marker.inode)
        {
            continue;
        }
        make_writable_and_remove(&path)?;
        remove_recovered_receipt(base, &marker, required_uid)?;
        fs::remove_file(entry.path())?;
    }
    Ok(())
}

fn write_private_new(path: &Path, bytes: &[u8]) -> Result<(), DynError> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn remove_recovered_receipt(
    base: &Path,
    marker: &Incomplete,
    required_uid: u32,
) -> Result<(), DynError> {
    let path = base.join(format!("receipt-{}.json", marker.token));
    let metadata = match fs::symlink_metadata(&path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != required_uid
        || metadata.mode() & 0o7777 != 0o600
    {
        return Err("recovered receipt identity invalid".into());
    }
    let receipt: Receipt = serde_json::from_slice(&fs::read(&path)?)?;
    if receipt.token != marker.token || receipt.caller_uid != marker.caller_uid {
        return Err("recovered receipt does not match marker".into());
    }
    fs::remove_file(path)?;
    Ok(())
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
    let stored_path = base.join(format!("receipt-{}.json", receipt.token));
    let stored_metadata = fs::symlink_metadata(&stored_path)?;
    if !stored_metadata.is_file()
        || stored_metadata.file_type().is_symlink()
        || stored_metadata.uid() != required_uid
        || stored_metadata.mode() & 0o7777 != 0o600
    {
        return Err("stored receipt identity invalid".into());
    }
    let stored: Receipt = serde_json::from_slice(&fs::read(&stored_path)?)?;
    if stored != *receipt {
        return Err("receipt differs from root-owned record".into());
    }
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
    make_writable_and_remove(&path)?;
    fs::remove_file(base.join(format!("receipt-{}.json", receipt.token)))?;
    Ok(())
}

fn copy_verified_file(
    source: &Dir,
    relative: &str,
    destination: &Path,
    mode: u32,
    size: u64,
    sha256: &str,
) -> Result<(), DynError> {
    let (parent, leaf) = resolve_parent_nofollow(source, Path::new(relative))?;
    let mut options = OpenOptions::new();
    options.read(true)._cap_fs_ext_follow(FollowSymlinks::No);
    let mut input = parent.open_with(&leaf, &options)?;
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
    read_regular_bounded(root, relative, MAX_FILE_BYTES)
}

fn read_regular_bounded(root: &Dir, relative: &Path, maximum: u64) -> Result<Vec<u8>, DynError> {
    let (parent, leaf) = resolve_parent_nofollow(root, relative)?;
    let mut options = OpenOptions::new();
    options.read(true)._cap_fs_ext_follow(FollowSymlinks::No);
    let file = parent.open_with(&leaf, &options)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > maximum {
        return Err("expected regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(maximum + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum {
        return Err("file exceeds limit".into());
    }
    Ok(bytes)
}

fn parse_manifest(bytes: &[u8]) -> Result<Vec<Entry>, DynError> {
    let text = std::str::from_utf8(bytes)?;
    if !text.ends_with('\n') {
        return Err("context manifest is not canonical".into());
    }
    let mut entries = Vec::new();
    let mut previous = None;
    let mut total = 0_u64;
    for line in text.lines() {
        if entries.len() >= MAX_ENTRIES {
            return Err("manifest entry count exceeds limit".into());
        }
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
            [_, "file", mode, size, sha256]
                if lower_hex(sha256, 64)
                    && size
                        .parse::<u64>()
                        .is_ok_and(|value| value <= MAX_FILE_BYTES) =>
            {
                let size: u64 = size.parse()?;
                total = total.checked_add(size).ok_or("aggregate size overflow")?;
                if total > MAX_TOTAL_BYTES {
                    return Err("snapshot aggregate exceeds limit".into());
                }
                entries.push(Entry::File {
                    path,
                    mode: parse_mode(mode)?,
                    size,
                    sha256: (*sha256).to_owned(),
                })
            }
            _ => return Err("invalid context manifest row".into()),
        }
    }
    Ok(entries)
}

fn validate_caller_source(path: &Path, caller_uid: u32) -> Result<(), DynError> {
    let canonical = path.canonicalize()?;
    if canonical != path {
        return Err("source path must be canonical and must not be a symlink".into());
    }
    let allowed_name = matches!(
        path.file_name().and_then(|value| value.to_str()),
        Some("workspace-context" | "connected-workspace-context")
    );
    if !allowed_name
        || path
            .parent()
            .and_then(Path::file_name)
            .and_then(|v| v.to_str())
            != Some(".artifacts")
    {
        return Err(
            "source must be a canonical caller .artifacts reviewed workspace context".into(),
        );
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() || metadata.uid() != caller_uid {
        return Err("source owner or type is invalid".into());
    }
    Ok(())
}

fn verify_self(expected_sha: &str, device: u64, inode: u64) -> Result<(), DynError> {
    if !lower_hex(expected_sha, 64) {
        return Err("invalid expected helper digest".into());
    }
    let executable = env::current_exe()?;
    let metadata = fs::symlink_metadata(&executable)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != 0
        || metadata.gid() != 0
        || metadata.mode() & 0o7777 != 0o555
        || metadata.dev() != device
        || metadata.ino() != inode
    {
        return Err("executed helper identity differs".into());
    }
    let bytes = fs::read(executable)?;
    if format!("{:x}", Sha256::digest(bytes)) != expected_sha {
        return Err("executed helper digest differs".into());
    }
    Ok(())
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
    collect_paths_from(root.try_clone()?, prefix)
}

fn collect_paths_from(directory: Dir, prefix: &Path) -> Result<BTreeSet<String>, DynError> {
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
            let child = open_dir_nofollow(&directory, Path::new(&entry.file_name()))?;
            result.extend(collect_paths_from(child, &relative)?);
        }
    }
    Ok(result)
}

fn open_dir_nofollow(parent: &Dir, name: &Path) -> Result<Dir, DynError> {
    if name.components().count() != 1
        || !matches!(name.components().next(), Some(Component::Normal(_)))
    {
        return Err("expected one safe directory component".into());
    }
    let fd = rustix::fs::openat(
        parent.as_fd(),
        name,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?;
    Ok(Dir::from_std_file(fs::File::from(fd)))
}

fn open_absolute_dir_nofollow(path: &Path) -> Result<Dir, DynError> {
    if !path.is_absolute() {
        return Err("source path must be absolute".into());
    }
    let mut current = Dir::open_ambient_dir("/", ambient_authority())?;
    for component in path.components().skip(1) {
        let Component::Normal(name) = component else {
            return Err("unsafe absolute source path".into());
        };
        current = open_dir_nofollow(&current, Path::new(name))?;
    }
    Ok(current)
}

fn resolve_parent_nofollow(
    root: &Dir,
    relative: &Path,
) -> Result<(Dir, std::ffi::OsString), DynError> {
    let mut components = relative.components().peekable();
    let mut current = root.try_clone()?;
    loop {
        let Component::Normal(name) = components.next().ok_or("empty source path")? else {
            return Err("unsafe source path".into());
        };
        if components.peek().is_none() {
            return Ok((current, name.to_os_string()));
        }
        current = open_dir_nofollow(&current, Path::new(name))?;
    }
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
    use std::os::unix::fs::symlink;

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
        let temporary_path = temporary.path().canonicalize().unwrap();
        let source_path = temporary_path.join("source");
        let base = temporary_path.join("snapshots");
        source(&source_path, b"verified\n");
        let uid = fs::symlink_metadata(temporary.path()).unwrap().uid();
        let expected = format!(
            "{:x}",
            Sha256::digest(fs::read(source_path.join("context-manifest.tsv")).unwrap())
        );
        let receipt = create_snapshot(&source_path, &expected, &base, uid, uid).unwrap();
        let mut forged = receipt.clone();
        forged.source_inode = forged.source_inode.wrapping_add(1);
        assert!(validate_receipt(&forged, &base, uid).is_err());
        fs::set_permissions(&temporary_path, fs::Permissions::from_mode(0o755)).unwrap();
        fs::rename(&source_path, temporary_path.join("old-source")).unwrap();
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
        let temporary_path = temporary.path().canonicalize().unwrap();
        let source_path = temporary_path.join("source");
        let base = temporary_path.join("snapshots");
        source(&source_path, b"verified\n");
        let uid = fs::symlink_metadata(temporary.path()).unwrap().uid();
        let expected = format!(
            "{:x}",
            Sha256::digest(fs::read(source_path.join("context-manifest.tsv")).unwrap())
        );
        let receipt = create_snapshot(&source_path, &expected, &base, uid, uid).unwrap();
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

    #[test]
    fn individual_resource_limit_is_enforced() {
        let oversized = format!(
            "huge\tfile\t0444\t{}\t{}\n",
            MAX_FILE_BYTES + 1,
            "0".repeat(64)
        );
        assert!(parse_manifest(oversized.as_bytes()).is_err());
        let aggregate = (0..11)
            .map(|index| {
                format!(
                    "f{index:02}\tfile\t0444\t{MAX_FILE_BYTES}\t{}\n",
                    "0".repeat(64)
                )
            })
            .collect::<String>();
        assert!(parse_manifest(aggregate.as_bytes()).is_err());
    }

    #[test]
    fn arbitrary_source_path_is_rejected() {
        let temporary = tempfile::tempdir().unwrap();
        let uid = fs::symlink_metadata(temporary.path()).unwrap().uid();
        assert!(validate_caller_source(temporary.path(), uid).is_err());
    }

    fn caller_source_path(temporary: &tempfile::TempDir, name: &str) -> PathBuf {
        let artifacts = temporary.path().join(".artifacts");
        fs::create_dir_all(&artifacts).unwrap();
        let source = artifacts.join(name);
        fs::create_dir(&source).unwrap();
        source.canonicalize().unwrap()
    }

    #[test]
    fn reviewed_workspace_context_names_are_accepted() {
        for name in ["workspace-context", "connected-workspace-context"] {
            let temporary = tempfile::tempdir().unwrap();
            let source = caller_source_path(&temporary, name);
            let uid = fs::symlink_metadata(&source).unwrap().uid();
            assert!(validate_caller_source(&source, uid).is_ok(), "{name}");
        }
    }

    #[test]
    fn unreviewed_source_locations_are_rejected() {
        let sibling_root = tempfile::tempdir().unwrap();
        let sibling = caller_source_path(&sibling_root, "other-workspace-context");
        let uid = fs::symlink_metadata(&sibling).unwrap().uid();
        assert!(validate_caller_source(&sibling, uid).is_err());

        let other_parent_root = tempfile::tempdir().unwrap();
        let other_parent = other_parent_root.path().join("build");
        fs::create_dir(&other_parent).unwrap();
        let source = other_parent.join("connected-workspace-context");
        fs::create_dir(&source).unwrap();
        assert!(validate_caller_source(&source, uid).is_err());
    }

    #[test]
    fn aliases_wrong_owner_and_non_directories_are_rejected() {
        let temporary = tempfile::tempdir().unwrap();
        let artifacts = temporary.path().join(".artifacts");
        fs::create_dir(&artifacts).unwrap();
        let target = temporary.path().join("actual-context");
        fs::create_dir(&target).unwrap();
        let alias = artifacts.join("connected-workspace-context");
        symlink(&target, &alias).unwrap();
        let uid = fs::symlink_metadata(&target).unwrap().uid();
        assert_eq!(alias.file_name().unwrap(), "connected-workspace-context");
        assert_eq!(alias.parent().unwrap().file_name().unwrap(), ".artifacts");
        assert_ne!(alias.canonicalize().unwrap(), alias);
        assert!(
            fs::symlink_metadata(&alias)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            validate_caller_source(&alias, uid).unwrap_err().to_string(),
            "source path must be canonical and must not be a symlink"
        );

        let source = caller_source_path(&temporary, "workspace-context");
        assert!(validate_caller_source(&source, uid + 1).is_err());

        let file_root = tempfile::tempdir().unwrap();
        let artifacts = file_root.path().join(".artifacts");
        fs::create_dir(&artifacts).unwrap();
        let file = artifacts.join("connected-workspace-context");
        fs::write(&file, b"not a directory").unwrap();
        assert!(validate_caller_source(&file, uid).is_err());
    }

    #[test]
    fn stale_incomplete_recovery_never_deletes_foreign_snapshot() {
        let temporary = tempfile::tempdir().unwrap();
        let base = temporary.path().join("base");
        fs::create_dir(&base).unwrap();
        fs::set_permissions(&base, fs::Permissions::from_mode(0o755)).unwrap();
        let uid = fs::symlink_metadata(temporary.path()).unwrap().uid();
        let token = "a".repeat(64);
        let snapshot = base.join(format!("snapshot-{token}"));
        fs::create_dir(&snapshot).unwrap();
        let identity = fs::symlink_metadata(&snapshot).unwrap();
        let marker = Incomplete {
            version: 1,
            token: token.clone(),
            caller_uid: uid + 1,
            created: 0,
            device: Some(identity.dev()),
            inode: Some(identity.ino()),
        };
        let marker_path = base.join(format!("incomplete-{token}.json"));
        fs::write(&marker_path, serde_json::to_vec(&marker).unwrap()).unwrap();
        fs::set_permissions(&marker_path, fs::Permissions::from_mode(0o600)).unwrap();
        recover_incomplete(&base, uid, uid, 0).unwrap();
        assert!(snapshot.exists());
        let mut owned = marker;
        owned.caller_uid = uid;
        fs::write(&marker_path, serde_json::to_vec(&owned).unwrap()).unwrap();
        let receipt_path = base.join(format!("receipt-{token}.json"));
        let receipt = Receipt {
            version: 1,
            token: token.clone(),
            manifest_sha256: "0".repeat(64),
            device: identity.dev(),
            inode: identity.ino(),
            caller_uid: uid,
            source_device: 1,
            source_inode: 1,
            source_path: "/caller/.artifacts/workspace-context".to_owned(),
        };
        write_private_new(&receipt_path, &serde_json::to_vec(&receipt).unwrap()).unwrap();
        recover_incomplete(&base, uid, uid, 0).unwrap();
        assert!(!snapshot.exists());
        assert!(!receipt_path.exists());
    }

    #[test]
    fn exchanged_intermediate_directory_is_rejected_componentwise() {
        let temporary = tempfile::tempdir().unwrap();
        let root_path = temporary.path().join("root");
        let outside = temporary.path().join("outside");
        fs::create_dir(&root_path).unwrap();
        fs::create_dir(root_path.join("nested")).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("secret"), b"known bytes").unwrap();
        let root = Dir::open_ambient_dir(&root_path, ambient_authority()).unwrap();
        fs::rename(root_path.join("nested"), root_path.join("old-nested")).unwrap();
        symlink(&outside, root_path.join("nested")).unwrap();
        assert!(read_regular(&root, Path::new("nested/secret")).is_err());
    }

    #[test]
    fn active_create_claim_cannot_be_reclaimed() {
        let temporary = tempfile::tempdir().unwrap();
        let uid = fs::symlink_metadata(temporary.path()).unwrap().uid();
        let _claim = acquire_claim(temporary.path(), uid).unwrap();
        let contender = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(temporary.path().join(".create.lock"))
            .unwrap();
        assert!(
            rustix::fs::flock(
                &contender,
                rustix::fs::FlockOperation::NonBlockingLockExclusive
            )
            .is_err()
        );
    }

    #[test]
    fn earliest_incomplete_marker_is_recovered() {
        let temporary = tempfile::tempdir().unwrap();
        let uid = fs::symlink_metadata(temporary.path()).unwrap().uid();
        let token = "b".repeat(64);
        let marker = Incomplete {
            version: 1,
            token: token.clone(),
            caller_uid: uid,
            created: 0,
            device: None,
            inode: None,
        };
        let path = temporary.path().join(format!("incomplete-{token}.json"));
        fs::write(&path, serde_json::to_vec(&marker).unwrap()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        recover_incomplete(temporary.path(), uid, uid, 0).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn private_records_are_mode_0600_from_creation() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("record");
        write_private_new(&path, b"record").unwrap();
        assert_eq!(fs::symlink_metadata(path).unwrap().mode() & 0o7777, 0o600);
    }
}
