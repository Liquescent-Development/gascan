use std::{error::Error, fs, io::Read, path::Path};

use serde::Deserialize;

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
    digest: String,
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
    if !valid_digest(digest)
        || record.id != digest.strip_prefix("sha256:").unwrap_or_default()
        || !valid_digest(&record.variants[0].digest)
    {
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

fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn valid_tag(value: &str) -> bool {
    let Some(suffix) = value.strip_prefix("gascan-workspace:") else {
        return false;
    };
    !suffix.is_empty()
        && !value.ends_with(":latest")
        && suffix.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
}
