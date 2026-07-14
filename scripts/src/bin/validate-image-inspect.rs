use std::{error::Error, io::Read};

use serde::Deserialize;

type DynError = Box<dyn Error + Send + Sync>;

#[derive(Deserialize)]
struct ImageRecord {
    configuration: Configuration,
    variants: Vec<Variant>,
}

#[derive(Deserialize)]
struct Configuration {
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
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let digest = validated_digest(&input)?;
    println!("{digest}");
    Ok(())
}

fn validated_digest(input: &str) -> Result<String, DynError> {
    let mut records: Vec<ImageRecord> = serde_json::from_str(input)?;
    if records.len() != 1 {
        return Err("image inspect must contain exactly one image record".into());
    }
    let record = records
        .pop()
        .ok_or("image inspect record disappeared during validation")?;
    if record.variants.len() != 1 {
        return Err("built image must contain exactly one platform variant".into());
    }
    let platform = &record.variants[0].platform;
    if platform.os != "linux" || platform.architecture != "arm64" {
        return Err(format!(
            "built image platform must be linux/arm64, got {}/{}",
            platform.os, platform.architecture
        )
        .into());
    }
    let digest = record.configuration.descriptor.digest.as_str();
    let Some(hex) = digest.strip_prefix("sha256:") else {
        return Err("image index digest must use sha256".into());
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("image index digest must be 64 lowercase hexadecimal characters".into());
    }
    Ok(digest.to_owned())
}
