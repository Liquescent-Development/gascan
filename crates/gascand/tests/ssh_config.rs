#[cfg(debug_assertions)]
use camino::Utf8Path;
#[cfg(debug_assertions)]
use gascan_core::fake_runtime::FakeRuntime;
#[cfg(debug_assertions)]
use gascan_core::manifest::Manifest;
use gascan_core::sandbox::SandboxId;
#[cfg(debug_assertions)]
use gascan_core::sandbox::SandboxSpec;
use gascand::{
    ActiveSsh, ManagedSshHost, SshManager, SshPaths, SshResolution, ensure_host_identity,
    prepare_openssh_files, publish_openssh_files, readiness_ssh_args,
};
#[cfg(debug_assertions)]
use gascand::{NoopProvisioner, SandboxService, SshReadinessPolicy, Store, UpRequest};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs;
use std::net::{IpAddr, Ipv4Addr};
#[cfg(debug_assertions)]
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::process::Command;
#[cfg(debug_assertions)]
use std::sync::Arc;
#[cfg(debug_assertions)]
use std::time::Duration;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn root(temp: &TempDir) -> Result<std::path::PathBuf, std::io::Error> {
    temp.path().canonicalize()
}

fn paths(temp: &TempDir) -> Result<SshPaths, Box<dyn std::error::Error>> {
    let home = root(temp)?;
    Ok(SshPaths::for_environment(None, Some(home.as_os_str()))?)
}

#[cfg(debug_assertions)]
async fn booted_readiness_context(
    temp: &TempDir,
) -> Result<(FakeRuntime, SandboxId, SshResolution, SshPaths), Box<dyn std::error::Error>> {
    let root = Utf8Path::from_path(temp.path()).ok_or("temporary root is not UTF-8")?;
    fs::write(
        root.join("gascan.toml"),
        "version = 1\nnetwork = 'networked'\n[ssh]\nhost_port = 24242\n",
    )?;
    let spec = SandboxSpec::from_root("readiness", root, Manifest::load(root)?)?;
    let ssh_paths = paths(temp)?;
    let host = TempDir::new()?;
    let host_key = ensure_host_identity(&paths(&host)?)
        .await?
        .public_key()
        .to_owned();
    let runtime = FakeRuntime::default();
    runtime
        .set_exec_result(format!("{host_key}\n").into_bytes(), Vec::new(), 0)
        .await;
    runtime.queue_created_ssh_host_port(24_242).await;
    let service = SandboxService::new_with_ssh_for_tests(
        runtime.clone(),
        Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
        ssh_paths.clone(),
        "/usr/bin/true".into(),
    );
    service.up(UpRequest::new(spec.clone())).await?;
    let resolution = service
        .status(spec.id())?
        .and_then(|record| record.ssh_resolution)
        .ok_or("booted sandbox has no SSH resolution")?;
    Ok((runtime, spec.id().clone(), resolution, ssh_paths))
}

#[cfg(debug_assertions)]
fn executable_script(
    root: &Utf8Path,
    name: &str,
    body: &str,
) -> Result<camino::Utf8PathBuf, Box<dyn std::error::Error>> {
    let path = root.join(name);
    fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n"))?;
    fs::set_permissions(path.as_std_path(), fs::Permissions::from_mode(0o755))?;
    Ok(path)
}

#[cfg(debug_assertions)]
fn strict_readiness_argv(paths: &SshPaths, id: &SandboxId, known_hosts: &str) -> Vec<OsString> {
    vec![
        "-F".into(),
        "/dev/null".into(),
        "-o".into(),
        "HostName=127.0.0.1".into(),
        "-o".into(),
        "Port=24242".into(),
        "-o".into(),
        "User=workspace".into(),
        "-o".into(),
        format!("IdentityFile={}", paths.private_key()).into(),
        "-o".into(),
        format!("HostKeyAlias=gascan-{id}").into(),
        "-o".into(),
        format!("UserKnownHostsFile={known_hosts}").into(),
        "-o".into(),
        "StrictHostKeyChecking=yes".into(),
        "-o".into(),
        "IdentitiesOnly=yes".into(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "ForwardAgent=no".into(),
        "-o".into(),
        "ClearAllForwardings=yes".into(),
        "127.0.0.1".into(),
        "/usr/bin/true".into(),
    ]
}

#[cfg(debug_assertions)]
fn nul_terminated_args(args: &[OsString]) -> Vec<u8> {
    args.iter()
        .flat_map(|argument| argument.as_os_str().as_bytes().iter().copied().chain([0]))
        .collect()
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

fn write_generation(
    paths: &SshPaths,
    contents: &str,
) -> Result<camino::Utf8PathBuf, Box<dyn std::error::Error>> {
    let digest = Sha256::digest(contents.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let path = paths.directory().join(format!("known_hosts.{digest}"));
    fs::write(path.as_std_path(), contents)?;
    fs::set_permissions(path.as_std_path(), fs::Permissions::from_mode(0o644))?;
    Ok(path)
}

fn hashed_host_record(alias: &str, public_key: &str) -> Result<String, Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let known_hosts = temp.path().join("known_hosts");
    fs::write(&known_hosts, format!("{alias} {public_key}\n"))?;
    let hashed = Command::new("/usr/bin/ssh-keygen")
        .args(["-q", "-H", "-f"])
        .arg(&known_hosts)
        .output()?;
    if !hashed.status.success() {
        return Err("ssh-keygen could not hash the hostile known-host record".into());
    }
    let lookup = Command::new("/usr/bin/ssh-keygen")
        .args(["-F", alias, "-f"])
        .arg(&known_hosts)
        .output()?;
    if !lookup.status.success() {
        return Err("hashed hostile known-host record does not match its alias".into());
    }
    let record = fs::read_to_string(known_hosts)?
        .lines()
        .next()
        .ok_or("ssh-keygen produced an empty hashed known-host file")?
        .to_owned();
    if !record.starts_with("|1|") {
        return Err("ssh-keygen did not produce a hashed known-host record".into());
    }
    Ok(record)
}

fn require_matching_host_record(
    lookup: &str,
    record: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let known_hosts = temp.path().join("known_hosts");
    fs::write(&known_hosts, format!("{record}\n"))?;
    let matched = Command::new("/usr/bin/ssh-keygen")
        .args(["-F", lookup, "-f"])
        .arg(&known_hosts)
        .output()?;
    let output = String::from_utf8(matched.stdout)?;
    if !matched.status.success() || !output.lines().any(|line| line == record) {
        return Err(format!("hostile known-host record did not match {lookup}").into());
    }
    Ok(())
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
async fn successful_publication_preserves_the_previous_known_hosts_generation_for_readers()
-> TestResult {
    let temp = TempDir::new()?;
    let paths = paths(&temp)?;
    let identity = ensure_host_identity(&paths).await?;
    publish_openssh_files(&paths, &identity, &[host("gascan-before", 2222, &identity)])?;
    let previous_config = fs::read_to_string(paths.config().as_std_path())?;
    let previous_generation = std::path::PathBuf::from(configured_known_hosts(&previous_config)?);
    let previous_contents = fs::read(&previous_generation)?;
    assert!(previous_generation.exists());

    publish_openssh_files(&paths, &identity, &[host("gascan-after", 2223, &identity)])?;
    let current_config = fs::read_to_string(paths.config().as_std_path())?;
    let current_generation = std::path::PathBuf::from(configured_known_hosts(&current_config)?);

    assert_ne!(current_generation, previous_generation);
    assert!(current_generation.exists());
    assert_eq!(fs::read(previous_generation)?, previous_contents);
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

    let args = readiness_ssh_args(&paths, &identity, &ready, &generation_known_hosts).await?;
    assert_eq!(
        args,
        vec![
            OsString::from("-F"),
            OsString::from("/dev/null"),
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

    let hostile_home = root(&temp)?.join("hostile-home");
    let hostile_ssh = hostile_home.join(".ssh");
    fs::create_dir_all(&hostile_ssh)?;
    let hostile_identity = hostile_home.join("hostile-identity");
    fs::write(&hostile_identity, b"hostile")?;
    let included = hostile_home.join("system-like.conf");
    fs::write(
        &included,
        format!(
            "Host *\n    ProxyCommand /usr/bin/false hostile-proxy\n    ProxyJump hostile.invalid\n    ControlMaster yes\n    PermitLocalCommand yes\n    LocalCommand /usr/bin/false hostile-local\n    IdentityFile {}\n",
            hostile_identity.display()
        ),
    )?;
    let hostile_config = hostile_ssh.join("config");
    fs::write(
        &hostile_config,
        format!("Host *\n    Include {}\n", included.display()),
    )?;
    let mut control_args = args.clone();
    control_args[1] = hostile_config.clone().into_os_string();
    let control = Command::new("/usr/bin/ssh")
        .arg("-G")
        .args(&control_args)
        .env_clear()
        .output()?;
    assert!(control.status.success());
    let control = String::from_utf8(control.stdout)?;
    let expanded = Command::new("/usr/bin/ssh")
        .arg("-G")
        .args(&args)
        .env_clear()
        .env("HOME", &hostile_home)
        .output()?;
    assert!(expanded.status.success());
    let expanded = String::from_utf8(expanded.stdout)?;
    let hostile_identity = hostile_identity
        .to_str()
        .ok_or("hostile path is not UTF-8")?;
    for hostile in ["hostile-proxy", "hostile-local", hostile_identity] {
        assert!(
            control.contains(hostile),
            "hostile SSH control config was not observed: {hostile}"
        );
    }
    for hostile in [
        "hostile-proxy",
        "hostile.invalid",
        "hostile-local",
        hostile_identity,
    ] {
        assert!(
            !expanded.contains(hostile),
            "ambient SSH config affected readiness: {hostile}"
        );
    }
    assert!(!paths.config().exists());
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn readiness_retries_transient_failure_with_identical_strict_argv() -> TestResult {
    let temp = TempDir::new()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("temporary root is not UTF-8")?;
    let (runtime, id, resolution, paths) = booted_readiness_context(&temp).await?;
    let counter = root.join("readiness-counter");
    let capture = root.join("readiness-argv");
    let program = executable_script(
        root,
        "transient-readiness",
        &format!(
            "count=0\n\
             if [ -f '{}' ]; then read -r count < '{}'; fi\n\
             count=$((count + 1))\n\
             printf '%s\\n' \"$count\" > '{}'\n\
             printf '%s\\0' \"$@\" >> '{}'\n\
             if [ \"$count\" -lt 3 ]; then\n\
                 printf 'connection refused\\n' >&2\n\
                 exit 255\n\
             fi",
            counter, counter, counter, capture
        ),
    )?;
    let config = fs::read_to_string(paths.config())?;
    let known_hosts = configured_known_hosts(&config)?.to_owned();
    let expected = strict_readiness_argv(&paths, &id, &known_hosts);

    let activated = SshManager
        .prepare_activation_for_paths_with_policy(
            &id,
            &runtime,
            Some(&resolution),
            &paths,
            &program,
            Duration::from_secs(1),
            SshReadinessPolicy {
                deadline: Duration::from_millis(500),
                retry_delay: Duration::from_millis(10),
                maximum_stderr: 128,
            },
        )
        .await?;

    assert!(activated);
    assert_eq!(fs::read_to_string(counter)?.trim(), "3");
    assert_eq!(
        fs::read(&capture)?,
        nul_terminated_args(&expected).repeat(3)
    );
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn permanent_readiness_failure_reports_bounded_lossy_final_stderr_tail() -> TestResult {
    let temp = TempDir::new()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("temporary root is not UTF-8")?;
    let (runtime, id, resolution, paths) = booted_readiness_context(&temp).await?;
    let program = executable_script(
        root,
        "failing-readiness",
        "printf '%096d' 0 >&2\nprintf '\\377Host key verification failed.\\n' >&2\nexit 255",
    )?;

    let error = SshManager
        .prepare_activation_for_paths_with_policy(
            &id,
            &runtime,
            Some(&resolution),
            &paths,
            &program,
            Duration::from_secs(1),
            SshReadinessPolicy {
                deadline: Duration::from_millis(500),
                retry_delay: Duration::from_millis(10),
                maximum_stderr: 64,
            },
        )
        .await
        .expect_err("permanent readiness failure must not activate SSH");
    assert_eq!(error.code(), "ssh_not_ready");
    let (endpoint, detail) = match error {
        gascand::ServiceError::SshNotReady { endpoint, detail } => (endpoint, detail),
        other => return Err(format!("expected SSH readiness error, got {other:?}").into()),
    };
    assert_eq!(endpoint.as_deref(), Some("127.0.0.1:24242"));
    assert!(
        detail.contains("127.0.0.1:24242"),
        "missing endpoint: {detail}"
    );
    assert!(detail.contains("500ms"), "missing deadline: {detail}");
    assert!(
        detail.contains("Host key verification failed."),
        "missing final OpenSSH detail: {detail}"
    );
    assert!(
        detail.contains('\u{fffd}'),
        "stderr was not lossy-decoded: {detail}"
    );
    assert!(
        detail.contains("Run `gascan doctor` for managed SSH configuration details."),
        "missing recovery instruction: {detail}"
    );
    let tail = detail
        .split("last OpenSSH stderr tail: ")
        .nth(1)
        .and_then(|value| value.split("\nRun `gascan doctor`").next())
        .ok_or("readiness error did not label the stderr tail")?;
    assert!(tail.len() <= 64, "stderr tail exceeded its bound: {tail:?}");
    assert!(tail.is_char_boundary(tail.len()));
    Ok(())
}

#[tokio::test]
async fn readiness_revalidates_the_managed_identity_pair() -> TestResult {
    let managed_temp = TempDir::new()?;
    let managed_paths = paths(&managed_temp)?;
    let stale_identity = ensure_host_identity(&managed_paths).await?;
    let ready = host("gascan-ready", 2222, &stale_identity);
    let prepared = prepare_openssh_files(
        &managed_paths,
        &stale_identity,
        std::slice::from_ref(&ready),
    )?;
    let generation_known_hosts = prepared.known_hosts().to_owned();

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

    assert!(
        readiness_ssh_args(
            &managed_paths,
            &stale_identity,
            &ready,
            &generation_known_hosts
        )
        .await
        .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn readiness_rejects_a_replaced_generation() -> TestResult {
    let temp = TempDir::new()?;
    let paths = paths(&temp)?;
    let identity = ensure_host_identity(&paths).await?;
    let ready = host("gascan-ready", 2222, &identity);
    let prepared = prepare_openssh_files(&paths, &identity, std::slice::from_ref(&ready))?;
    fs::write(
        prepared.known_hosts().as_std_path(),
        format!(
            "{},[127.0.0.1]:{} {}\n",
            ready.active.alias, 2223, ready.host_public_key
        ),
    )?;

    assert!(
        readiness_ssh_args(&paths, &identity, &ready, prepared.known_hosts())
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn readiness_rejects_a_valid_generation_with_the_wrong_host_key() -> TestResult {
    let temp = TempDir::new()?;
    let managed_paths = paths(&temp)?;
    let identity = ensure_host_identity(&managed_paths).await?;
    let expected = host("gascan-ready", 2222, &identity);

    let other_temp = TempDir::new()?;
    let other_paths = paths(&other_temp)?;
    let other_identity = ensure_host_identity(&other_paths).await?;
    let mut wrong_key = expected.clone();
    wrong_key.host_public_key = other_identity.public_key().to_owned();
    wrong_key.active.host_key_fingerprint = other_identity.fingerprint().to_owned();
    let prepared = prepare_openssh_files(&managed_paths, &identity, &[wrong_key])?;

    assert!(
        readiness_ssh_args(&managed_paths, &identity, &expected, prepared.known_hosts())
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn readiness_rejects_a_plain_alias_record_with_another_key() -> TestResult {
    let temp = TempDir::new()?;
    let managed_paths = paths(&temp)?;
    let identity = ensure_host_identity(&managed_paths).await?;
    let ready = host("gascan-ready", 2222, &identity);
    let other_temp = TempDir::new()?;
    let other_identity = ensure_host_identity(&paths(&other_temp)?).await?;
    let hostile = format!("gascan-ready {}", other_identity.public_key());
    require_matching_host_record("gascan-ready", &hostile)?;
    let generation = write_generation(
        &managed_paths,
        &format!(
            "gascan-ready,[127.0.0.1]:2222 {}\n{hostile}\n",
            ready.host_public_key
        ),
    )?;

    assert!(
        readiness_ssh_args(&managed_paths, &identity, &ready, &generation)
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn readiness_rejects_a_hashed_alias_record_with_another_key() -> TestResult {
    let temp = TempDir::new()?;
    let managed_paths = paths(&temp)?;
    let identity = ensure_host_identity(&managed_paths).await?;
    let ready = host("gascan-ready", 2222, &identity);
    let other_temp = TempDir::new()?;
    let other_identity = ensure_host_identity(&paths(&other_temp)?).await?;
    let hostile = hashed_host_record("gascan-ready", other_identity.public_key())?;
    let generation = write_generation(
        &managed_paths,
        &format!(
            "gascan-ready,[127.0.0.1]:2222 {}\n{hostile}\n",
            ready.host_public_key
        ),
    )?;

    assert!(
        readiness_ssh_args(&managed_paths, &identity, &ready, &generation)
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn readiness_rejects_a_wildcard_comma_host_pattern() -> TestResult {
    let temp = TempDir::new()?;
    let managed_paths = paths(&temp)?;
    let identity = ensure_host_identity(&managed_paths).await?;
    let ready = host("gascan-ready", 2222, &identity);
    let other_temp = TempDir::new()?;
    let other_identity = ensure_host_identity(&paths(&other_temp)?).await?;
    let hostile = format!("gascan-other,gascan-* {}", other_identity.public_key());
    require_matching_host_record("gascan-ready", &hostile)?;
    let generation = write_generation(
        &managed_paths,
        &format!(
            "gascan-ready,[127.0.0.1]:2222 {}\n{hostile}\n",
            ready.host_public_key
        ),
    )?;

    assert!(
        readiness_ssh_args(&managed_paths, &identity, &ready, &generation)
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn readiness_rejects_a_duplicate_canonical_endpoint() -> TestResult {
    let temp = TempDir::new()?;
    let managed_paths = paths(&temp)?;
    let identity = ensure_host_identity(&managed_paths).await?;
    let ready = host("gascan-ready", 2222, &identity);
    let other_temp = TempDir::new()?;
    let other_identity = ensure_host_identity(&paths(&other_temp)?).await?;
    let hostile = format!(
        "gascan-other,[127.0.0.1]:2222 {}",
        other_identity.public_key()
    );
    require_matching_host_record("[127.0.0.1]:2222", &hostile)?;
    let generation = write_generation(
        &managed_paths,
        &format!(
            "gascan-ready,[127.0.0.1]:2222 {}\n{hostile}\n",
            ready.host_public_key
        ),
    )?;

    assert!(
        readiness_ssh_args(&managed_paths, &identity, &ready, &generation)
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn readiness_accepts_other_canonical_managed_records() -> TestResult {
    let temp = TempDir::new()?;
    let managed_paths = paths(&temp)?;
    let identity = ensure_host_identity(&managed_paths).await?;
    let ready = host("gascan-ready", 2222, &identity);
    let other_temp = TempDir::new()?;
    let other_identity = ensure_host_identity(&paths(&other_temp)?).await?;
    let generation = write_generation(
        &managed_paths,
        &format!(
            "gascan-ready,[127.0.0.1]:2222 {}\n\ngascan-other,[127.0.0.1]:2223 {}\n",
            ready.host_public_key,
            other_identity.public_key()
        ),
    )?;

    readiness_ssh_args(&managed_paths, &identity, &ready, &generation).await?;
    Ok(())
}

#[tokio::test]
async fn readiness_rejects_noncanonical_generation_grammar() -> TestResult {
    let temp = TempDir::new()?;
    let managed_paths = paths(&temp)?;
    let identity = ensure_host_identity(&managed_paths).await?;
    let ready = host("gascan-ready", 2222, &identity);
    let other_temp = TempDir::new()?;
    let other_identity = ensure_host_identity(&paths(&other_temp)?).await?;
    let canonical = format!("gascan-ready,[127.0.0.1]:2222 {}\n", ready.host_public_key);
    let cases = [
        ("comment", "# trusted by somebody else\n".to_owned()),
        (
            "negated pattern",
            format!("!gascan-ready {}\n", other_identity.public_key()),
        ),
        (
            "malformed alias",
            format!(
                "gascan--other,[127.0.0.1]:2223 {}\n",
                other_identity.public_key()
            ),
        ),
        (
            "malformed endpoint",
            format!(
                "gascan-other,[127.0.0.1]:02223 {}\n",
                other_identity.public_key()
            ),
        ),
        (
            "non-loopback endpoint",
            format!(
                "gascan-other,[192.0.2.1]:2223 {}\n",
                other_identity.public_key()
            ),
        ),
        (
            "invalid public key",
            "gascan-other,[127.0.0.1]:2223 ssh-ed25519 AAAA\n".to_owned(),
        ),
        (
            "duplicate alias",
            format!(
                "gascan-other,[127.0.0.1]:2223 {}\ngascan-other,[127.0.0.1]:2224 {}\n",
                other_identity.public_key(),
                other_identity.public_key()
            ),
        ),
        ("duplicate target record", canonical.clone()),
    ];

    for (case, hostile) in cases {
        let generation = write_generation(&managed_paths, &format!("{canonical}{hostile}"))?;
        assert!(
            readiness_ssh_args(&managed_paths, &identity, &ready, &generation)
                .await
                .is_err(),
            "readiness accepted {case}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn readiness_rejects_a_generation_for_a_different_alias() -> TestResult {
    let temp = TempDir::new()?;
    let paths = paths(&temp)?;
    let identity = ensure_host_identity(&paths).await?;
    let expected = host("gascan-ready", 2222, &identity);
    let prepared = prepare_openssh_files(&paths, &identity, std::slice::from_ref(&expected))?;
    let mut wrong_alias = expected;
    wrong_alias.active.alias = "gascan-other".to_owned();

    assert!(
        readiness_ssh_args(&paths, &identity, &wrong_alias, prepared.known_hosts())
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn readiness_rejects_a_generation_for_a_different_port() -> TestResult {
    let temp = TempDir::new()?;
    let paths = paths(&temp)?;
    let identity = ensure_host_identity(&paths).await?;
    let expected = host("gascan-ready", 2222, &identity);
    let prepared = prepare_openssh_files(&paths, &identity, std::slice::from_ref(&expected))?;
    let mut wrong_port = expected;
    wrong_port.active.port = 2223;

    assert!(
        readiness_ssh_args(&paths, &identity, &wrong_port, prepared.known_hosts())
            .await
            .is_err()
    );
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
async fn published_status_reads_only_the_matching_committed_alias() -> TestResult {
    let temp = TempDir::new()?;
    let paths = paths(&temp)?;
    let identity = ensure_host_identity(&paths).await?;
    let id = SandboxId::try_from("code-123456789abc".to_owned())?;
    let managed = host("gascan-code-123456789abc", 22222, &identity);
    let expected = SshResolution::new(
        1,
        serde_json::json!({
            "enabled": true,
            "host_key_fingerprint": managed.active.host_key_fingerprint.clone(),
            "client_key_fingerprint": managed.active.client_key_fingerprint.clone(),
        }),
    );

    assert_eq!(
        SshManager
            .published_for_paths(&id, Some(&expected), &paths)
            .await?,
        None
    );
    publish_openssh_files(&paths, &identity, std::slice::from_ref(&managed))?;
    assert_eq!(
        SshManager
            .published_for_paths(&id, Some(&expected), &paths)
            .await?,
        Some(managed.active)
    );
    Ok(())
}

#[tokio::test]
async fn one_published_snapshot_serves_multiple_records_without_revalidating_identity() -> TestResult
{
    let temp = TempDir::new()?;
    let paths = paths(&temp)?;
    let identity = ensure_host_identity(&paths).await?;
    let first_id = SandboxId::try_from("code-111111111111".to_owned())?;
    let second_id = SandboxId::try_from("code-222222222222".to_owned())?;
    let first = host(&format!("gascan-{first_id}"), 22221, &identity);
    let second = host(&format!("gascan-{second_id}"), 22222, &identity);
    publish_openssh_files(&paths, &identity, &[first.clone(), second.clone()])?;
    let resolution = |active: &ActiveSsh| {
        SshResolution::new(
            1,
            serde_json::json!({
                "enabled": true,
                "host_key_fingerprint": active.host_key_fingerprint,
                "client_key_fingerprint": active.client_key_fingerprint,
            }),
        )
    };
    let first_resolution = resolution(&first.active);
    let second_resolution = resolution(&second.active);

    let snapshot = SshManager.published_snapshot_for_paths(&paths).await?;
    fs::remove_file(paths.private_key().as_std_path())?;

    assert_eq!(
        snapshot.for_sandbox(&first_id, Some(&first_resolution))?,
        Some(first.active)
    );
    assert_eq!(
        snapshot.for_sandbox(&second_id, Some(&second_resolution))?,
        Some(second.active)
    );
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
