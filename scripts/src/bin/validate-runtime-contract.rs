use std::{
    collections::HashSet,
    env, fs,
    path::{Component, Path},
    process::ExitCode,
};

use serde::Deserialize;

const GENERATED_MISE_SOURCE: &str = ".artifacts/mise-linux-arm64";

#[derive(Deserialize)]
struct Contract {
    version: u32,
    helpers: Vec<Helper>,
}

#[derive(Deserialize)]
struct Helper {
    path: String,
    source: String,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("validate-runtime-contract: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os().skip(1);
    let root = arguments
        .next()
        .ok_or("usage: validate-runtime-contract ROOT")?;
    if arguments.next().is_some() {
        return Err("usage: validate-runtime-contract ROOT".into());
    }
    validate(Path::new(&root))
}

fn validate(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let contract: Contract = toml::from_str(&fs::read_to_string(
        root.join("images/workspace/runtime-contract.toml"),
    )?)?;
    if contract.version != 1 {
        return Err(format!("unsupported runtime contract version {}", contract.version).into());
    }

    let dockerfile = fs::read_to_string(root.join("images/workspace/Dockerfile"))?;
    let service = fs::read_to_string(root.join("crates/gascand/src/service.rs"))?;
    let mut paths = HashSet::new();
    let mut sources = HashSet::new();

    for helper in &contract.helpers {
        if !Path::new(&helper.path).is_absolute() {
            return Err(format!("helper path is not absolute: {}", helper.path).into());
        }
        if !paths.insert(&helper.path) {
            return Err(format!("duplicate helper path: {}", helper.path).into());
        }
        if !safe_relative_source(&helper.source) {
            return Err(format!(
                "helper source is not a safe relative path: {}",
                helper.source
            )
            .into());
        }
        if !sources.insert(&helper.source) {
            return Err(format!("duplicate helper source: {}", helper.source).into());
        }

        let copy = format!("COPY --chmod=0555 {} {}", helper.source, helper.path);
        if !dockerfile.lines().any(|line| line.trim() == copy) {
            return Err(format!("Dockerfile does not install {}", helper.path).into());
        }
        if !service.contains(&format!("\"{}\"", helper.path)) {
            return Err(format!("provisioning does not reference {}", helper.path).into());
        }

        if helper.source == GENERATED_MISE_SOURCE {
            continue;
        }
        let metadata = fs::symlink_metadata(root.join(&helper.source))?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(format!("helper source is not a regular file: {}", helper.source).into());
        }
    }
    Ok(())
}

fn safe_relative_source(source: &str) -> bool {
    !source.is_empty()
        && Path::new(source)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}
