use std::{
    error::Error,
    fs,
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt},
        process::ExitStatusExt,
    },
    path::Path,
    process::{Command, ExitCode},
};

type DynError = Box<dyn Error + Send + Sync>;

fn main() -> Result<ExitCode, DynError> {
    let mut arguments = std::env::args_os().skip(1);
    let lock_path = arguments.next().ok_or("missing lock path")?;
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new("--")) {
        return Err("missing lock command separator".into());
    }
    let command = arguments.next().ok_or("missing lock command")?;
    let command_arguments: Vec<_> = arguments.collect();

    let _lock = open_lock(Path::new(&lock_path))?;
    let status = Command::new(command).args(command_arguments).status()?;
    Ok(match status.code() {
        Some(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        None => ExitCode::from(
            status
                .signal()
                .and_then(|signal| u8::try_from(128 + signal).ok())
                .unwrap_or(1),
        ),
    })
}

fn open_lock(path: &Path) -> Result<fs::File, DynError> {
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)?;
    validate_metadata(&lock.metadata()?)?;
    match rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => {}
        Err(error) if error == rustix::io::Errno::WOULDBLOCK => {
            if let Some(marker) = std::env::var_os("GASCAN_SAFE_LOCK_TEST_WAITING_FILE") {
                let marker = Path::new(&marker);
                let mut options = fs::OpenOptions::new();
                options.write(true).create_new(true).mode(0o600);
                options.open(marker)?.sync_all()?;
            }
            rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive)?;
        }
        Err(error) => return Err(error.into()),
    }
    let descriptor_metadata = lock.metadata()?;
    validate_metadata(&descriptor_metadata)?;
    let path_metadata = fs::symlink_metadata(path)?;
    validate_metadata(&path_metadata)?;
    if path_metadata.dev() != descriptor_metadata.dev()
        || path_metadata.ino() != descriptor_metadata.ino()
    {
        return Err("lock path changed while acquiring the lock".into());
    }
    Ok(lock)
}

fn validate_metadata(metadata: &fs::Metadata) -> Result<(), DynError> {
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.mode() & 0o777 != 0o600
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.nlink() != 1
    {
        return Err("lock is not an ownership-validated, safely permissioned regular file".into());
    }
    Ok(())
}
