use std::{error::Error, io::Read};

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
}

#[derive(Deserialize)]
struct Platform {
    os: String,
    architecture: String,
}

fn main() -> Result<(), DynError> {
    let expected_tag = std::env::args()
        .nth(1)
        .ok_or("missing expected image tag")?;
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
