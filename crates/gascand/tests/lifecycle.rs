use async_trait::async_trait;
use camino::{Utf8Path, Utf8PathBuf};
use gascan_core::fake_runtime::{FailureBoundary, FakeRuntime};
use gascan_core::manifest::Manifest;
use gascan_core::policy::PolicyCompiler;
use gascan_core::runtime::{
    ResourceIdentity, ResourceKind, ResourceOwnership, RuntimeBackend, RuntimeCall, RuntimeError,
};
use gascan_core::sandbox::SandboxSpec;
use gascand::{
    NoopProvisioner, OperationStatus, PortReservation, SandboxService, SshManager, SshPaths,
    UpRequest, ensure_host_identity,
};
use gascand::{ProvisionRequest, ProvisionResolution, Provisioner, ServiceError};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use std::error::Error;
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::os::unix::fs::PermissionsExt;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

type TestResult = Result<(), Box<dyn Error>>;

fn networked_spec(name: &str, root: &Utf8Path) -> Result<SandboxSpec, Box<dyn Error>> {
    std::fs::write(
        root.join("gascan.toml"),
        "version = 1\nnetwork = 'networked'\n[ssh]\nenabled = false\n",
    )?;
    Ok(SandboxSpec::from_root(name, root, Manifest::load(root)?)?)
}

fn networked_ssh_spec(
    name: &str,
    root: &Utf8Path,
    host_port: Option<u16>,
) -> Result<SandboxSpec, Box<dyn Error>> {
    let port = host_port.map_or_else(String::new, |port| format!("host_port = {port}\n"));
    std::fs::write(
        root.join("gascan.toml"),
        format!("version = 1\nnetwork = 'networked'\n[ssh]\n{port}"),
    )?;
    Ok(SandboxSpec::from_root(name, root, Manifest::load(root)?)?)
}

fn ssh_paths(root: &Utf8Path, name: &str) -> Result<SshPaths, Box<dyn Error>> {
    let config_home = root.join(name);
    std::fs::create_dir(&config_home)?;
    let config_home = std::fs::canonicalize(config_home)?;
    Ok(SshPaths::for_environment(
        Some(config_home.as_os_str()),
        None,
    )?)
}

fn test_service(
    runtime: FakeRuntime,
    root: &Utf8Path,
    paths: SshPaths,
) -> Result<SandboxService<FakeRuntime>, Box<dyn Error>> {
    Ok(SandboxService::new_with_ssh_for_tests(
        runtime,
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
        paths,
        Utf8PathBuf::from("/usr/bin/true"),
    ))
}

fn readiness_program(
    root: &Utf8Path,
    name: &str,
    body: &str,
) -> Result<Utf8PathBuf, Box<dyn Error>> {
    let path = root.join(name);
    std::fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n"))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    Ok(path)
}

fn capturing_readiness_program(
    root: &Utf8Path,
    name: &str,
    capture: &Utf8Path,
) -> Result<Utf8PathBuf, Box<dyn Error>> {
    readiness_program(
        root,
        name,
        &format!("printf '%s\\n' \"$@\" > '{}'", capture),
    )
}

async fn generated_public_key(root: &Utf8Path, name: &str) -> Result<String, Box<dyn Error>> {
    let paths = ssh_paths(root, name)?;
    Ok(ensure_host_identity(&paths).await?.public_key().to_owned())
}

fn service_with_readiness(
    runtime: FakeRuntime,
    root: &Utf8Path,
    paths: SshPaths,
    readiness: Utf8PathBuf,
) -> Result<SandboxService<FakeRuntime>, Box<dyn Error>> {
    Ok(SandboxService::new_with_ssh_for_tests(
        runtime,
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
        paths,
        readiness,
    ))
}

fn spec(name: &str, root: &Utf8Path) -> Result<SandboxSpec, Box<dyn Error>> {
    Ok(SandboxSpec::from_root(name, root, Manifest::load(root)?)?)
}

fn rewrite_runtime_image(path: &Utf8Path, image: &str) -> TestResult {
    let mut snapshot: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let sandboxes = snapshot["sandboxes"]
        .as_array_mut()
        .ok_or("runtime sandboxes")?;
    let sandbox = sandboxes.first_mut().ok_or("runtime sandbox")?;
    sandbox["image"] = json!(image);
    std::fs::write(path, serde_json::to_vec(&snapshot)?)?;
    Ok(())
}

fn automatic_port_collision() -> RuntimeError {
    RuntimeError::CommandFailed {
        operation: "container".to_owned(),
        exit_code: Some(1),
        stderr: "Error: listen tcp 127.0.0.1:49152: bind: address already in use\n".to_owned(),
    }
}

#[test]
fn automatic_ssh_port_reservation_is_loopback_unprivileged_and_exclusive() -> TestResult {
    let reservation = PortReservation::reserve()?;
    let port = reservation.port();
    assert!((1024..=u16::MAX).contains(&port));
    assert!(
        TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).is_err(),
        "the reservation must remain live until explicitly released"
    );
    reservation.release();
    let rebound = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))?;
    assert_eq!(
        rebound.local_addr()?,
        std::net::SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
    );
    Ok(())
}

#[tokio::test]
async fn explicit_ssh_port_bypasses_automatic_reservation() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("utf8 root")?;
    let occupied = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?;
    let port = occupied.local_addr()?.port();
    let desired = networked_ssh_spec("explicit-bypass", root, Some(port))?;
    let paths = ssh_paths(root, "ssh-explicit-bypass")?;

    let prepared = SshManager
        .prepare_create_for_paths(&desired, &paths)
        .await?
        .ok_or("enabled SSH preparation")?;

    assert_eq!(prepared.host_port(), port);
    drop(occupied);
    Ok(())
}

#[tokio::test]
async fn automatic_ssh_port_retries_exactly_eight_native_bind_collisions() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("utf8 root")?;
    let desired = networked_ssh_spec("automatic-retries", root, None)?;
    let runtime = FakeRuntime::default();
    for _ in 0..8 {
        runtime.queue_create_error(automatic_port_collision()).await;
    }
    let service = test_service(
        runtime.clone(),
        root,
        ssh_paths(root, "ssh-automatic-retries")?,
    )?;

    let error = match service.up(UpRequest::new(desired)).await {
        Ok(_) => return Err("eight collisions unexpectedly succeeded".into()),
        Err(error) => error,
    };

    assert_eq!(error.code(), "ssh_port_unavailable");
    assert_eq!(
        runtime
            .calls()
            .await
            .iter()
            .filter(|call| matches!(call, RuntimeCall::Create(_)))
            .count(),
        8
    );
    Ok(())
}

#[tokio::test]
async fn explicit_ssh_port_collision_never_retries_or_substitutes() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("utf8 root")?;
    let port = PortReservation::reserve()?.port();
    let desired = networked_ssh_spec("explicit-collision", root, Some(port))?;
    let runtime = FakeRuntime::default();
    runtime.queue_create_error(automatic_port_collision()).await;
    let service = test_service(
        runtime.clone(),
        root,
        ssh_paths(root, "ssh-explicit-collision")?,
    )?;

    let error = match service.up(UpRequest::new(desired)).await {
        Ok(_) => return Err("an explicit collision unexpectedly succeeded".into()),
        Err(error) => error,
    };

    assert_eq!(error.code(), "ssh_port_unavailable");
    let creates = runtime
        .calls()
        .await
        .into_iter()
        .filter_map(|call| match call {
            RuntimeCall::Create(request) => Some(request),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(creates.len(), 1);
    assert!(creates[0].ports().iter().any(|mapping| {
        mapping.host_address == std::net::IpAddr::V4(Ipv4Addr::LOCALHOST)
            && mapping.host_port == port
            && mapping.guest_port == 22
    }));
    Ok(())
}

#[tokio::test]
async fn networked_default_create_injects_client_key_and_native_loopback_mapping() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("utf8 root")?;
    let paths = ssh_paths(root, "ssh-networked-create")?;
    let client = ensure_host_identity(&paths).await?;
    let host_public_key = generated_public_key(root, "host-networked-create").await?;
    let runtime = FakeRuntime::default();
    runtime
        .set_exec_result(format!("{host_public_key}\n").into_bytes(), Vec::new(), 0)
        .await;
    runtime.queue_created_ssh_host_port(23_456).await;
    let service = test_service(runtime.clone(), root, paths)?;
    let desired = networked_ssh_spec("networked-create", root, None)?;

    service.up(UpRequest::new(desired.clone())).await?;

    let create = runtime
        .calls()
        .await
        .into_iter()
        .find_map(|call| match call {
            RuntimeCall::Create(request) => Some(request),
            _ => None,
        })
        .ok_or("create request")?;
    assert_eq!(
        create
            .environment()
            .get("GASCAN_SSH_AUTHORIZED_KEY")
            .map(String::as_str),
        Some(client.public_key())
    );
    assert_eq!(
        create.environment().get("GASCAN_SSH_ENABLED"),
        Some(&"1".to_owned())
    );
    assert!(create.ports().iter().any(|port| {
        port.host_address == std::net::IpAddr::V4(Ipv4Addr::LOCALHOST)
            && port.guest_port == 22
            && port.host_port >= 1024
    }));
    assert_eq!(
        runtime
            .inspect(desired.id())
            .await?
            .ok_or("runtime sandbox")?
            .ports(),
        [gascan_core::runtime::RuntimePort {
            host_address: std::net::IpAddr::V4(Ipv4Addr::LOCALHOST),
            host_port: 23_456,
            guest_port: 22,
        }]
    );
    Ok(())
}

#[tokio::test]
async fn offline_default_create_injects_no_key_and_publishes_no_ssh_port() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("utf8 root")?;
    let desired = spec("offline-no-ssh", root)?;
    let runtime = FakeRuntime::default();
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );

    service.up(UpRequest::new(desired)).await?;

    let create = runtime
        .calls()
        .await
        .into_iter()
        .find_map(|call| match call {
            RuntimeCall::Create(request) => Some(request),
            _ => None,
        })
        .ok_or("create request")?;
    assert_eq!(
        create.environment().get("GASCAN_SSH_ENABLED"),
        Some(&"0".to_owned())
    );
    assert!(
        !create
            .environment()
            .contains_key("GASCAN_SSH_AUTHORIZED_KEY")
    );
    assert!(
        create
            .ports()
            .iter()
            .all(|mapping| mapping.guest_port != 22)
    );
    Ok(())
}

#[tokio::test]
async fn ssh_up_inspects_mapping_reads_host_key_runs_strict_readiness_and_commits_alias()
-> TestResult {
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("utf8 root")?;
    let paths = ssh_paths(root, "ssh-up-sequence")?;
    let client = ensure_host_identity(&paths).await?;
    let host_public_key = generated_public_key(root, "host-up-sequence").await?;
    let capture = root.join("readiness-args");
    let readiness = capturing_readiness_program(root, "capture-readiness", &capture)?;
    let runtime = FakeRuntime::default();
    runtime
        .set_exec_result(format!("{host_public_key}\n").into_bytes(), Vec::new(), 0)
        .await;
    runtime.queue_created_ssh_host_port(23_457).await;
    let service = service_with_readiness(runtime.clone(), root, paths.clone(), readiness)?;
    let desired = networked_ssh_spec("ssh-up-sequence", root, None)?;

    service.up(UpRequest::new(desired.clone())).await?;

    let calls = runtime.calls().await;
    let start = calls
        .iter()
        .position(|call| matches!(call, RuntimeCall::Start(id) if id == desired.id()))
        .ok_or("start")?;
    let mapping_inspect = calls
        .iter()
        .enumerate()
        .skip(start + 1)
        .find_map(|(index, call)| {
            matches!(call, RuntimeCall::Inspect(id) if id == desired.id()).then_some(index)
        })
        .ok_or("mapping inspect")?;
    let host_key = calls
        .iter()
        .enumerate()
        .skip(mapping_inspect + 1)
        .find_map(|(index, call)| match call {
            RuntimeCall::Exec(request)
                if request.argv
                    == [
                        "/usr/bin/cat",
                        "/home/workspace/.config/gascan/ssh/host/ssh_host_ed25519_key.pub",
                    ] =>
            {
                Some(index)
            }
            _ => None,
        })
        .ok_or("fixed host-key read")?;
    assert!(start < mapping_inspect && mapping_inspect < host_key);

    let args = std::fs::read_to_string(&capture)?;
    for required in [
        "-F",
        "/dev/null",
        "HostName=127.0.0.1",
        "Port=23457",
        "User=workspace",
        "StrictHostKeyChecking=yes",
        "IdentitiesOnly=yes",
        "BatchMode=yes",
        "ForwardAgent=no",
        "ClearAllForwardings=yes",
        "127.0.0.1",
        "/usr/bin/true",
    ] {
        assert!(
            args.lines().any(|argument| argument == required),
            "missing readiness argument {required:?}: {args}"
        );
    }
    assert!(
        args.lines()
            .any(|argument| { argument == format!("IdentityFile={}", client.private_key()) })
    );
    assert!(
        args.lines()
            .any(|argument| argument.starts_with("UserKnownHostsFile="))
    );

    let record = service.status(desired.id())?.ok_or("sandbox record")?;
    let resolution = record.ssh_resolution.ok_or("SSH resolution")?;
    assert_eq!(resolution.version, 1);
    assert_eq!(resolution.details["enabled"], true);
    assert_eq!(
        resolution.details["client_key_fingerprint"],
        client.fingerprint()
    );
    assert_eq!(
        std::fs::read_to_string(paths.config())?.contains(&format!(
            "Host gascan-{}\n    HostName 127.0.0.1\n    Port 23457",
            desired.id()
        )),
        true
    );
    Ok(())
}

#[tokio::test]
async fn ssh_down_removes_alias_before_attempting_runtime_stop() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("utf8 root")?;
    let paths = ssh_paths(root, "ssh-down-order")?;
    let host_public_key = generated_public_key(root, "host-down-order").await?;
    let runtime = FakeRuntime::default();
    runtime
        .set_exec_result(format!("{host_public_key}\n").into_bytes(), Vec::new(), 0)
        .await;
    let service = test_service(runtime.clone(), root, paths.clone())?;
    let desired = networked_ssh_spec("ssh-down-order", root, None)?;
    service.up(UpRequest::new(desired.clone())).await?;
    runtime.inject_failure(FailureBoundary::Stop).await;

    assert!(service.stop(desired.id()).await.is_err());

    assert!(
        !std::fs::read_to_string(paths.config())?
            .contains(&format!("Host gascan-{}", desired.id()))
    );
    Ok(())
}

#[tokio::test]
async fn ssh_up_after_down_restores_same_inspected_native_mapping() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("utf8 root")?;
    let paths = ssh_paths(root, "ssh-down-up")?;
    let host_public_key = generated_public_key(root, "host-down-up").await?;
    let runtime = FakeRuntime::default();
    runtime
        .set_exec_result(format!("{host_public_key}\n").into_bytes(), Vec::new(), 0)
        .await;
    runtime.queue_created_ssh_host_port(23_458).await;
    let service = test_service(runtime.clone(), root, paths.clone())?;
    let desired = networked_ssh_spec("ssh-down-up", root, None)?;
    service.up(UpRequest::new(desired.clone())).await?;
    service.stop(desired.id()).await?;

    service.up(UpRequest::new(desired.clone())).await?;

    let config = std::fs::read_to_string(paths.config())?;
    assert!(config.contains(&format!("Host gascan-{}", desired.id())));
    assert!(config.contains("    Port 23458\n"));
    assert_eq!(
        runtime
            .inspect(desired.id())
            .await?
            .ok_or("runtime")?
            .ports()[0]
            .host_port,
        23_458
    );
    Ok(())
}

#[tokio::test]
async fn ssh_destroy_removes_alias_and_sandbox_resources_but_retains_client_identity() -> TestResult
{
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("utf8 root")?;
    let paths = ssh_paths(root, "ssh-destroy")?;
    let client = ensure_host_identity(&paths).await?;
    let host_public_key = generated_public_key(root, "host-destroy").await?;
    let runtime = FakeRuntime::default();
    runtime
        .set_exec_result(format!("{host_public_key}\n").into_bytes(), Vec::new(), 0)
        .await;
    let service = test_service(runtime.clone(), root, paths.clone())?;
    let desired = networked_ssh_spec("ssh-destroy", root, None)?;
    service.up(UpRequest::new(desired.clone())).await?;

    service.destroy(desired.id()).await?;

    assert!(
        !std::fs::read_to_string(paths.config())?
            .contains(&format!("Host gascan-{}", desired.id()))
    );
    assert!(client.private_key().exists());
    assert!(paths.public_key().exists());
    assert!(runtime.list_resources().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn ssh_host_key_failure_publishes_no_alias_and_rolls_back_created_resources() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("utf8 root")?;
    let paths = ssh_paths(root, "ssh-host-key-failure")?;
    let runtime = FakeRuntime::default();
    runtime
        .set_exec_result(b"ssh-rsa invalid\n".to_vec(), Vec::new(), 0)
        .await;
    let service = test_service(runtime.clone(), root, paths.clone())?;
    let desired = networked_ssh_spec("ssh-host-key-failure", root, None)?;

    let error = match service.up(UpRequest::new(desired.clone())).await {
        Ok(_) => return Err("invalid host key unexpectedly succeeded".into()),
        Err(error) => error,
    };

    assert_eq!(error.code(), "ssh_host_key_mismatch");
    assert!(runtime.inspect(desired.id()).await?.is_none());
    assert!(
        !paths.config().exists()
            || !std::fs::read_to_string(paths.config())?
                .contains(&format!("Host gascan-{}", desired.id()))
    );
    Ok(())
}

#[tokio::test]
async fn ssh_readiness_failure_publishes_no_alias_and_rolls_back_created_resources() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("utf8 root")?;
    let paths = ssh_paths(root, "ssh-readiness-failure")?;
    let host_public_key = generated_public_key(root, "host-readiness-failure").await?;
    let readiness = readiness_program(root, "fail-readiness", "exit 23")?;
    let runtime = FakeRuntime::default();
    runtime
        .set_exec_result(format!("{host_public_key}\n").into_bytes(), Vec::new(), 0)
        .await;
    let service = service_with_readiness(runtime.clone(), root, paths.clone(), readiness)?;
    let desired = networked_ssh_spec("ssh-readiness-failure", root, None)?;

    let error = match service.up(UpRequest::new(desired.clone())).await {
        Ok(_) => return Err("failed readiness unexpectedly succeeded".into()),
        Err(error) => error,
    };

    assert_eq!(error.code(), "ssh_not_ready");
    assert!(runtime.inspect(desired.id()).await?.is_none());
    assert!(
        !paths.config().exists()
            || !std::fs::read_to_string(paths.config())?
                .contains(&format!("Host gascan-{}", desired.id()))
    );
    Ok(())
}

#[tokio::test]
async fn ssh_config_commit_failure_publishes_no_alias_restores_resolution_and_rolls_back()
-> TestResult {
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("utf8 root")?;
    let paths = ssh_paths(root, "ssh-config-failure")?;
    let host_public_key = generated_public_key(root, "host-config-failure").await?;
    let target = root.join("hostile-config-target");
    std::fs::write(&target, "Host hostile\n")?;
    let readiness = readiness_program(
        root,
        "replace-config-before-commit",
        &format!(
            "/bin/rm -f '{}'\n/bin/ln -s '{}' '{}'",
            paths.config(),
            target,
            paths.config()
        ),
    )?;
    let runtime = FakeRuntime::default();
    runtime
        .set_exec_result(format!("{host_public_key}\n").into_bytes(), Vec::new(), 0)
        .await;
    let service = service_with_readiness(runtime.clone(), root, paths.clone(), readiness)?;
    let desired = networked_ssh_spec("ssh-config-failure", root, None)?;

    let error = match service.up(UpRequest::new(desired.clone())).await {
        Ok(_) => return Err("unsafe config target unexpectedly committed".into()),
        Err(error) => error,
    };

    assert_eq!(error.code(), "ssh_config_update_failed");
    assert_eq!(std::fs::read_to_string(&target)?, "Host hostile\n");
    assert!(runtime.inspect(desired.id()).await?.is_none());
    assert!(
        service
            .status(desired.id())?
            .ok_or("failed record")?
            .ssh_resolution
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn apply_rejects_changed_storage_without_runtime_calls() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let runtime = FakeRuntime::default();
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    service
        .up(UpRequest::new(spec("storage-change", root)?))
        .await?;

    std::fs::write(
        root.join("gascan.toml"),
        "version = 1\n[storage]\ntools = \"20GiB\"\n",
    )?;
    let before = runtime.calls().await.len();
    let error = match service
        .apply(UpRequest::new(spec("storage-change", root)?))
        .await
    {
        Ok(_) => return Err("storage change unexpectedly applied".into()),
        Err(error) => error,
    };

    assert_eq!(error.code(), "storage_change_requires_recreate");
    assert!(error.to_string().contains("tools"));
    assert!(error.to_string().contains("10GiB"));
    assert!(error.to_string().contains("20GiB"));
    assert_eq!(runtime.calls().await.len(), before);
    Ok(())
}

#[tokio::test]
async fn apply_rejects_legacy_storage_resolution_without_runtime_calls() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let runtime = FakeRuntime::default();
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    let desired = spec("legacy-storage", root)?;
    service.up(UpRequest::new(desired.clone())).await?;
    let mut record = service.status(desired.id())?.ok_or("sandbox record")?;
    record.storage_resolution = None;
    service.store().put_sandbox(&record)?;

    let before = runtime.calls().await.len();
    let error = match service.apply(UpRequest::new(desired)).await {
        Ok(_) => return Err("legacy storage resolution unexpectedly applied".into()),
        Err(error) => error,
    };

    assert_eq!(error.code(), "storage_change_requires_recreate");
    assert_eq!(runtime.calls().await.len(), before);
    Ok(())
}

#[derive(Default)]
struct ControlledProvisioner {
    fail_provision: AtomicBool,
    fail_health: AtomicBool,
    provisions: AtomicUsize,
}

#[async_trait]
impl Provisioner for ControlledProvisioner {
    async fn provision(
        &self,
        _request: ProvisionRequest<'_>,
    ) -> Result<ProvisionResolution, ServiceError> {
        self.provisions.fetch_add(1, Ordering::SeqCst);
        if self.fail_provision.load(Ordering::SeqCst) {
            return Err(ServiceError::Provision(
                "injected provision failure".to_owned(),
            ));
        }
        Ok(ProvisionResolution {
            setup: Some(json!({"resolved":"prior-setup"})),
            tools: Some(json!({"resolved":"prior-tools"})),
        })
    }

    async fn health_check(
        &self,
        _id: &gascan_core::sandbox::SandboxId,
    ) -> Result<(), ServiceError> {
        if self.fail_health.load(Ordering::SeqCst) {
            return Err(ServiceError::Provision(
                "injected health failure".to_owned(),
            ));
        }
        Ok(())
    }
}

struct RollbackFailingProvisioner {
    runtime: FakeRuntime,
}

struct ExtraContainerProvisioner {
    runtime: FakeRuntime,
    id: gascan_core::sandbox::SandboxId,
}

#[async_trait]
impl Provisioner for ExtraContainerProvisioner {
    async fn provision(
        &self,
        _request: ProvisionRequest<'_>,
    ) -> Result<ProvisionResolution, ServiceError> {
        self.runtime
            .seed_container_resource(
                "extra-rollback-container",
                self.id.clone(),
                ResourceOwnership::GasCanOwned,
            )
            .await?;
        Err(ServiceError::Provision(
            "injected provision failure with extra container".to_owned(),
        ))
    }

    async fn health_check(
        &self,
        _id: &gascan_core::sandbox::SandboxId,
    ) -> Result<(), ServiceError> {
        Ok(())
    }
}

struct TerminalReadFailingProvisioner {
    store: gascand::Store,
}

#[async_trait]
impl Provisioner for TerminalReadFailingProvisioner {
    async fn provision(
        &self,
        _request: ProvisionRequest<'_>,
    ) -> Result<ProvisionResolution, ServiceError> {
        Ok(ProvisionResolution::default())
    }

    async fn health_check(
        &self,
        _id: &gascan_core::sandbox::SandboxId,
    ) -> Result<(), ServiceError> {
        self.store.fail_next_operation_event_read();
        Ok(())
    }
}

#[async_trait]
impl Provisioner for RollbackFailingProvisioner {
    async fn provision(
        &self,
        _request: ProvisionRequest<'_>,
    ) -> Result<ProvisionResolution, ServiceError> {
        self.runtime.inject_failure(FailureBoundary::Remove).await;
        Err(ServiceError::Provision(
            "injected replacement provision failure".to_owned(),
        ))
    }

    async fn health_check(
        &self,
        _id: &gascan_core::sandbox::SandboxId,
    ) -> Result<(), ServiceError> {
        Ok(())
    }
}

#[tokio::test]
async fn failed_initial_up_retry_runs_provision_and_persists_actual_resolution() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("retry-hooks", root, Manifest::load(root)?);
    let runtime = FakeRuntime::default();
    let provisioner = Arc::new(ControlledProvisioner::default());
    provisioner.fail_provision.store(true, Ordering::SeqCst);
    let service = SandboxService::new(
        runtime,
        gascand::Store::open(root.join("state.db"))?,
        provisioner.clone(),
    );
    assert!(service.up(UpRequest::new(make_spec()?)).await.is_err());
    provisioner.fail_provision.store(false, Ordering::SeqCst);
    service.up(UpRequest::new(make_spec()?)).await?;
    service.up(UpRequest::new(make_spec()?)).await?;
    assert_eq!(provisioner.provisions.load(Ordering::SeqCst), 2);
    let record = service.status(make_spec()?.id())?.ok_or("record")?;
    assert_eq!(
        record
            .setup_resolution
            .as_ref()
            .and_then(|value| value.details.get("resolution")),
        Some(&json!({"resolved":"prior-setup"}))
    );
    Ok(())
}

#[tokio::test]
async fn retained_setup_failure_persists_storage_and_up_retries_setup() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let setup = b"exit 28\n";
    std::fs::write(root.join("setup.sh"), setup)?;
    std::fs::write(
        root.join("gascan.toml"),
        "version = 1\nsetup = 'setup.sh'\n[storage]\ntools = '11GiB'\n",
    )?;
    let make_spec = || SandboxSpec::from_root("retry-retained-setup", root, Manifest::load(root)?);
    let runtime = FakeRuntime::default();
    let digest = format!("{:x}  /workspace/setup.sh\n", Sha256::digest(setup)).into_bytes();
    runtime
        .queue_exec_results([
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (digest.clone(), Vec::new(), 0),
            (Vec::new(), b"No space left on device".to_vec(), 28),
        ])
        .await;
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );

    assert!(service.up(UpRequest::new(make_spec()?)).await.is_err());
    let id = make_spec()?.id().clone();
    let failed = service.status(&id)?.ok_or("failed record")?;
    assert_eq!(
        failed
            .storage_resolution
            .as_ref()
            .and_then(|resolution| resolution.details["tools_bytes"].as_u64()),
        Some(11 * 1024_u64.pow(3))
    );

    runtime
        .queue_exec_results([
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (digest, Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
        ])
        .await;
    service.up(UpRequest::new(make_spec()?)).await?;
    let setup_runs = runtime
        .calls()
        .await
        .into_iter()
        .filter(|call| {
            matches!(
                call,
                RuntimeCall::Exec(request)
                    if request.argv.first().map(String::as_str) == Some("/bin/bash")
            )
        })
        .count();
    assert_eq!(setup_runs, 2);
    assert!(
        service
            .status(&id)?
            .ok_or("retried record")?
            .setup_resolution
            .is_some()
    );
    Ok(())
}

#[tokio::test]
async fn create_failure_before_resources_does_not_persist_storage_resolution() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    std::fs::write(
        root.join("gascan.toml"),
        "version = 1\n[storage]\ntools = '11GiB'\n",
    )?;
    let desired = spec("create-no-storage-resolution", root)?;
    let runtime = FakeRuntime::failing_once(FailureBoundary::Create);
    let service = SandboxService::new(
        runtime,
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );

    assert!(service.up(UpRequest::new(desired.clone())).await.is_err());
    assert!(
        service
            .status(desired.id())?
            .ok_or("failed record")?
            .storage_resolution
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn stopped_apply_that_starts_then_fails_records_running_reality() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("apply-running-reality", root, Manifest::load(root)?);
    let id = make_spec()?.id().clone();
    let runtime = FakeRuntime::default();
    let provisioner = Arc::new(ControlledProvisioner::default());
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        provisioner.clone(),
    );
    service.up(UpRequest::new(make_spec()?)).await?;
    service.stop(&id).await?;
    std::fs::write(
        root.join("gascan.toml"),
        "version = 1\n[tools]\nnode = '22'\n",
    )?;
    provisioner.fail_provision.store(true, Ordering::SeqCst);
    assert!(service.apply(UpRequest::new(make_spec()?)).await.is_err());
    assert_eq!(
        service.latest_operation()?.ok_or("operation")?.status,
        OperationStatus::Failed
    );
    assert_eq!(
        service.status(&id)?.ok_or("record")?.actual_state,
        gascand::ActualState::Running
    );
    Ok(())
}

#[tokio::test]
async fn unchanged_apply_inspects_and_starts_stopped_runtime_without_rerunning_hooks() -> TestResult
{
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("unchanged-apply", root, Manifest::load(root)?);
    let id = make_spec()?.id().clone();
    let runtime = FakeRuntime::default();
    let provisioner = Arc::new(ControlledProvisioner::default());
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        provisioner.clone(),
    );
    service.up(UpRequest::new(make_spec()?)).await?;
    runtime.stop(&id).await?;
    service.apply(UpRequest::new(make_spec()?)).await?;
    assert_eq!(
        runtime.inspect(&id).await?.ok_or("runtime")?.state,
        gascan_core::runtime::ContainerState::Running
    );
    assert_eq!(provisioner.provisions.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn image_resolution_only_proves_the_approved_image_when_valid_and_matching() -> TestResult {
    let old_digest = "ghcr.io/liquescent-development/gascan/workspace:old@sha256:\
                      bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    for (recorded, change_required) in [
        (None, true),
        (Some(json!({"digest": "RUNNING"})), false),
        (Some(json!({"digest": old_digest})), true),
        (Some(json!({"digest": 7})), true),
    ] {
        let root = tempfile::tempdir()?;
        let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
        let make_spec = || SandboxSpec::from_root("image-state", root, Manifest::load(root)?);
        let id = make_spec()?.id().clone();
        let runtime = FakeRuntime::default();
        let service = SandboxService::new(
            runtime.clone(),
            gascand::Store::open(root.join("state.db"))?,
            Arc::new(NoopProvisioner),
        );
        service.up(UpRequest::new(make_spec()?)).await?;
        let running_digest = runtime.inspect(&id).await?.ok_or("running sandbox")?.image;
        let mut record = service.status(&id)?.ok_or("sandbox record")?;
        record.image_resolution = recorded.map(|details| {
            let details = if details["digest"] == "RUNNING" {
                json!({"digest": running_digest})
            } else {
                details
            };
            gascand::ImageResolution::new(1, details)
        });
        service.store().put_sandbox(&record)?;

        let operation = service.up(UpRequest::new(make_spec()?)).await?;
        let events = service.store().operation_events(operation.id)?;
        assert_eq!(
            events.iter().any(|event| {
                event
                    .details
                    .as_ref()
                    .and_then(|details| details.get("phase"))
                    .and_then(serde_json::Value::as_str)
                    == Some("apply_required")
            }),
            change_required,
            "recorded image: {:?}, running image: {running_digest}",
            record.image_resolution
        );
    }
    Ok(())
}

#[tokio::test]
async fn image_replace_apply_preserves_resources_and_commits_new_image() -> TestResult {
    let old_image = "ghcr.io/liquescent-development/gascan/workspace:old@sha256:\
                     bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let runtime_path = root.join("runtime.json");
    let state_path = root.join("state.db");
    let capabilities = FakeRuntime::default().capabilities().await?;
    let initial_runtime = FakeRuntime::persistent(capabilities.clone(), &runtime_path).await?;
    let desired = networked_spec("image-replace-success", root)?;
    let initial_service = SandboxService::new(
        initial_runtime.clone(),
        gascand::Store::open(&state_path)?,
        Arc::new(NoopProvisioner),
    );
    initial_service.up(UpRequest::new(desired.clone())).await?;
    let mut record = initial_service
        .status(desired.id())?
        .ok_or("sandbox record")?;
    record.image_resolution = Some(gascand::ImageResolution::new(
        1,
        json!({"digest": old_image}),
    ));
    initial_service.store().put_sandbox(&record)?;
    let before_resources = initial_runtime
        .list_resources()
        .await?
        .into_iter()
        .map(|resource| (resource.kind(), resource.name().to_owned()))
        .collect::<Vec<_>>();
    drop(initial_service);
    drop(initial_runtime);
    rewrite_runtime_image(&runtime_path, old_image)?;

    let runtime = FakeRuntime::persistent(capabilities, &runtime_path).await?;
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(&state_path)?,
        Arc::new(NoopProvisioner),
    );
    let approved_image = PolicyCompiler::compile(desired.clone(), &runtime.capabilities().await?)?
        .image()
        .to_owned();

    service.apply(UpRequest::new(desired.clone())).await?;

    let calls = runtime.calls().await;
    let prepare = calls
        .iter()
        .position(
            |call| matches!(call, RuntimeCall::PrepareImage(image) if image == &approved_image),
        )
        .ok_or("prepare image")?;
    let list = calls
        .iter()
        .enumerate()
        .skip(prepare + 1)
        .find_map(|(index, call)| matches!(call, RuntimeCall::ListResources).then_some(index))
        .ok_or("list resources")?;
    let stop = calls
        .iter()
        .position(|call| matches!(call, RuntimeCall::Stop(id) if id == desired.id()))
        .ok_or("stop old container")?;
    let remove = calls
        .iter()
        .position(|call| {
            matches!(call, RuntimeCall::Remove(request)
                if request.resources().len() == 1
                    && request.resources()[0].kind() == ResourceKind::Container)
        })
        .ok_or("remove old container")?;
    let create = calls
        .iter()
        .position(|call| matches!(call, RuntimeCall::CreateContainer(_)))
        .ok_or("create replacement container")?;
    assert!(prepare < list && list < stop && stop < remove && remove < create);
    let after_resources = runtime
        .list_resources()
        .await?
        .into_iter()
        .map(|resource| (resource.kind(), resource.name().to_owned()))
        .collect::<Vec<_>>();
    assert_eq!(after_resources, before_resources);
    assert_eq!(
        runtime
            .inspect(desired.id())
            .await?
            .ok_or("replacement runtime")?
            .image,
        approved_image
    );
    assert_eq!(
        service
            .status(desired.id())?
            .ok_or("replacement record")?
            .image_resolution
            .and_then(|resolution| resolution.details["digest"].as_str().map(ToOwned::to_owned)),
        Some(approved_image)
    );
    assert_eq!(
        service
            .latest_operation()?
            .ok_or("completed operation")?
            .status,
        OperationStatus::Completed
    );
    Ok(())
}

#[tokio::test]
async fn image_replace_up_reports_apply_required_without_runtime_mutation() -> TestResult {
    let old_image = "ghcr.io/liquescent-development/gascan/workspace:old@sha256:\
                     bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    for recorded in ["approved", "invalid"] {
        let root = tempfile::tempdir()?;
        let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
        let runtime_path = root.join("runtime.json");
        let state_path = root.join("state.db");
        let capabilities = FakeRuntime::default().capabilities().await?;
        let initial_runtime = FakeRuntime::persistent(capabilities.clone(), &runtime_path).await?;
        let desired = spec("image-replace-up", root)?;
        let initial_service = SandboxService::new(
            initial_runtime.clone(),
            gascand::Store::open(&state_path)?,
            Arc::new(NoopProvisioner),
        );
        initial_service.up(UpRequest::new(desired.clone())).await?;
        let approved_image = initial_runtime
            .inspect(desired.id())
            .await?
            .ok_or("initial runtime")?
            .image;
        let mut record = initial_service
            .status(desired.id())?
            .ok_or("sandbox record")?;
        if recorded == "invalid" {
            record.image_resolution = Some(gascand::ImageResolution::new(
                1,
                json!({"digest":"workspace:latest"}),
            ));
            initial_service.store().put_sandbox(&record)?;
        }
        drop(initial_service);
        drop(initial_runtime);
        rewrite_runtime_image(&runtime_path, old_image)?;

        let runtime = FakeRuntime::persistent(capabilities, &runtime_path).await?;
        let service = SandboxService::new(
            runtime.clone(),
            gascand::Store::open(&state_path)?,
            Arc::new(NoopProvisioner),
        );
        let operation = service.up(UpRequest::new(desired)).await?;
        let events = service.store().operation_events(operation.id)?;

        assert!(runtime.calls().await.iter().all(|call| {
            matches!(
                call,
                RuntimeCall::Capabilities | RuntimeCall::Inspect(_) | RuntimeCall::ListResources
            )
        }));
        assert!(events.iter().any(|event| {
            event.details.as_ref().is_some_and(|details| {
                details["phase"] == "apply_required"
                    && details["reason"] == "image_changed"
                    && details["running_image"] == old_image
                    && details["approved_image"] == approved_image
                    && if recorded == "invalid" {
                        details["recorded_image"].is_null()
                    } else {
                        details["recorded_image"] == approved_image
                    }
            })
        }));
    }
    Ok(())
}

#[tokio::test]
async fn image_replace_rejects_runtime_image_change_after_preflight() -> TestResult {
    let old_image = "ghcr.io/liquescent-development/gascan/workspace:old@sha256:\
                     bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let changed_image = "ghcr.io/liquescent-development/gascan/workspace:changed@sha256:\
                         cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let runtime_path = root.join("runtime.json");
    let state_path = root.join("state.db");
    let capabilities = FakeRuntime::default().capabilities().await?;
    let initial_runtime = FakeRuntime::persistent(capabilities.clone(), &runtime_path).await?;
    let desired = spec("image-change-after-preflight", root)?;
    let initial_service = SandboxService::new(
        initial_runtime.clone(),
        gascand::Store::open(&state_path)?,
        Arc::new(NoopProvisioner),
    );
    initial_service.up(UpRequest::new(desired.clone())).await?;
    let mut record = initial_service
        .status(desired.id())?
        .ok_or("sandbox record")?;
    record.image_resolution = Some(gascand::ImageResolution::new(
        1,
        json!({"digest": old_image}),
    ));
    initial_service.store().put_sandbox(&record)?;
    drop(initial_service);
    drop(initial_runtime);
    rewrite_runtime_image(&runtime_path, old_image)?;

    let runtime = FakeRuntime::persistent(capabilities, &runtime_path).await?;
    runtime
        .change_image_on_prepare(changed_image.to_owned())
        .await;
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(&state_path)?,
        Arc::new(NoopProvisioner),
    );

    let error = match service.apply(UpRequest::new(desired)).await {
        Ok(_) => return Err("changed predecessor image unexpectedly applied".into()),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        ServiceError::ImageUpgradeRequired {
            ref current,
            ref requested,
            ..
        } if current == changed_image
            && requested == include_str!("../../../images/workspace/approved-image.txt")
    ));
    let operation = service.latest_operation()?.ok_or("failed operation")?;
    assert_eq!(
        operation.error_code.as_deref(),
        Some(gascan_proto::error_code::IMAGE_UPGRADE_REQUIRED)
    );
    assert_eq!(
        operation
            .error_details
            .as_ref()
            .and_then(|details| details.get("current"))
            .and_then(serde_json::Value::as_str),
        Some(changed_image)
    );
    assert!(runtime.calls().await.iter().all(|call| {
        !matches!(
            call,
            RuntimeCall::Stop(_) | RuntimeCall::Remove(_) | RuntimeCall::CreateContainer(_)
        )
    }));
    Ok(())
}

#[tokio::test]
async fn image_replace_failures_restore_previous_image_and_resources() -> TestResult {
    let old_image = "ghcr.io/liquescent-development/gascan/workspace:old@sha256:\
                     bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    for (boundary, fail_provision, fail_health) in [
        (Some(FailureBoundary::Stop), false, false),
        (Some(FailureBoundary::Remove), false, false),
        (Some(FailureBoundary::CreateContainer), false, false),
        (
            Some(FailureBoundary::CreateContainerAfterMutation),
            false,
            false,
        ),
        (Some(FailureBoundary::Start), false, false),
        (None, true, false),
        (None, false, true),
    ] {
        let root = tempfile::tempdir()?;
        let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
        let runtime_path = root.join("runtime.json");
        let state_path = root.join("state.db");
        let capabilities = FakeRuntime::default().capabilities().await?;
        let initial_runtime = FakeRuntime::persistent(capabilities.clone(), &runtime_path).await?;
        let desired = networked_spec("image-replace-rollback", root)?;
        let provisioner = Arc::new(ControlledProvisioner::default());
        let initial_service = SandboxService::new(
            initial_runtime.clone(),
            gascand::Store::open(&state_path)?,
            provisioner.clone(),
        );
        initial_service.up(UpRequest::new(desired.clone())).await?;
        let mut record = initial_service
            .status(desired.id())?
            .ok_or("sandbox record")?;
        record.image_resolution = Some(gascand::ImageResolution::new(
            1,
            json!({"digest": old_image}),
        ));
        initial_service.store().put_sandbox(&record)?;
        let before_resources = initial_runtime
            .list_resources()
            .await?
            .into_iter()
            .map(|resource| (resource.kind(), resource.name().to_owned()))
            .collect::<Vec<_>>();
        drop(initial_service);
        drop(initial_runtime);
        rewrite_runtime_image(&runtime_path, old_image)?;

        let runtime = FakeRuntime::persistent(capabilities, &runtime_path).await?;
        if let Some(boundary) = boundary {
            runtime.inject_failure(boundary).await;
        }
        provisioner
            .fail_provision
            .store(fail_provision, Ordering::SeqCst);
        provisioner.fail_health.store(fail_health, Ordering::SeqCst);
        let service = SandboxService::new(
            runtime.clone(),
            gascand::Store::open(&state_path)?,
            provisioner,
        );

        assert!(
            service
                .apply(UpRequest::new(desired.clone()))
                .await
                .is_err()
        );

        assert_eq!(
            runtime
                .inspect(desired.id())
                .await?
                .ok_or("rolled back runtime")?
                .image,
            old_image,
            "boundary {boundary:?}, provision={fail_provision}, health={fail_health}"
        );
        assert_eq!(
            service
                .status(desired.id())?
                .ok_or("rolled back record")?
                .image_resolution
                .and_then(|resolution| resolution.details["digest"]
                    .as_str()
                    .map(ToOwned::to_owned)),
            Some(old_image.to_owned())
        );
        assert_eq!(
            service
                .latest_operation()?
                .ok_or("failed operation")?
                .status,
            OperationStatus::Failed
        );
        let after_resources = runtime
            .list_resources()
            .await?
            .into_iter()
            .map(|resource| (resource.kind(), resource.name().to_owned()))
            .collect::<Vec<_>>();
        assert_eq!(after_resources, before_resources);
        assert!(runtime.calls().await.iter().all(|call| {
            !matches!(call, RuntimeCall::Remove(request)
            if request.resources().iter().any(|resource| {
                matches!(resource.kind(), ResourceKind::Volume | ResourceKind::Network)
            }))
        }));
        if boundary == Some(FailureBoundary::CreateContainerAfterMutation) {
            let calls = runtime.calls().await;
            let failed_create = calls
                .iter()
                .position(|call| matches!(call, RuntimeCall::CreateContainer(_)))
                .ok_or("failed replacement create")?;
            let partial_remove = calls[failed_create + 1..]
                .iter()
                .position(|call| matches!(call, RuntimeCall::Remove(_)))
                .map(|offset| failed_create + 1 + offset)
                .ok_or("partial replacement remove")?;
            assert!(
                calls[failed_create + 1..partial_remove]
                    .iter()
                    .all(|call| !matches!(call, RuntimeCall::ListResources)),
                "partial cleanup rediscovered evidence instead of using CreateFailure"
            );
            assert!(matches!(
                &calls[partial_remove],
                RuntimeCall::Remove(request)
                    if request.resources().len() == 1
                        && request.resources()[0].kind() == ResourceKind::Container
            ));
        }
    }
    Ok(())
}

#[tokio::test]
async fn canonical_runtime_image_identity_starts_without_apply_required() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let runtime_path = root.join("runtime.json");
    let state_path = root.join("state.db");
    let desired = spec("canonical-runtime-image", root)?;
    let capabilities = FakeRuntime::default().capabilities().await?;
    let approved = PolicyCompiler::compile(desired.clone(), &capabilities)?
        .image()
        .to_owned();
    let (tagged_name, digest) = approved
        .rsplit_once('@')
        .ok_or("approved image lacks digest")?;
    let tag_separator = tagged_name.rfind(':').ok_or("approved image lacks a tag")?;
    let canonical = format!("{}@{digest}", &tagged_name[..tag_separator]);

    let initial_runtime = FakeRuntime::persistent(capabilities.clone(), &runtime_path).await?;
    let initial_service = SandboxService::new(
        initial_runtime.clone(),
        gascand::Store::open(&state_path)?,
        Arc::new(NoopProvisioner),
    );
    initial_service.up(UpRequest::new(desired.clone())).await?;
    initial_runtime.stop(desired.id()).await?;
    drop(initial_service);
    drop(initial_runtime);
    rewrite_runtime_image(&runtime_path, &canonical)?;

    let runtime = FakeRuntime::persistent(capabilities, &runtime_path).await?;
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(&state_path)?,
        Arc::new(NoopProvisioner),
    );
    let operation = service.up(UpRequest::new(desired.clone())).await?;
    let events = service.store().operation_events(operation.id)?;

    assert!(events.iter().all(|event| {
        event
            .details
            .as_ref()
            .and_then(|details| details["phase"].as_str())
            != Some("apply_required")
    }));
    assert!(
        runtime
            .calls()
            .await
            .iter()
            .any(|call| matches!(call, RuntimeCall::Start(id) if id == desired.id()))
    );
    assert_eq!(
        runtime
            .inspect(desired.id())
            .await?
            .ok_or("runtime sandbox")?
            .state,
        gascan_core::runtime::ContainerState::Running
    );
    Ok(())
}

#[tokio::test]
async fn image_replace_rejects_missing_or_extra_owned_container_evidence() -> TestResult {
    let old_image = "ghcr.io/liquescent-development/gascan/workspace:old@sha256:\
                     bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    for extra in [false, true] {
        let root = tempfile::tempdir()?;
        let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
        let desired = spec("image-replace-container-evidence", root)?;
        let runtime = FakeRuntime::default();
        let service = SandboxService::new(
            runtime.clone(),
            gascand::Store::open(root.join("state.db"))?,
            Arc::new(NoopProvisioner),
        );
        service.up(UpRequest::new(desired.clone())).await?;
        let mut record = service.status(desired.id())?.ok_or("record")?;
        record.image_resolution = Some(gascand::ImageResolution::new(
            1,
            json!({"digest":old_image}),
        ));
        service.store().put_sandbox(&record)?;
        if extra {
            runtime
                .seed_container_resource(
                    "extra-owned-container",
                    desired.id().clone(),
                    ResourceOwnership::GasCanOwned,
                )
                .await?;
        } else {
            runtime
                .forget_resource(&ResourceIdentity::new(
                    ResourceKind::Container,
                    desired.id().to_string(),
                )?)
                .await;
        }
        let before = runtime.calls().await.len();

        assert!(service.apply(UpRequest::new(desired)).await.is_err());

        assert!(runtime.calls().await[before..].iter().all(|call| {
            !matches!(
                call,
                RuntimeCall::Stop(_)
                    | RuntimeCall::Remove(_)
                    | RuntimeCall::CreateContainer(_)
                    | RuntimeCall::Start(_)
                    | RuntimeCall::Exec(_)
            )
        }));
    }
    Ok(())
}

#[tokio::test]
async fn image_replace_rollback_rejects_extra_owned_container_evidence_before_cleanup() -> TestResult
{
    let old_image = "ghcr.io/liquescent-development/gascan/workspace:old@sha256:\
                     bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let desired = spec("image-replace-rollback-container-evidence", root)?;
    let runtime = FakeRuntime::default();
    let initial_service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    initial_service.up(UpRequest::new(desired.clone())).await?;
    let mut record = initial_service.status(desired.id())?.ok_or("record")?;
    record.image_resolution = Some(gascand::ImageResolution::new(
        1,
        json!({"digest":old_image}),
    ));
    initial_service.store().put_sandbox(&record)?;
    drop(initial_service);
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(ExtraContainerProvisioner {
            runtime: runtime.clone(),
            id: desired.id().clone(),
        }),
    );

    assert!(service.apply(UpRequest::new(desired)).await.is_err());

    let removes = runtime
        .calls()
        .await
        .into_iter()
        .filter(|call| matches!(call, RuntimeCall::Remove(_)))
        .count();
    assert_eq!(removes, 1, "rollback mutated ambiguous container evidence");
    Ok(())
}

#[tokio::test]
async fn image_replace_preserves_primary_and_rollback_errors() -> TestResult {
    let old_image = "ghcr.io/liquescent-development/gascan/workspace:old@sha256:\
                     bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let runtime_path = root.join("runtime.json");
    let state_path = root.join("state.db");
    let capabilities = FakeRuntime::default().capabilities().await?;
    let initial_runtime = FakeRuntime::persistent(capabilities.clone(), &runtime_path).await?;
    let desired = spec("image-replace-double-failure", root)?;
    let initial_service = SandboxService::new(
        initial_runtime.clone(),
        gascand::Store::open(&state_path)?,
        Arc::new(NoopProvisioner),
    );
    initial_service.up(UpRequest::new(desired.clone())).await?;
    let mut record = initial_service
        .status(desired.id())?
        .ok_or("sandbox record")?;
    record.image_resolution = Some(gascand::ImageResolution::new(
        1,
        json!({"digest": old_image}),
    ));
    initial_service.store().put_sandbox(&record)?;
    drop(initial_service);
    drop(initial_runtime);
    rewrite_runtime_image(&runtime_path, old_image)?;

    let runtime = FakeRuntime::persistent(capabilities, &runtime_path).await?;
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(&state_path)?,
        Arc::new(RollbackFailingProvisioner {
            runtime: runtime.clone(),
        }),
    );

    let error = match service.apply(UpRequest::new(desired)).await {
        Ok(_) => return Err("replacement unexpectedly succeeded".into()),
        Err(error) => error,
    };

    assert!(
        error
            .to_string()
            .contains("injected replacement provision failure")
    );
    assert!(error.to_string().contains("rollback failed"));
    assert!(error.to_string().contains("injected failure at remove"));
    assert_eq!(
        service
            .latest_operation()?
            .ok_or("failed operation")?
            .status,
        OperationStatus::Failed
    );
    Ok(())
}

#[tokio::test]
async fn image_replace_database_commit_failure_restores_runtime_and_resolution() -> TestResult {
    let old_image = "ghcr.io/liquescent-development/gascan/workspace:old@sha256:\
                     bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let runtime_path = root.join("runtime.json");
    let state_path = root.join("state.db");
    let capabilities = FakeRuntime::default().capabilities().await?;
    let initial_runtime = FakeRuntime::persistent(capabilities.clone(), &runtime_path).await?;
    let desired = spec("image-replace-database-failure", root)?;
    let id = desired.id().clone();
    let initial_service = SandboxService::new(
        initial_runtime.clone(),
        gascand::Store::open(&state_path)?,
        Arc::new(NoopProvisioner),
    );
    initial_service.up(UpRequest::new(desired.clone())).await?;
    let mut record = initial_service
        .status(desired.id())?
        .ok_or("sandbox record")?;
    record.image_resolution = Some(gascand::ImageResolution::new(
        1,
        json!({"digest": old_image}),
    ));
    initial_service.store().put_sandbox(&record)?;
    drop(initial_service);
    drop(initial_runtime);
    rewrite_runtime_image(&runtime_path, old_image)?;

    let runtime = FakeRuntime::persistent(capabilities, &runtime_path).await?;
    let store = gascand::Store::open(&state_path)?;
    let service = SandboxService::new(
        runtime.clone(),
        store.clone(),
        Arc::new(TerminalReadFailingProvisioner {
            store: store.clone(),
        }),
    );

    let error = match service.apply(UpRequest::new(desired)).await {
        Ok(_) => return Err("database race unexpectedly succeeded".into()),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("injected operation event read failure")
    );
    assert_eq!(
        runtime
            .inspect(&id)
            .await?
            .ok_or("rolled back runtime")?
            .image,
        old_image
    );
    assert_eq!(
        service
            .status(&id)?
            .ok_or("rolled back record")?
            .image_resolution
            .and_then(|resolution| resolution.details["digest"].as_str().map(ToOwned::to_owned)),
        Some(old_image.to_owned())
    );
    assert_eq!(
        service
            .latest_operation()?
            .ok_or("terminal operation")?
            .status,
        OperationStatus::Failed
    );
    Ok(())
}

#[tokio::test]
async fn up_after_destroy_reprovisions_the_fresh_runtime() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("recreate-hooks", root, Manifest::load(root)?);
    let id = make_spec()?.id().clone();
    let provisioner = Arc::new(ControlledProvisioner::default());
    let service = SandboxService::new(
        FakeRuntime::default(),
        gascand::Store::open(root.join("state.db"))?,
        provisioner.clone(),
    );
    service.up(UpRequest::new(make_spec()?)).await?;
    service.destroy(&id).await?;
    service.up(UpRequest::new(make_spec()?)).await?;
    assert_eq!(provisioner.provisions.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn provision_and_health_failures_roll_back_new_resources() -> TestResult {
    for health in [false, true] {
        let root = tempfile::tempdir()?;
        let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
        let spec = SandboxSpec::from_root("hook-failure", root, Manifest::load(root)?)?;
        let id = spec.id().clone();
        let runtime = FakeRuntime::default();
        let provisioner = Arc::new(ControlledProvisioner::default());
        provisioner.fail_provision.store(!health, Ordering::SeqCst);
        provisioner.fail_health.store(health, Ordering::SeqCst);
        let service = SandboxService::new(
            runtime.clone(),
            gascand::Store::open(root.join("state.db"))?,
            provisioner,
        );
        assert!(service.up(UpRequest::new(spec)).await.is_err());
        assert!(runtime.inspect(&id).await?.is_none());
        assert!(runtime.list_resources().await?.is_empty());
        assert!(service.store().pending_operations()?.is_empty());
        let calls = runtime.calls().await;
        let started = calls
            .iter()
            .position(|call| matches!(call, RuntimeCall::Start(call_id) if call_id == &id))
            .ok_or("start call")?;
        let stopped = calls
            .iter()
            .position(|call| matches!(call, RuntimeCall::Stop(call_id) if call_id == &id))
            .ok_or("rollback stop call")?;
        let removed = calls
            .iter()
            .position(|call| matches!(call, RuntimeCall::Remove(_)))
            .ok_or("rollback remove call")?;
        assert!(started < stopped && stopped < removed);
        assert!(matches!(
            &calls[stopped - 1],
            RuntimeCall::Inspect(call_id) if call_id == &id
        ));
    }
    Ok(())
}

#[tokio::test]
async fn rollback_failure_preserves_provision_error_and_stops_before_remove() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let spec = SandboxSpec::from_root("rollback-diagnostic", root, Manifest::load(root)?)?;
    let id = spec.id().clone();
    let runtime = FakeRuntime::failing_once(FailureBoundary::Remove);
    let provisioner = Arc::new(ControlledProvisioner::default());
    provisioner.fail_provision.store(true, Ordering::SeqCst);
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        provisioner,
    );

    let error = match service.up(UpRequest::new(spec)).await {
        Ok(_) => return Err("provisioning unexpectedly succeeded".into()),
        Err(error) => error,
    };
    assert_eq!(
        error.to_string(),
        "provisioning failed: injected provision failure; rollback failed: injected failure at remove"
    );
    let calls = runtime.calls().await;
    let stopped = calls
        .iter()
        .position(|call| matches!(call, RuntimeCall::Stop(call_id) if call_id == &id))
        .ok_or("rollback stop call")?;
    let removed = calls
        .iter()
        .position(|call| matches!(call, RuntimeCall::Remove(_)))
        .ok_or("rollback remove call")?;
    assert!(stopped < removed);
    assert!(matches!(
        &calls[stopped - 1],
        RuntimeCall::Inspect(call_id) if call_id == &id
    ));
    assert_eq!(
        runtime.inspect(&id).await?.ok_or("retained runtime")?.state,
        gascan_core::runtime::ContainerState::Stopped
    );
    let operation = service.latest_operation()?.ok_or("operation")?;
    assert_eq!(operation.error_code.as_deref(), Some("provision_failed"));
    assert_eq!(
        operation
            .error_details
            .as_ref()
            .ok_or("operation error details")?["message"],
        "provisioning failed: injected provision failure; rollback failed: injected failure at remove"
    );
    Ok(())
}

#[tokio::test]
async fn failed_apply_retains_prior_setup_and_tool_resolutions() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("apply-retain", root, Manifest::load(root)?);
    let id = make_spec()?.id().clone();
    let runtime = FakeRuntime::default();
    let provisioner = Arc::new(ControlledProvisioner::default());
    let service = SandboxService::new(
        runtime,
        gascand::Store::open(root.join("state.db"))?,
        provisioner.clone(),
    );
    service.up(UpRequest::new(make_spec()?)).await?;
    let prior = service.status(&id)?.ok_or("record")?;
    provisioner.fail_provision.store(true, Ordering::SeqCst);
    std::fs::write(
        root.join("gascan.toml"),
        "version = 1\n[tools]\nnode = '22'\n",
    )?;

    assert!(service.apply(UpRequest::new(make_spec()?)).await.is_err());
    assert!(service.store().pending_operations()?.is_empty());
    let retained = service.status(&id)?.ok_or("record")?;
    assert_eq!(retained.setup_resolution, prior.setup_resolution);
    assert_eq!(retained.tool_resolution, prior.tool_resolution);
    Ok(())
}

#[tokio::test]
async fn synchronous_runtime_failures_after_begin_are_terminal_not_pending() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("terminal-errors", root, Manifest::load(root)?);
    let id = make_spec()?.id().clone();
    let runtime = FakeRuntime::default();
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    service.up(UpRequest::new(make_spec()?)).await?;
    service.stop(&id).await?;

    runtime.inject_failure(FailureBoundary::Start).await;
    assert!(service.start(&id).await.is_err());
    assert!(service.store().pending_operations()?.is_empty());
    service.start(&id).await?;
    runtime.inject_failure(FailureBoundary::Stop).await;
    assert!(service.stop(&id).await.is_err());
    assert!(service.store().pending_operations()?.is_empty());
    runtime.inject_failure(FailureBoundary::Inspect).await;
    std::fs::write(
        root.join("gascan.toml"),
        "version = 1\n[tools]\nnode = '22'\n",
    )?;
    assert!(service.apply(UpRequest::new(make_spec()?)).await.is_err());
    assert!(service.store().pending_operations()?.is_empty());
    runtime.inject_failure(FailureBoundary::Remove).await;
    assert!(service.destroy(&id).await.is_err());
    assert!(service.store().pending_operations()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn stopped_up_auto_starts_and_apply_status_list_complete() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("surface", root, Manifest::load(root)?);
    let id = make_spec()?.id().clone();
    let runtime = FakeRuntime::default();
    let provisioner = Arc::new(ControlledProvisioner::default());
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        provisioner,
    );
    service.up(UpRequest::new(make_spec()?)).await?;
    runtime.stop(&id).await?;
    service.up(UpRequest::new(make_spec()?)).await?;
    assert_eq!(
        runtime.inspect(&id).await?.ok_or("runtime")?.state,
        gascan_core::runtime::ContainerState::Running
    );
    let applied = service.apply(UpRequest::new(make_spec()?)).await?;
    assert_eq!(
        service.store().latest_operation()?.ok_or("operation")?.id,
        applied.id
    );
    assert_eq!(
        service.status(&id)?.ok_or("status")?.actual_state,
        gascand::ActualState::Running
    );
    assert!(service.list()?.iter().any(|record| record.id == id));
    Ok(())
}

#[tokio::test]
async fn event_stream_matches_ordered_durable_events_and_receiver_drop_does_not_deadlock()
-> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("events", root, Manifest::load(root)?);
    let id = make_spec()?.id().clone();
    let service = SandboxService::new(
        FakeRuntime::default(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    let mut operation = service.up(UpRequest::new(make_spec()?)).await?;
    let durable = service.store().operation_events(operation.id)?;
    let mut streamed = Vec::new();
    while let Some(event) = operation.events.recv().await {
        streamed.push(event);
    }
    assert_eq!(streamed, durable);
    assert!(
        streamed
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
    assert_eq!(
        streamed.last().map(|event| event.status),
        Some(OperationStatus::Completed)
    );
    let dropped = service.stop(&id).await?;
    drop(dropped.events);
    service.start(&id).await?;
    Ok(())
}

async fn volume_names(
    runtime: &FakeRuntime,
    spec: SandboxSpec,
) -> Result<Vec<String>, Box<dyn Error>> {
    Ok(
        PolicyCompiler::compile(spec, &runtime.capabilities().await?)?
            .volumes()
            .iter()
            .map(|volume| volume.name.clone())
            .collect(),
    )
}

#[tokio::test]
async fn failed_create_preserves_preexisting_volume_and_removes_only_new_resources() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("rollback-volumes", root, Manifest::load(root)?);
    let runtime = FakeRuntime::failing_once(FailureBoundary::Start);
    let id = make_spec()?.id().clone();
    let names = volume_names(&runtime, make_spec()?).await?;
    runtime
        .seed_volume(&names[0], Some(id), ResourceOwnership::GasCanOwned)
        .await?;
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );

    assert!(service.up(UpRequest::new(make_spec()?)).await.is_err());
    assert!(runtime.volume_exists(&names[0]).await);
    for name in &names[1..] {
        assert!(!runtime.volume_exists(name).await);
    }
    assert!(
        runtime
            .list_resources()
            .await?
            .iter()
            .all(|resource| resource.kind() == ResourceKind::Volume && resource.name() == names[0])
    );
    Ok(())
}

#[tokio::test]
async fn failed_networked_up_rolls_back_the_managed_network() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let spec = networked_spec("network-rollback", root)?;
    let network = PolicyCompiler::managed_network_name(spec.id());
    let runtime = FakeRuntime::failing_once(FailureBoundary::Start);
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );

    assert!(service.up(UpRequest::new(spec)).await.is_err());
    assert!(!runtime.network_exists(&network).await);
    Ok(())
}

#[tokio::test]
async fn destroy_removes_the_managed_network_after_successful_networked_up() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let spec = networked_spec("network-destroy", root)?;
    let id = spec.id().clone();
    let network = PolicyCompiler::managed_network_name(&id);
    let runtime = FakeRuntime::default();
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );

    service.up(UpRequest::new(spec)).await?;
    assert!(runtime.network_exists(&network).await);
    service.destroy(&id).await?;
    assert!(!runtime.network_exists(&network).await);
    Ok(())
}

#[tokio::test]
async fn successful_create_accepts_expected_preexisting_owned_volume_without_claiming_it_created()
-> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("preexisting-success", root, Manifest::load(root)?);
    let runtime = FakeRuntime::default();
    let id = make_spec()?.id().clone();
    let names = volume_names(&runtime, make_spec()?).await?;
    runtime
        .seed_volume(&names[0], Some(id), ResourceOwnership::GasCanOwned)
        .await?;
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    service.up(UpRequest::new(make_spec()?)).await?;
    assert!(runtime.volume_exists(&names[0]).await);
    Ok(())
}

#[tokio::test]
async fn foreign_volume_collision_is_refused_and_preserved() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("volume-collision", root, Manifest::load(root)?);
    let runtime = FakeRuntime::default();
    let names = volume_names(&runtime, make_spec()?).await?;
    runtime
        .seed_volume(&names[0], None, ResourceOwnership::Foreign)
        .await?;
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );

    let error = match service.up(UpRequest::new(make_spec()?)).await {
        Ok(_) => return Err("volume collision unexpectedly succeeded".into()),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("volume exists with different ownership")
    );
    assert!(runtime.volume_exists(&names[0]).await);
    Ok(())
}

#[tokio::test]
async fn partial_create_collision_rolls_back_only_resources_created_by_failed_call() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("partial-collision", root, Manifest::load(root)?);
    let runtime = FakeRuntime::default();
    let names = volume_names(&runtime, make_spec()?).await?;
    runtime
        .seed_volume(&names[1], None, ResourceOwnership::Foreign)
        .await?;
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    assert!(service.up(UpRequest::new(make_spec()?)).await.is_err());
    assert!(!runtime.volume_exists(&names[0]).await);
    assert!(runtime.volume_exists(&names[1]).await);
    assert_eq!(
        service.latest_operation()?.ok_or("operation")?.status,
        OperationStatus::Failed
    );
    Ok(())
}

#[tokio::test]
async fn destroy_removes_exact_owned_resources_and_retains_foreign_inventory() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("destroy-exact", root, Manifest::load(root)?);
    let runtime = FakeRuntime::default();
    let id = make_spec()?.id().clone();
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    service.up(UpRequest::new(make_spec()?)).await?;
    runtime
        .seed_volume(
            "foreign-neighbor",
            Some(id.clone()),
            ResourceOwnership::Foreign,
        )
        .await?;

    service.destroy(&id).await?;

    let inventory = runtime.list_resources().await?;
    assert_eq!(inventory.len(), 1);
    assert_eq!(inventory[0].name(), "foreign-neighbor");
    assert_eq!(inventory[0].ownership(), ResourceOwnership::Foreign);
    Ok(())
}

#[tokio::test]
async fn destroy_retains_extra_owned_volume_with_known_sandbox_association() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("destroy-extra", root, Manifest::load(root)?);
    let id = make_spec()?.id().clone();
    let runtime = FakeRuntime::default();
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    service.up(UpRequest::new(make_spec()?)).await?;
    runtime
        .seed_volume(
            "gascan-extra-owned",
            Some(id.clone()),
            ResourceOwnership::GasCanOwned,
        )
        .await?;
    service.destroy(&id).await?;
    assert!(runtime.volume_exists("gascan-extra-owned").await);
    let report = service.reconcile().await?;
    assert!(report.findings.iter().any(|finding| matches!(finding, gascand::ReconcileFinding::UnknownOwned(resource) if resource.name() == "gascan-extra-owned")));
    Ok(())
}

async fn wait_for_start_calls(runtime: &FakeRuntime, expected: usize) -> TestResult {
    for _ in 0..10_000 {
        let count = runtime
            .calls()
            .await
            .iter()
            .filter(|call| matches!(call, gascan_core::runtime::RuntimeCall::Start(_)))
            .count();
        if count >= expected {
            return Ok(());
        }
        tokio::task::yield_now().await;
    }
    Err(format!("timed out waiting for {expected} start calls").into())
}

#[tokio::test]
async fn same_key_mutations_serialize_at_the_runtime_boundary() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let first = SandboxSpec::from_root("same-key", root, Manifest::load(root)?)?;
    let second = SandboxSpec::from_root("same-key", root, Manifest::load(root)?)?;
    let runtime = FakeRuntime::default();
    runtime.gate(FailureBoundary::Start).await;
    let service = Arc::new(SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    ));
    let one = tokio::spawn({
        let service = service.clone();
        async move { service.up(UpRequest::new(first)).await }
    });
    wait_for_start_calls(&runtime, 1).await?;
    let two = tokio::spawn({
        let service = service.clone();
        async move { service.up(UpRequest::new(second)).await }
    });
    tokio::task::yield_now().await;
    assert_eq!(
        runtime
            .calls()
            .await
            .iter()
            .filter(|call| matches!(call, gascan_core::runtime::RuntimeCall::Start(_)))
            .count(),
        1
    );
    runtime.release(FailureBoundary::Start, 1).await;
    one.await??;
    two.await??;
    Ok(())
}

#[tokio::test]
async fn different_keys_reach_the_runtime_concurrently() -> TestResult {
    let one_root = tempfile::tempdir()?;
    let two_root = tempfile::tempdir()?;
    let db_root = tempfile::tempdir()?;
    let one_root = Utf8Path::from_path(one_root.path()).ok_or("utf8 root")?;
    let two_root = Utf8Path::from_path(two_root.path()).ok_or("utf8 root")?;
    let one = SandboxSpec::from_root("one", one_root, Manifest::load(one_root)?)?;
    let two = SandboxSpec::from_root("two", two_root, Manifest::load(two_root)?)?;
    let runtime = FakeRuntime::default();
    runtime.gate(FailureBoundary::Start).await;
    let service = Arc::new(SandboxService::new(
        runtime.clone(),
        gascand::Store::open(db_root.path().join("state.db"))?,
        Arc::new(NoopProvisioner),
    ));
    let first = tokio::spawn({
        let service = service.clone();
        async move { service.up(UpRequest::new(one)).await }
    });
    let second = tokio::spawn({
        let service = service.clone();
        async move { service.up(UpRequest::new(two)).await }
    });
    wait_for_start_calls(&runtime, 2).await?;
    runtime.release(FailureBoundary::Start, 2).await;
    first.await??;
    second.await??;
    Ok(())
}

#[tokio::test]
async fn reconcile_waits_for_live_same_sandbox_mutation_instead_of_terminalizing_it() -> TestResult
{
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let spec = SandboxSpec::from_root("reconcile-live", root, Manifest::load(root)?)?;
    let runtime = FakeRuntime::default();
    runtime.gate(FailureBoundary::Start).await;
    let service = Arc::new(SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    ));
    let up = tokio::spawn({
        let service = service.clone();
        async move { service.up(UpRequest::new(spec)).await }
    });
    wait_for_start_calls(&runtime, 1).await?;
    let reconcile = tokio::spawn({
        let service = service.clone();
        async move { service.reconcile().await }
    });
    tokio::task::yield_now().await;
    assert!(!reconcile.is_finished());
    runtime.release(FailureBoundary::Start, 1).await;
    up.await??;
    reconcile.await??;
    assert_eq!(
        service.latest_operation()?.ok_or("operation")?.status,
        OperationStatus::Completed
    );
    Ok(())
}

#[tokio::test]
async fn failed_start_rolls_back_new_sandbox_and_records_failure() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let manifest = Manifest::load(root)?;
    let spec = SandboxSpec::from_root("lifecycle", root, manifest)?;
    let runtime = FakeRuntime::failing_once(FailureBoundary::Start);
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );

    assert!(service.up(UpRequest::new(spec)).await.is_err());
    assert!(
        runtime
            .inspect(&service.list()?.first().ok_or("record")?.id)
            .await?
            .is_none()
    );
    assert_eq!(
        service.latest_operation()?.ok_or("operation")?.status,
        OperationStatus::Failed
    );
    Ok(())
}

#[tokio::test]
async fn repeated_up_is_idempotent() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("repeat", root, Manifest::load(root)?);
    let runtime = FakeRuntime::default();
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    service.up(UpRequest::new(make_spec()?)).await?;
    service.up(UpRequest::new(make_spec()?)).await?;
    assert_eq!(runtime.created_count().await, 1);
    Ok(())
}

#[tokio::test]
async fn start_stop_destroy_are_idempotent_and_emit_terminal_events() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("states", root, Manifest::load(root)?);
    let runtime = FakeRuntime::default();
    let service = SandboxService::new(
        runtime,
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    let id = make_spec()?.id().clone();
    service.up(UpRequest::new(make_spec()?)).await?;
    service.stop(&id).await?;
    service.stop(&id).await?;
    let mut started = service.start(&id).await?;
    let mut statuses = Vec::new();
    while let Some(event) = started.events.recv().await {
        statuses.push(event.status);
    }
    assert_eq!(statuses.last(), Some(&OperationStatus::Completed));
    service.start(&id).await?;
    service.destroy(&id).await?;
    service.destroy(&id).await?;
    assert_eq!(
        service.status(&id)?.ok_or("record")?.actual_state,
        gascand::ActualState::Absent
    );
    Ok(())
}

#[tokio::test]
async fn missing_start_stop_and_apply_are_refused_without_runtime_mutation() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let spec = SandboxSpec::from_root("missing", root, Manifest::load(root)?)?;
    let id = spec.id().clone();
    let runtime = FakeRuntime::default();
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    assert!(service.start(&id).await.is_err());
    assert!(service.stop(&id).await.is_err());
    assert!(service.apply(UpRequest::new(spec)).await.is_err());
    assert!(service.destroy(&id).await.is_err());
    assert_eq!(runtime.created_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn keyed_lock_registry_does_not_retain_finished_sandbox_keys() -> TestResult {
    let db = tempfile::tempdir()?;
    let service = SandboxService::new(
        FakeRuntime::default(),
        gascand::Store::open(db.path().join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    for index in 0..64 {
        let root = tempfile::tempdir()?;
        let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
        let spec = SandboxSpec::from_root(&format!("lock-{index}"), root, Manifest::load(root)?)?;
        service.up(UpRequest::new(spec)).await?;
    }
    assert_eq!(service.keyed_lock_count()?, 0);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn blocked_sqlite_writer_does_not_block_single_tokio_worker() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let path = root.join("state.db");
    let store = gascand::Store::open(&path)?;
    let blocker = rusqlite::Connection::open(&path)?;
    blocker.execute_batch("BEGIN IMMEDIATE")?;
    let spec = SandboxSpec::from_root("blocked-db", root, Manifest::load(root)?)?;
    let service = Arc::new(SandboxService::new(
        FakeRuntime::default(),
        store,
        Arc::new(NoopProvisioner),
    ));
    let operation = tokio::spawn({
        let service = service.clone();
        async move { service.up(UpRequest::new(spec)).await }
    });
    let started = Instant::now();
    tokio::task::yield_now().await;
    let unrelated = Arc::new(AtomicBool::new(false));
    let marker = unrelated.clone();
    tokio::spawn(async move {
        marker.store(true, Ordering::SeqCst);
    });
    tokio::task::yield_now().await;
    assert!(unrelated.load(Ordering::SeqCst));
    assert!(started.elapsed() < Duration::from_secs(1));
    blocker.execute_batch("ROLLBACK")?;
    operation.await??;
    Ok(())
}
