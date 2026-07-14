use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

use gascan_image_tools::{DynError, RedirectRules, open_with_redirect_rules};
use sha2::{Digest, Sha256};

fn main() -> Result<(), DynError> {
    let mut args = std::env::args().skip(1);
    let url = args.next().ok_or("missing artifact URL")?;
    let expected = args.next().ok_or("missing artifact SHA-256")?;
    let destination = args.next().ok_or("missing artifact destination")?;
    if args.next().is_some() {
        return Err("unexpected artifact downloader argument".into());
    }
    if !lower_hex(&expected, 64) {
        return Err("artifact SHA-256 must be 64 lowercase hexadecimal characters".into());
    }

    eprintln!("Downloading verified image artifact: {url}");
    let mut response = open_with_redirect_rules(&url, RedirectRules::image_artifacts())?;
    let destination = Path::new(&destination);
    let parent = destination
        .parent()
        .ok_or("artifact destination has no parent")?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut downloaded = 0_u64;
    let mut next_progress = 8 * 1024 * 1024;
    loop {
        let count = response.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        temporary.write_all(&buffer[..count])?;
        hasher.update(&buffer[..count]);
        downloaded += count as u64;
        if downloaded >= next_progress {
            eprintln!("Downloaded {downloaded} bytes");
            next_progress += 8 * 1024 * 1024;
        }
    }
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        return Err(format!("SHA-256 mismatch for {url}").into());
    }
    temporary.as_file_mut().sync_all()?;
    temporary.persist(destination)?;
    eprintln!("Verified {downloaded} bytes: {}", destination.display());
    Ok(())
}

fn lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
