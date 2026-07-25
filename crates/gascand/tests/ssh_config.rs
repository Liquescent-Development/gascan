use gascand::{
    ActiveSsh, ManagedSshHost, SshPaths, ensure_host_identity, prepare_openssh_files,
    publish_openssh_files, readiness_ssh_args,
};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs;
use std::net::{IpAddr, Ipv4Addr};
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

fn host(alias: &str, port: u16, identity: &gascand::HostIdentity) -> ManagedSshHost {
    ManagedSshHost {
        active: ActiveSsh {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port,
            alias: alias.to_owned(),
            host_key_fingerprint: identity.fingerprint().to_owned(),
            client_key_fingerprint: identity.fingerprint().to_owned(),
        },
        host_public_key: identity.public_key().to_owned(),
    }
}

fn configured_known_hosts(config: &str) -> Result<&str, Box<dyn std::error::Error>> {
    config
        .lines()
        .find_map(|line| line.trim().strip_prefix("UserKnownHostsFile "))
        .ok_or_else(|| "generated config does not name known-hosts".into())
}

#[tokio::test]
async fn publishes_stable_sorted_strict_openssh_files() -> TestResult {
    let temp = TempDir::new()?;
    let paths = paths(&temp)?;
    let identity = ensure_host_identity(&paths).await?;
    let hosts = [
        host("gascan-zeta", 2222, &identity),
        host("gascan-alpha", 2201, &identity),
    ];

    publish_openssh_files(&paths, &identity, &hosts)?;
    let config_before = fs::read_to_string(paths.config().as_std_path())?;
    let known_hosts_path = configured_known_hosts(&config_before)?;
    let known_hosts_before = fs::read_to_string(known_hosts_path)?;
    publish_openssh_files(&paths, &identity, &hosts)?;
    assert_eq!(
        fs::read_to_string(paths.config().as_std_path())?,
        config_before
    );
    assert_eq!(fs::read_to_string(known_hosts_path)?, known_hosts_before);
    let generation = std::path::Path::new(known_hosts_path)
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or("known-hosts generation is not UTF-8")?;
    let expected_generation = Sha256::digest(known_hosts_before.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(generation, format!("known_hosts.{expected_generation}"));

    assert!(config_before.find("Host gascan-alpha") < config_before.find("Host gascan-zeta"));
    for required in [
        "HostName 127.0.0.1",
        "User workspace",
        "IdentitiesOnly yes",
        "StrictHostKeyChecking yes",
        "ForwardAgent no",
        "HostKeyAlias gascan-alpha",
        &format!("IdentityFile {}", paths.private_key()),
        &format!("UserKnownHostsFile {known_hosts_path}"),
    ] {
        assert!(
            config_before.contains(required),
            "missing required directive: {required}"
        );
    }
    assert!(!config_before.contains("ClearAllForwardings"));
    assert!(!config_before.contains("StrictHostKeyChecking no"));
    assert!(known_hosts_before.lines().eq([
        format!(
            "gascan-alpha,[127.0.0.1]:2201 {}",
            identity
                .public_key()
                .split_whitespace()
                .take(2)
                .collect::<Vec<_>>()
                .join(" ")
        ),
        format!(
            "gascan-zeta,[127.0.0.1]:2222 {}",
            identity
                .public_key()
                .split_whitespace()
                .take(2)
                .collect::<Vec<_>>()
                .join(" ")
        ),
    ]));

    let private_bytes = fs::read(paths.private_key().as_std_path())?;
    let generated = [config_before.as_bytes(), known_hosts_before.as_bytes()].concat();
    assert!(
        !generated
            .windows(private_bytes.len())
            .any(|window| window == private_bytes)
    );
    for file in [
        paths.config().as_std_path(),
        std::path::Path::new(known_hosts_path),
    ] {
        let metadata = fs::symlink_metadata(file)?;
        assert!(metadata.is_file());
        assert_eq!(metadata.uid(), rustix::process::geteuid().as_raw());
        assert_eq!(metadata.nlink(), 1);
        assert_eq!(metadata.permissions().mode() & 0o7777, 0o644);
    }
    Ok(())
}

#[tokio::test]
async fn readiness_args_are_discrete_and_do_not_weaken_reusable_config() -> TestResult {
    let temp = TempDir::new()?;
    let paths = paths(&temp)?;
    let identity = ensure_host_identity(&paths).await?;
    let ready = host("gascan-ready", 2222, &identity);
    let prepared = prepare_openssh_files(&paths, &identity, std::slice::from_ref(&ready))?;
    let generation_known_hosts = prepared.known_hosts().to_owned();
    assert_eq!(
        generation_known_hosts.file_name(),
        Some(prepared.generation())
    );
    assert!(generation_known_hosts.exists());
    assert!(!paths.config().exists());

    assert_eq!(
        readiness_ssh_args(&paths, &identity, &ready, &generation_known_hosts)?,
        vec![
            OsString::from("-o"),
            OsString::from("HostName=127.0.0.1"),
            OsString::from("-o"),
            OsString::from("Port=2222"),
            OsString::from("-o"),
            OsString::from("User=workspace"),
            OsString::from("-o"),
            OsString::from(format!("IdentityFile={}", paths.private_key())),
            OsString::from("-o"),
            OsString::from("HostKeyAlias=gascan-ready"),
            OsString::from("-o"),
            OsString::from(format!("UserKnownHostsFile={generation_known_hosts}")),
            OsString::from("-o"),
            OsString::from("StrictHostKeyChecking=yes"),
            OsString::from("-o"),
            OsString::from("IdentitiesOnly=yes"),
            OsString::from("-o"),
            OsString::from("BatchMode=yes"),
            OsString::from("-o"),
            OsString::from("ForwardAgent=no"),
            OsString::from("-o"),
            OsString::from("ClearAllForwardings=yes"),
            OsString::from("127.0.0.1"),
            OsString::from("/usr/bin/true"),
        ]
    );
    assert!(!paths.config().exists());
    Ok(())
}

#[tokio::test]
async fn rejects_invalid_or_inconsistent_active_hosts_before_replacement() -> TestResult {
    let temp = TempDir::new()?;
    let paths = paths(&temp)?;
    let identity = ensure_host_identity(&paths).await?;
    publish_openssh_files(&paths, &identity, &[host("gascan-valid", 2222, &identity)])?;
    let config_before = fs::read(paths.config().as_std_path())?;
    let config_text = std::str::from_utf8(&config_before)?;
    let known_hosts_path = configured_known_hosts(config_text)?;
    let known_hosts_before = fs::read(known_hosts_path)?;

    let mut attacks = Vec::new();
    let mut non_loopback = host("gascan-other", 2223, &identity);
    non_loopback.active.host = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));
    attacks.push(non_loopback);
    attacks.push(host("gascan-bad\nHost *", 2223, &identity));
    attacks.push(host("gascan-zero", 0, &identity));
    let mut client_mismatch = host("gascan-client", 2223, &identity);
    client_mismatch.active.client_key_fingerprint = "SHA256:wrong".to_owned();
    attacks.push(client_mismatch);
    let mut host_mismatch = host("gascan-host", 2223, &identity);
    host_mismatch.active.host_key_fingerprint = "SHA256:wrong".to_owned();
    attacks.push(host_mismatch);

    for attack in attacks {
        assert!(publish_openssh_files(&paths, &identity, &[attack]).is_err());
        assert_eq!(fs::read(paths.config().as_std_path())?, config_before);
        assert_eq!(fs::read(known_hosts_path)?, known_hosts_before);
    }
    let duplicate = host("gascan-duplicate", 2224, &identity);
    assert!(publish_openssh_files(&paths, &identity, &[duplicate.clone(), duplicate]).is_err());
    Ok(())
}

#[tokio::test]
async fn rejected_config_target_cannot_change_the_active_trust_generation() -> TestResult {
    let temp = TempDir::new()?;
    let paths = paths(&temp)?;
    let identity = ensure_host_identity(&paths).await?;
    publish_openssh_files(&paths, &identity, &[host("gascan-before", 2222, &identity)])?;
    let config_before = fs::read_to_string(paths.config().as_std_path())?;
    let known_hosts_path = configured_known_hosts(&config_before)?.to_owned();
    let known_hosts_before = fs::read(&known_hosts_path)?;
    fs::hard_link(
        paths.config().as_std_path(),
        paths.directory().join("config-link").as_std_path(),
    )?;

    assert!(
        publish_openssh_files(&paths, &identity, &[host("gascan-after", 2223, &identity)]).is_err()
    );
    assert_eq!(
        fs::read_to_string(paths.config().as_std_path())?,
        config_before
    );
    assert_eq!(fs::read(known_hosts_path)?, known_hosts_before);
    Ok(())
}

#[tokio::test]
async fn publication_reloads_and_revalidates_the_managed_identity_pair() -> TestResult {
    let managed_temp = TempDir::new()?;
    let managed_paths = paths(&managed_temp)?;
    let stale_identity = ensure_host_identity(&managed_paths).await?;
    let stale_host = host("gascan-stale", 2222, &stale_identity);

    let replacement_temp = TempDir::new()?;
    let replacement_paths = paths(&replacement_temp)?;
    ensure_host_identity(&replacement_paths).await?;
    fs::copy(
        replacement_paths.private_key().as_std_path(),
        managed_paths.private_key().as_std_path(),
    )?;
    fs::set_permissions(
        managed_paths.private_key().as_std_path(),
        fs::Permissions::from_mode(0o600),
    )?;
    fs::copy(
        replacement_paths.public_key().as_std_path(),
        managed_paths.public_key().as_std_path(),
    )?;
    fs::set_permissions(
        managed_paths.public_key().as_std_path(),
        fs::Permissions::from_mode(0o644),
    )?;

    assert!(publish_openssh_files(&managed_paths, &stale_identity, &[stale_host]).is_err());
    assert!(!managed_paths.config().exists());
    Ok(())
}

#[tokio::test]
async fn rejects_symlink_hard_link_fifo_and_unsafe_generated_targets() -> TestResult {
    let symlink_temp = TempDir::new()?;
    let symlink_paths = paths(&symlink_temp)?;
    let symlink_identity = ensure_host_identity(&symlink_paths).await?;
    publish_openssh_files(
        &symlink_paths,
        &symlink_identity,
        &[host("gascan-link", 2222, &symlink_identity)],
    )?;
    fs::remove_file(symlink_paths.config().as_std_path())?;
    let victim = root(&symlink_temp)?.join("victim");
    fs::write(&victim, b"retain")?;
    std::os::unix::fs::symlink(&victim, symlink_paths.config().as_std_path())?;
    assert!(
        publish_openssh_files(
            &symlink_paths,
            &symlink_identity,
            &[host("gascan-link", 2222, &symlink_identity)],
        )
        .is_err()
    );
    assert_eq!(fs::read(victim)?, b"retain");

    let hard_link_temp = TempDir::new()?;
    let hard_link_paths = paths(&hard_link_temp)?;
    let hard_link_identity = ensure_host_identity(&hard_link_paths).await?;
    publish_openssh_files(
        &hard_link_paths,
        &hard_link_identity,
        &[host("gascan-hard", 2222, &hard_link_identity)],
    )?;
    let config = fs::read_to_string(hard_link_paths.config().as_std_path())?;
    let known_hosts = std::path::Path::new(configured_known_hosts(&config)?);
    let backing = root(&hard_link_temp)?.join("backing");
    fs::rename(known_hosts, &backing)?;
    fs::hard_link(&backing, known_hosts)?;
    assert!(
        publish_openssh_files(
            &hard_link_paths,
            &hard_link_identity,
            &[host("gascan-hard", 2222, &hard_link_identity)],
        )
        .is_err()
    );
    assert_eq!(fs::symlink_metadata(known_hosts)?.nlink(), 2);

    let fifo_temp = TempDir::new()?;
    let fifo_paths = paths(&fifo_temp)?;
    let fifo_identity = ensure_host_identity(&fifo_paths).await?;
    publish_openssh_files(
        &fifo_paths,
        &fifo_identity,
        &[host("gascan-fifo", 2222, &fifo_identity)],
    )?;
    fs::remove_file(fifo_paths.config().as_std_path())?;
    let status = Command::new("/usr/bin/mkfifo")
        .arg(fifo_paths.config().as_std_path())
        .status()?;
    assert!(status.success());
    assert!(
        publish_openssh_files(
            &fifo_paths,
            &fifo_identity,
            &[host("gascan-fifo", 2222, &fifo_identity)],
        )
        .is_err()
    );

    let mode_temp = TempDir::new()?;
    let mode_paths = paths(&mode_temp)?;
    let mode_identity = ensure_host_identity(&mode_paths).await?;
    publish_openssh_files(
        &mode_paths,
        &mode_identity,
        &[host("gascan-mode", 2222, &mode_identity)],
    )?;
    let config = fs::read_to_string(mode_paths.config().as_std_path())?;
    let known_hosts = std::path::Path::new(configured_known_hosts(&config)?);
    fs::set_permissions(known_hosts, fs::Permissions::from_mode(0o666))?;
    assert!(
        publish_openssh_files(
            &mode_paths,
            &mode_identity,
            &[host("gascan-mode", 2222, &mode_identity)],
        )
        .is_err()
    );
    Ok(())
}
