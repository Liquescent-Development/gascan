use std::{fs, path::Path};

use gascan_image_tools::{
    ArtifactClass, DynError, RedirectRules, install_bounded_artifact, install_verified_artifact,
    open_with_redirect_rules, validate_cached_artifact,
};

fn main() -> Result<(), DynError> {
    let mut args = std::env::args().skip(1);
    let class = artifact_class(&args.next().ok_or("missing artifact class")?)?;
    let url = args.next().ok_or("missing artifact URL")?;
    let expected = args.next().ok_or("missing artifact SHA-256")?;
    let destination = args.next().ok_or("missing artifact destination")?;
    let exact_size = args.next().map(|value| value.parse::<u64>()).transpose()?;
    if args.next().is_some() {
        return Err("unexpected artifact downloader argument".into());
    }
    let destination = Path::new(&destination);
    let redirect_rules = RedirectRules::for_artifact(class);
    redirect_rules.require_initial_url(&url)?;

    if let Ok(metadata) = fs::symlink_metadata(destination) {
        if metadata.file_type().is_file() && metadata.len() <= class.maximum_bytes() {
            let expected_size = exact_size.unwrap_or(metadata.len());
            if validate_cached_artifact(destination, &expected, expected_size).is_ok() {
                eprintln!(
                    "Revalidated cached image artifact: {}",
                    destination.display()
                );
                return Ok(());
            }
        }
    }

    let response = open_with_redirect_rules(&url, redirect_rules)?;
    if let Some(length) = response.content_length() {
        let limit = exact_size.unwrap_or(class.maximum_bytes());
        if length > limit || exact_size.is_some_and(|size| length != size) {
            return Err("artifact HTTP content length violates the size limit".into());
        }
    }
    if let Some(size) = exact_size {
        return install_verified_artifact(response, destination, &expected, size, class);
    }
    install_bounded_artifact(response, destination, &expected, class)
}

fn artifact_class(value: &str) -> Result<ArtifactClass, DynError> {
    match value {
        "mise" => Ok(ArtifactClass::Mise),
        "chromium" => Ok(ArtifactClass::Chromium),
        "workspace-bundle" => Ok(ArtifactClass::WorkspaceBundle),
        _ => Err(format!("unknown artifact class: {value}").into()),
    }
}
