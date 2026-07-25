use gascand::{SshPaths, ensure_host_identity};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::process::Command;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn root(temp: &TempDir) -> Result<std::path::PathBuf, std::io::Error> {
    temp.path().canonicalize()
}

fn paths(temp: &TempDir) -> Result<SshPaths, Box<dyn std::error::Error>> {
    let xdg = root(temp)?.join("xdg");
    Ok(SshPaths::for_environment(Some(xdg.as_os_str()), None)?)
}

#[test]
fn resolves_xdg_config_home_before_home_fallback() -> TestResult {
    let temp = TempDir::new()?;
    let base = root(&temp)?;
    let xdg = base.join("xdg");
    let home = base.join("home");

    let preferred = SshPaths::for_environment(Some(xdg.as_os_str()), Some(home.as_os_str()))?;
    assert_eq!(preferred.directory().as_std_path(), xdg.join("gascan/ssh"));

    let fallback = SshPaths::for_environment(None, Some(home.as_os_str()))?;
    assert_eq!(
        fallback.directory().as_std_path(),
        home.join(".config/gascan/ssh")
    );
    assert!(SshPaths::for_environment(None, None).is_err());
    assert!(
        SshPaths::for_environment(Some(OsStr::new("relative")), Some(home.as_os_str())).is_err()
    );
    Ok(())
}

#[tokio::test]
async fn generates_one_valid_ed25519_identity_with_exact_metadata() -> TestResult {
    let temp = TempDir::new()?;
    let paths = paths(&temp)?;

    let first = ensure_host_identity(&paths).await?;
    let private_before = fs::read(first.private_key.as_std_path())?;
    let second = ensure_host_identity(&paths).await?;

    assert_eq!(first, second);
    assert_eq!(fs::read(second.private_key.as_std_path())?, private_before);
    assert!(first.public_key.starts_with("ssh-ed25519 "));
    assert!(first.fingerprint.starts_with("SHA256:"));

    for directory in [
        paths.gascan_directory().as_std_path(),
        paths.directory().as_std_path(),
    ] {
        let metadata = fs::symlink_metadata(directory)?;
        assert!(metadata.is_dir());
        assert_eq!(metadata.uid(), rustix::process::geteuid().as_raw());
        assert_eq!(metadata.permissions().mode() & 0o7777, 0o700);
    }
    for (file, mode) in [
        (paths.private_key().as_std_path(), 0o600),
        (paths.public_key().as_std_path(), 0o644),
    ] {
        let metadata = fs::symlink_metadata(file)?;
        assert!(metadata.is_file());
        assert_eq!(metadata.uid(), rustix::process::geteuid().as_raw());
        assert_eq!(metadata.nlink(), 1);
        assert_eq!(metadata.permissions().mode() & 0o7777, mode);
    }
    Ok(())
}

#[tokio::test]
async fn rejects_symlink_ancestors_and_managed_key_paths() -> TestResult {
    let ancestor_temp = TempDir::new()?;
    let base = root(&ancestor_temp)?;
    let target = base.join("target");
    fs::create_dir(&target)?;
    let linked = base.join("linked");
    std::os::unix::fs::symlink(&target, &linked)?;
    let attacked = SshPaths::for_environment(Some(linked.as_os_str()), None)?;
    assert!(ensure_host_identity(&attacked).await.is_err());
    assert!(!target.join("gascan").exists());

    let file_temp = TempDir::new()?;
    let paths = paths(&file_temp)?;
    let identity = ensure_host_identity(&paths).await?;
    let public_key = paths.public_key().as_std_path();
    fs::remove_file(public_key)?;
    let victim = root(&file_temp)?.join("victim");
    fs::write(&victim, b"retain")?;
    std::os::unix::fs::symlink(&victim, public_key)?;
    assert!(ensure_host_identity(&paths).await.is_err());
    assert_eq!(fs::read(victim)?, b"retain");
    assert!(!identity.public_key.is_empty());
    Ok(())
}

#[tokio::test]
async fn rejects_non_sticky_world_writable_path_ancestors() -> TestResult {
    let temp = TempDir::new()?;
    let base = root(&temp)?;
    let unsafe_ancestor = base.join("unsafe");
    fs::create_dir(&unsafe_ancestor)?;
    fs::set_permissions(&unsafe_ancestor, fs::Permissions::from_mode(0o777))?;
    let config_home = unsafe_ancestor.join("xdg");
    let attacked = SshPaths::for_environment(Some(config_home.as_os_str()), None)?;
    assert!(ensure_host_identity(&attacked).await.is_err());
    assert!(!config_home.join("gascan").exists());
    Ok(())
}

#[tokio::test]
async fn rejects_hard_links_fifos_and_non_regular_keys() -> TestResult {
    let hard_link_temp = TempDir::new()?;
    let hard_link_paths = paths(&hard_link_temp)?;
    ensure_host_identity(&hard_link_paths).await?;
    let private_key = hard_link_paths.private_key().as_std_path();
    let backing = root(&hard_link_temp)?.join("backing");
    fs::rename(private_key, &backing)?;
    fs::hard_link(&backing, private_key)?;
    assert!(ensure_host_identity(&hard_link_paths).await.is_err());
    assert_eq!(fs::symlink_metadata(private_key)?.nlink(), 2);

    let fifo_temp = TempDir::new()?;
    let fifo_paths = paths(&fifo_temp)?;
    ensure_host_identity(&fifo_paths).await?;
    fs::remove_file(fifo_paths.private_key().as_std_path())?;
    let status = Command::new("/usr/bin/mkfifo")
        .arg(fifo_paths.private_key().as_std_path())
        .status()?;
    assert!(status.success());
    assert!(ensure_host_identity(&fifo_paths).await.is_err());

    let directory_temp = TempDir::new()?;
    let directory_paths = paths(&directory_temp)?;
    ensure_host_identity(&directory_paths).await?;
    fs::remove_file(directory_paths.public_key().as_std_path())?;
    fs::create_dir(directory_paths.public_key().as_std_path())?;
    assert!(ensure_host_identity(&directory_paths).await.is_err());
    Ok(())
}

#[tokio::test]
async fn rejects_unsafe_private_mode() -> TestResult {
    let mode_temp = TempDir::new()?;
    let mode_paths = paths(&mode_temp)?;
    ensure_host_identity(&mode_paths).await?;
    fs::set_permissions(
        mode_paths.private_key().as_std_path(),
        fs::Permissions::from_mode(0o644),
    )?;
    assert!(ensure_host_identity(&mode_paths).await.is_err());
    Ok(())
}

#[tokio::test]
async fn rejects_malformed_private_and_public_keys_without_disclosing_bytes() -> TestResult {
    let private_temp = TempDir::new()?;
    let private_paths = paths(&private_temp)?;
    ensure_host_identity(&private_paths).await?;
    let private_sentinel = b"PRIVATE-SENTINEL-CONTENT";
    fs::write(private_paths.private_key().as_std_path(), private_sentinel)?;
    fs::set_permissions(
        private_paths.private_key().as_std_path(),
        fs::Permissions::from_mode(0o600),
    )?;
    let error = ensure_host_identity(&private_paths)
        .await
        .expect_err("malformed private key must fail");
    let diagnostic = format!("{error:?} {error}");
    assert!(!diagnostic.contains(std::str::from_utf8(private_sentinel)?));

    let public_temp = TempDir::new()?;
    let public_paths = paths(&public_temp)?;
    ensure_host_identity(&public_paths).await?;
    fs::write(
        public_paths.public_key().as_std_path(),
        b"ssh-ed25519 malformed",
    )?;
    fs::set_permissions(
        public_paths.public_key().as_std_path(),
        fs::Permissions::from_mode(0o644),
    )?;
    assert!(ensure_host_identity(&public_paths).await.is_err());
    Ok(())
}
