use std::{collections::BTreeMap, error::Error, io::Read};

use serde::Deserialize;

type DynError = Box<dyn Error + Send + Sync>;

#[derive(Deserialize)]
struct ContainerRecord {
    configuration: Configuration,
}

#[derive(Deserialize)]
struct Configuration {
    id: String,
    name: String,
    labels: BTreeMap<String, String>,
}

fn main() -> Result<(), DynError> {
    let mut args = std::env::args().skip(1);
    let name = args.next().ok_or("missing expected container name")?;
    let token = args.next().ok_or("missing expected owner token")?;
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
    let configuration = &records[0].configuration;
    if configuration.id != name || configuration.name != name {
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
    Ok(())
}

fn lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
