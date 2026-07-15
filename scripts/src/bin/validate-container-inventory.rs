use std::{error::Error, io::Read};

use serde::Deserialize;

type DynError = Box<dyn Error + Send + Sync>;

#[derive(Deserialize)]
struct ContainerRecord {
    configuration: Configuration,
}

#[derive(Deserialize)]
struct Configuration {
    name: String,
}

fn main() -> Result<(), DynError> {
    let expected: Vec<String> = std::env::args().skip(1).collect();
    if expected.is_empty() {
        return Err("at least one exact container name is required".into());
    }
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let records: Vec<ContainerRecord> = serde_json::from_str(&input)?;
    if let Some(name) = records
        .iter()
        .map(|record| &record.configuration.name)
        .find(|name| expected.contains(name))
    {
        return Err(format!("exact container remains in inventory: {name}").into());
    }
    Ok(())
}
