use gascand::{PeerUid, SocketPaths, validate_peer_uid};
use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
#[cfg(target_os = "macos")]
fn default_runtime_directory_avoids_the_tmp_symlink() -> TestResult {
    let paths = SocketPaths::for_user_with_uid_and_environment(501, None)?;
    assert_eq!(
        paths.directory(),
        std::path::Path::new("/private/tmp/gascan-501")
    );
    Ok(())
}

fn root(temp: &TempDir) -> Result<PathBuf, std::io::Error> {
    temp.path().canonicalize()
}

#[test]
fn creates_exact_private_directory_and_socket_modes() -> TestResult {
    let temp = TempDir::new()?;
    let paths = SocketPaths::from_runtime_root(root(&temp)?.join("runtime"));
    let listener = paths.bind()?;
    let directory = fs::symlink_metadata(paths.directory())?;
    let socket = fs::symlink_metadata(paths.socket())?;
    assert_eq!(directory.permissions().mode() & 0o777, 0o700);
    assert_eq!(socket.permissions().mode() & 0o777, 0o600);
    assert!(socket.file_type().is_socket());
    drop(listener);
    Ok(())
}

#[test]
fn rejects_symlink_runtime_directory_and_socket_path() -> TestResult {
    let temp = TempDir::new()?;
    let target = root(&temp)?.join("target");
    fs::create_dir(&target)?;
    let linked_root = root(&temp)?.join("linked");
    std::os::unix::fs::symlink(&target, &linked_root)?;
    assert!(SocketPaths::from_runtime_root(linked_root).bind().is_err());

    let paths = SocketPaths::from_runtime_root(root(&temp)?.join("runtime"));
    fs::create_dir(paths.directory())?;
    fs::set_permissions(paths.directory(), fs::Permissions::from_mode(0o700))?;
    std::os::unix::fs::symlink(root(&temp)?.join("elsewhere"), paths.socket())?;
    assert!(paths.bind().is_err());
    Ok(())
}

#[test]
fn rejects_an_intermediate_symlink_component() -> TestResult {
    let temp = TempDir::new()?;
    let target = root(&temp)?.join("target");
    fs::create_dir(&target)?;
    let intermediate = root(&temp)?.join("intermediate");
    std::os::unix::fs::symlink(&target, &intermediate)?;
    let paths = SocketPaths::from_runtime_root(intermediate.join("runtime"));
    assert!(paths.bind().is_err());
    assert!(!target.join("runtime").exists());
    Ok(())
}

#[test]
fn rejects_existing_runtime_directory_with_non_private_mode() -> TestResult {
    let temp = TempDir::new()?;
    let paths = SocketPaths::from_runtime_root(root(&temp)?.join("runtime"));
    fs::create_dir(paths.directory())?;
    fs::set_permissions(paths.directory(), fs::Permissions::from_mode(0o755))?;
    assert!(paths.bind().is_err());
    Ok(())
}

#[test]
fn refuses_live_socket_and_arbitrary_file_but_replaces_stale_owned_socket() -> TestResult {
    let temp = TempDir::new()?;
    let live_paths = SocketPaths::from_runtime_root(root(&temp)?.join("live"));
    let live = live_paths.bind()?;
    assert!(live_paths.bind().is_err());
    drop(live);

    let file_paths = SocketPaths::from_runtime_root(root(&temp)?.join("file"));
    fs::create_dir(file_paths.directory())?;
    fs::set_permissions(file_paths.directory(), fs::Permissions::from_mode(0o700))?;
    fs::write(file_paths.socket(), b"do not delete")?;
    assert!(file_paths.bind().is_err());
    assert_eq!(fs::read(file_paths.socket())?, b"do not delete");

    let stale_paths = SocketPaths::from_runtime_root(root(&temp)?.join("stale"));
    fs::create_dir(stale_paths.directory())?;
    fs::set_permissions(stale_paths.directory(), fs::Permissions::from_mode(0o700))?;
    let stale = UnixListener::bind(stale_paths.socket())?;
    drop(stale);
    let replacement = stale_paths.bind()?;
    assert_eq!(
        fs::symlink_metadata(stale_paths.socket())?.uid(),
        rustix::process::geteuid().as_raw()
    );
    drop(replacement);
    Ok(())
}

#[test]
fn peer_uid_validator_requires_exact_effective_uid() {
    assert!(validate_peer_uid(PeerUid::new(501), PeerUid::new(501)).is_ok());
    assert!(validate_peer_uid(PeerUid::new(502), PeerUid::new(501)).is_err());
}

#[test]
fn cleanup_preserves_a_replacement_at_the_socket_path() -> TestResult {
    let temp = TempDir::new()?;
    let paths = SocketPaths::from_runtime_root(root(&temp)?.join("runtime"));
    let owned = paths.bind()?;
    fs::remove_file(paths.socket())?;
    fs::write(paths.socket(), b"replacement")?;
    drop(owned);
    assert_eq!(fs::read(paths.socket())?, b"replacement");
    Ok(())
}

#[test]
fn cleanup_is_anchored_to_the_open_directory_after_path_substitution() -> TestResult {
    let temp = TempDir::new()?;
    let paths = SocketPaths::from_runtime_root(root(&temp)?.join("runtime"));
    let owned = paths.bind()?;
    let displaced = root(&temp)?.join("displaced");
    fs::rename(paths.directory(), &displaced)?;
    fs::create_dir(paths.directory())?;
    fs::set_permissions(paths.directory(), fs::Permissions::from_mode(0o700))?;
    drop(owned);
    assert!(!displaced.join("gascand.sock").exists());
    Ok(())
}
