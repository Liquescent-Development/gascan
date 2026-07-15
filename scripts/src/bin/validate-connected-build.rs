use std::{
    error::Error,
    fs,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
};

use serde::Deserialize;
use sha2::{Digest, Sha256};

type DynError = Box<dyn Error + Send + Sync>;

#[derive(Deserialize)]
struct ImageRecord {
    id: String,
    configuration: Configuration,
    variants: Vec<Variant>,
}

#[derive(Deserialize)]
struct Configuration {
    name: String,
    descriptor: Descriptor,
}

#[derive(Deserialize)]
struct Descriptor {
    digest: String,
}

#[derive(Deserialize)]
struct Variant {
    platform: Platform,
}

#[derive(Deserialize)]
struct Platform {
    os: String,
    architecture: String,
}

#[derive(Deserialize)]
struct BuildReceipt {
    reference: String,
    tag: String,
    platform: String,
    lock_digest: String,
    context_digest: String,
    image_digest: String,
    status: String,
}

fn main() -> Result<(), DynError> {
    let mut args = std::env::args().skip(1);
    let first = args.next().ok_or("missing command or expected image tag")?;
    if first == "stage-secret" {
        let wrapper = args.next().ok_or("missing wrapper")?;
        let secret = args.next().ok_or("missing secret")?;
        let repository = args.next().ok_or("missing repository")?;
        if args.next().is_some() {
            return Err("unexpected stage-secret argument".into());
        }
        return stage_secret(
            Path::new(&wrapper),
            Path::new(&secret),
            Path::new(&repository),
        );
    }
    if first == "copy-public" {
        let public = args.next().ok_or("missing public snapshot")?;
        let wrapper = args.next().ok_or("missing wrapper")?;
        let digest = args.next().ok_or("missing context digest")?;
        if args.next().is_some() {
            return Err("unexpected copy-public argument".into());
        }
        verify_root(Path::new(&wrapper), 0o700)?;
        verify_manifest(Path::new(&public), &digest)?;
        return copy_tree(Path::new(&public), Path::new(&wrapper));
    }
    if first == "validate-receipt" {
        let reference = args.next().ok_or("missing reference file")?;
        let receipt = args.next().ok_or("missing receipt file")?;
        let lock_digest = args.next().ok_or("missing lock digest")?;
        let context_digest = args.next().ok_or("missing context digest")?;
        if args.next().is_some() {
            return Err("unexpected validate-receipt argument".into());
        }
        return validate_receipt_pair(
            Path::new(&reference),
            Path::new(&receipt),
            &lock_digest,
            &context_digest,
        );
    }
    if first == "prepare-wrapper" {
        let public = args.next().ok_or("missing public snapshot")?;
        let wrapper = args.next().ok_or("missing wrapper")?;
        let secret = args.next().ok_or("missing secret")?;
        let digest = args.next().ok_or("missing context digest")?;
        return prepare_wrapper(
            Path::new(&public),
            Path::new(&wrapper),
            Path::new(&secret),
            &digest,
        );
    }
    if first == "verify-wrapper" {
        let wrapper = args.next().ok_or("missing wrapper")?;
        let digest = args.next().ok_or("missing context digest")?;
        let identity = args.next().ok_or("missing secret identity")?;
        return verify_wrapper(Path::new(&wrapper), &digest, &identity);
    }
    let expected_tag = first;
    if !valid_tag(&expected_tag) {
        return Err("expected image tag is not an exact immutable build tag".into());
    }
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let mut records: Vec<ImageRecord> = serde_json::from_str(&input)?;
    if records.len() != 1 {
        return Err("inspect must contain exactly one image record".into());
    }
    let record = records.pop().ok_or("inspect record disappeared")?;
    if record.configuration.name != expected_tag {
        return Err("inspect name differs from the exact built tag".into());
    }
    if record.variants.len() != 1
        || record.variants[0].platform.os != "linux"
        || record.variants[0].platform.architecture != "arm64"
    {
        return Err("built image must contain exactly linux/arm64".into());
    }
    let digest = &record.configuration.descriptor.digest;
    if !valid_digest(digest) || record.id != *digest {
        return Err("image id and immutable descriptor digest must match".into());
    }
    println!("{digest}");
    Ok(())
}

fn validate_receipt_pair(
    reference_path: &Path,
    receipt_path: &Path,
    expected_lock: &str,
    expected_context: &str,
) -> Result<(), DynError> {
    if !expected_lock
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || expected_lock.len() != 64
        || !expected_context
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || expected_context.len() != 64
    {
        return Err("expected receipt digests are invalid".into());
    }
    let reference_bytes = fs::read(reference_path)?;
    if !reference_bytes.ends_with(b"\n")
        || reference_bytes[..reference_bytes.len() - 1].contains(&b'\n')
    {
        return Err("reference must contain exactly one line".into());
    }
    let reference = std::str::from_utf8(&reference_bytes[..reference_bytes.len() - 1])?;
    let (tag, image_digest) = reference.rsplit_once('@').ok_or("reference is malformed")?;
    if !valid_tag(tag) || !valid_digest(image_digest) {
        return Err("reference is not exact".into());
    }
    let receipt: BuildReceipt = serde_json::from_slice(&fs::read(receipt_path)?)?;
    if receipt.reference != reference
        || receipt.tag != tag
        || receipt.platform != "linux/arm64"
        || receipt.lock_digest != expected_lock
        || receipt.context_digest != expected_context
        || receipt.image_digest != image_digest
        || receipt.status != "succeeded"
    {
        return Err("receipt pair identities disagree".into());
    }
    Ok(())
}

fn prepare_wrapper(
    public: &Path,
    wrapper: &Path,
    secret: &Path,
    digest: &str,
) -> Result<(), DynError> {
    verify_root(wrapper, 0o700)?;
    verify_manifest(public, digest)?;
    copy_tree(public, wrapper)?;
    let secrets = wrapper.join(".build-secrets");
    fs::create_dir(&secrets)?;
    fs::set_permissions(&secrets, fs::Permissions::from_mode(0o700))?;
    let bytes = read_valid_secret(secret)?;
    let staged = secrets.join("gascamp_read_token");
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staged)?;
    output.write_all(&bytes)?;
    output.sync_all()?;
    let identity = format!("{:x}", Sha256::digest(&bytes));
    let mut ignore = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(wrapper.join(".dockerignore"))?;
    ignore.write_all(b".build-secrets\n")?;
    ignore.sync_all()?;
    println!("{identity}");
    Ok(())
}

fn stage_secret(wrapper: &Path, secret: &Path, repository: &Path) -> Result<(), DynError> {
    verify_root(wrapper, 0o700)?;
    let canonical_secret = fs::canonicalize(secret)?;
    let canonical_repository = fs::canonicalize(repository)?;
    if canonical_secret.starts_with(canonical_repository) {
        return Err("secret file must be outside the repository".into());
    }
    let secrets = wrapper.join(".build-secrets");
    fs::create_dir(&secrets)?;
    fs::set_permissions(&secrets, fs::Permissions::from_mode(0o700))?;
    let bytes = read_valid_secret(secret)?;
    let staged = secrets.join("gascamp_read_token");
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staged)?;
    output.write_all(&bytes)?;
    output.sync_all()?;
    let identity = format!("{:x}", Sha256::digest(&bytes));
    let mut ignore = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(wrapper.join(".dockerignore"))?;
    ignore.write_all(b".build-secrets\n")?;
    ignore.sync_all()?;
    println!("{identity}");
    Ok(())
}

fn read_valid_secret(secret: &Path) -> Result<Vec<u8>, DynError> {
    let mut source = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(secret)?;
    let metadata = source.metadata()?;
    let uid = rustix::process::getuid().as_raw();
    if !metadata.is_file() || metadata.uid() != uid || metadata.mode() & 0o7777 != 0o600 {
        return Err("secret must be a current-UID regular 0600 file".into());
    }
    let mut bytes = Vec::new();
    source.read_to_end(&mut bytes)?;
    if bytes.is_empty()
        || !bytes.ends_with(b"\n")
        || bytes[..bytes.len() - 1].contains(&b'\n')
        || bytes[..bytes.len() - 1].iter().all(u8::is_ascii_whitespace)
    {
        return Err("secret must contain exactly one nonempty line".into());
    }
    Ok(bytes)
}

fn verify_wrapper(wrapper: &Path, digest: &str, identity: &str) -> Result<(), DynError> {
    verify_root(wrapper, 0o700)?;
    verify_manifest(wrapper, digest)?;
    if fs::read(wrapper.join(".dockerignore"))? != b".build-secrets\n" {
        return Err("secret exclusion differs".into());
    }
    let path = wrapper.join(".build-secrets/gascamp_read_token");
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::getuid().as_raw()
        || metadata.mode() & 0o7777 != 0o600
    {
        return Err("staged secret identity is unsafe".into());
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    if format!("{:x}", Sha256::digest(&bytes)) != identity {
        return Err("staged secret content changed".into());
    }
    Ok(())
}

fn verify_root(path: &Path, mode: u32) -> Result<(), DynError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != rustix::process::getuid().as_raw()
        || metadata.mode() & 0o7777 != mode
    {
        return Err("wrapper identity is unsafe".into());
    }
    Ok(())
}

fn verify_manifest(root: &Path, expected: &str) -> Result<(), DynError> {
    let bytes = fs::read(root.join("context-manifest.tsv"))?;
    if format!("{:x}", Sha256::digest(bytes)) != expected {
        return Err("public manifest digest differs".into());
    }
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), DynError> {
    let mut entries = fs::read_dir(source)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let from = entry.path();
        let to = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&from)?;
        if metadata.file_type().is_symlink() {
            return Err("public snapshot contains a symlink".into());
        }
        if metadata.is_dir() {
            fs::create_dir(&to)?;
            fs::set_permissions(&to, fs::Permissions::from_mode(0o700))?;
            copy_tree(&from, &to)?;
        } else if metadata.is_file() {
            let mut input = fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&from)?;
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&to)?;
            std::io::copy(&mut input, &mut output)?;
            output.sync_all()?;
        } else {
            return Err("public snapshot contains a special file".into());
        }
    }
    Ok(())
}

fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn valid_tag(value: &str) -> bool {
    value.starts_with("gascan-workspace:")
        && !value.ends_with(":latest")
        && value["gascan-workspace:".len()..].bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
}
