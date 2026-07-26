use nix::unistd::{Uid, User};
use std::io;
use std::path::{Component, PathBuf};

/// Resolve the effective account's home through the system account database.
///
/// OpenSSH expands `~` from this account record rather than from the mutable
/// `HOME` environment variable, so every production SSH path must share this
/// authority.
pub fn effective_account_home() -> io::Result<PathBuf> {
    let uid = Uid::effective();
    let user = User::from_uid(uid)
        .map_err(io::Error::from)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "effective account is missing"))?;
    let home = user.dir;
    if !home.is_absolute()
        || home
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "effective account home is not absolute and normalized",
        ));
    }
    Ok(home)
}
