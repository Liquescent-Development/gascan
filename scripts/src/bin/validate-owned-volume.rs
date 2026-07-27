use std::{collections::BTreeMap, error::Error, io::Read};

use serde::Deserialize;

type DynError = Box<dyn Error + Send + Sync>;

// A single volume record is small. Read at most MAX+1 bytes so container CLI
// output cannot cause unbounded memory growth before ownership is established.
const MAX_VOLUME_INSPECT_BYTES: usize = 64 * 1024;

#[derive(Deserialize)]
struct VolumeRecord {
    id: String,
    configuration: Configuration,
}

#[derive(Deserialize)]
struct Configuration {
    name: String,
    labels: BTreeMap<String, String>,
}

fn main() -> Result<(), DynError> {
    let mut args = std::env::args().skip(1);
    let name = args.next().ok_or("missing expected volume name")?;
    let token = args.next().ok_or("missing expected owner token")?;
    if args.next().is_some() {
        return Err("unexpected ownership validator argument".into());
    }
    if !lower_hex(&token, 32) {
        return Err("owner token must be 128-bit lowercase hexadecimal".into());
    }

    let mut input = Vec::with_capacity(MAX_VOLUME_INSPECT_BYTES + 1);
    let stdin = std::io::stdin();
    stdin
        .lock()
        .take((MAX_VOLUME_INSPECT_BYTES + 1) as u64)
        .read_to_end(&mut input)?;
    if input.len() > MAX_VOLUME_INSPECT_BYTES {
        return Err(format!("volume inspect exceeds {MAX_VOLUME_INSPECT_BYTES}-byte limit").into());
    }
    let records: Vec<VolumeRecord> = serde_json::from_slice(&input)?;
    if records.len() != 1 {
        return Err("inspect must contain exactly one volume record".into());
    }

    let record = &records[0];
    if record.id != name || record.configuration.name != name {
        return Err("volume identity does not match the expected name".into());
    }
    if record
        .configuration
        .labels
        .get("dev.gascan.test")
        .map(String::as_str)
        != Some("true")
        || record
            .configuration
            .labels
            .get("dev.gascan.test.owner")
            .map(String::as_str)
            != Some(token.as_str())
    {
        return Err("volume ownership labels do not match".into());
    }
    Ok(())
}

fn lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
