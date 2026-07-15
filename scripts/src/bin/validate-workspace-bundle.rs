use std::{env, fs, path::PathBuf, process::ExitCode};

use gascan_image_tools::bundle::{PublishedBundleLocks, validate_bundle};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("validate-workspace-bundle: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os().skip(1);
    let lock_path = required(&mut arguments, "LOCK_FILE")?;
    let record = required(&mut arguments, "RECORD_NAME")?;
    let archive = required(&mut arguments, "ARCHIVE")?;
    let destination = required(&mut arguments, "DESTINATION")?;
    if arguments.next().is_some() {
        return Err(
            "usage: validate-workspace-bundle LOCK_FILE RECORD_NAME ARCHIVE DESTINATION".into(),
        );
    }
    let record = record.to_str().ok_or("RECORD_NAME must be valid UTF-8")?;
    let locks = PublishedBundleLocks::from_toml(&fs::read_to_string(lock_path)?)?;
    let evidence = validate_bundle(locks.named(record)?, &archive, &destination)?;
    println!(
        "sha256={} size={} entries={} regular_files={}",
        evidence.archive_sha256, evidence.archive_size, evidence.entries, evidence.regular_files
    );
    Ok(())
}

fn required(
    arguments: &mut impl Iterator<Item = std::ffi::OsString>,
    name: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    arguments
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("missing {name}").into())
}
