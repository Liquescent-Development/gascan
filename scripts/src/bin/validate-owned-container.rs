use std::{collections::BTreeMap, error::Error, io::Read};

use serde::Deserialize;

type DynError = Box<dyn Error + Send + Sync>;

#[derive(Deserialize)]
struct ContainerRecord {
    id: String,
    configuration: Configuration,
}

#[derive(Deserialize)]
struct Configuration {
    id: String,
    labels: BTreeMap<String, String>,
    image: Option<ContainerImage>,
}

#[derive(Deserialize)]
struct ContainerImage {
    descriptor: ImageDescriptor,
    reference: String,
}

#[derive(Deserialize)]
struct ImageDescriptor {
    digest: String,
}

fn main() -> Result<(), DynError> {
    let mut args = std::env::args().skip(1);
    let name = args.next().ok_or("missing expected container name")?;
    let token = args.next().ok_or("missing expected owner token")?;
    let expected_image = match (args.next(), args.next(), args.next()) {
        (None, None, None) => None,
        (Some(digest), Some(reference), None)
            if digest
                .strip_prefix("sha256:")
                .is_some_and(|value| lower_hex(value, 64))
                && approved_reference(&reference, &digest) =>
        {
            Some((digest, reference))
        }
        _ => return Err("invalid expected container image binding".into()),
    };
    if args.next().is_some() {
        return Err("unexpected ownership validator argument".into());
    }
    if !lower_hex(&token, 32) {
        return Err("owner token must be 128-bit lowercase hexadecimal".into());
    }
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let records: Vec<ContainerRecord> = serde_json::from_str(&input)?;
    if records.len() != 1 {
        return Err("inspect must contain exactly one container record".into());
    }
    let record = &records[0];
    let configuration = &record.configuration;
    if record.id != name || configuration.id != name {
        return Err("container identity does not match the expected name".into());
    }
    if configuration
        .labels
        .get("dev.gascan.test")
        .map(String::as_str)
        != Some("true")
        || configuration
            .labels
            .get("dev.gascan.test.owner")
            .map(String::as_str)
            != Some(token.as_str())
    {
        return Err("container ownership labels do not match".into());
    }
    if let Some((digest, reference)) = expected_image {
        let image = configuration
            .image
            .as_ref()
            .ok_or("container inspection omitted image binding")?;
        if image.descriptor.digest != digest
            || !equivalent_image_reference(&image.reference, &reference, &digest)
        {
            return Err("container image binding does not match the approved image".into());
        }
    }
    Ok(())
}

fn equivalent_image_reference(observed: &str, expected: &str, digest: &str) -> bool {
    if observed == expected {
        return true;
    }
    expected
        .strip_prefix("ghcr.io/liquescent-development/gascan/workspace:")
        .and_then(|value| value.strip_suffix(&format!("@{digest}")))
        .is_some_and(|tag| !tag.is_empty())
        && observed == format!("ghcr.io/liquescent-development/gascan/workspace@{digest}")
}

fn lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn approved_reference(reference: &str, digest: &str) -> bool {
    if reference
        .strip_prefix("gascan-workspace:")
        .is_some_and(|value| lower_hex(value, 16))
    {
        return true;
    }
    let Some(tagged) = reference
        .strip_prefix("ghcr.io/liquescent-development/gascan/workspace:")
        .and_then(|value| value.strip_suffix(&format!("@{digest}")))
    else {
        return false;
    };
    !tagged.is_empty()
        && tagged
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}
