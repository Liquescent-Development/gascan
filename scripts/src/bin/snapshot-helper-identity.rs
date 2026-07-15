use std::{
    env, error::Error, fs, io::Read, os::unix::fs::MetadataExt, path::Path, process::ExitCode,
};

use sha2::{Digest, Sha256};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("snapshot-helper-identity: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let path = env::args_os().nth(1).ok_or("missing fixed helper path")?;
    if env::args_os().nth(2).is_some() {
        return Err("unexpected argument".into());
    }
    let (sha, dev, ino) = validate(Path::new(&path))?;
    println!("{sha}\t{dev}\t{ino}");
    Ok(())
}

fn validate(path: &Path) -> Result<(String, u64, u64), Box<dyn Error>> {
    validate_from(path, Path::new("/"), 0, 0)
}

fn validate_from(
    path: &Path,
    trusted_root: &Path,
    uid: u32,
    gid: u32,
) -> Result<(String, u64, u64), Box<dyn Error>> {
    if !path.is_absolute() {
        return Err("helper path must be absolute".into());
    }
    if !path.starts_with(trusted_root) {
        return Err("helper escapes trusted root".into());
    }
    let mut current = trusted_root.to_path_buf();
    for component in path
        .parent()
        .ok_or("helper has no parent")?
        .strip_prefix(trusted_root)?
        .components()
    {
        current.push(component);
        let metadata = fs::symlink_metadata(&current)?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || metadata.uid() != uid
            || metadata.mode() & 0o022 != 0
        {
            return Err("helper ancestry must be real root-owned non-writable directories".into());
        }
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.uid() != uid
        || metadata.gid() != gid
        || metadata.mode() & 0o7777 != 0o555
    {
        return Err("helper must be a root:wheel regular non-symlink mode-0555 file".into());
    }
    let mut options = fs::OpenOptions::new();
    use std::os::unix::fs::OpenOptionsExt;
    options.read(true).custom_flags(libc::O_NOFOLLOW);
    let mut file = options.open(path)?;
    let opened = file.metadata()?;
    if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
        return Err("helper changed while opening".into());
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok((
        format!("{:x}", hasher.finalize()),
        opened.dev(),
        opened.ino(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn writable_parent_and_symlink_are_rejected_before_identity_output() {
        let temporary = tempfile::tempdir().unwrap();
        let helper = temporary.path().join("helper");
        fs::write(&helper, b"helper").unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o555)).unwrap();
        assert!(validate(&helper).is_err());
        let link = temporary.path().join("link");
        symlink(&helper, &link).unwrap();
        assert!(validate(&link).is_err());
    }

    #[test]
    fn helper_replacement_cannot_reuse_expected_digest() {
        let temporary = tempfile::tempdir().unwrap();
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let identity = fs::symlink_metadata(temporary.path()).unwrap();
        let helper = temporary.path().join("helper");
        fs::write(&helper, b"first").unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o555)).unwrap();
        let expected =
            validate_from(&helper, temporary.path(), identity.uid(), identity.gid()).unwrap();
        let replacement = temporary.path().join("replacement");
        fs::write(&replacement, b"second").unwrap();
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o555)).unwrap();
        fs::rename(&replacement, &helper).unwrap();
        let actual =
            validate_from(&helper, temporary.path(), identity.uid(), identity.gid()).unwrap();
        assert_ne!(actual, expected);
    }
}
