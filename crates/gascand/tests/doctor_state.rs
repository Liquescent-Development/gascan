use gascan_core::doctor::{DoctorFact, DoctorFacts};
use gascan_core::fake_runtime::FakeRuntime;
use gascan_core::sandbox::SandboxId;
use gascan_proto::v1;
use gascan_proto::v1::gas_can_server::GasCan;
use gascand::{
    ActiveSsh, ActivityTracker, ActualState, DesiredState, DoctorState, ManagedSshHost,
    NoopProvisioner, OperationKind, SandboxApi, SandboxRecord, SandboxService, SshPaths,
    SshResolution, Store, ensure_host_identity, publish_openssh_files,
};
use std::net::{IpAddr, Ipv4Addr};
use std::os::unix::fs::PermissionsExt as _;
use std::sync::Arc;
use std::time::Duration;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn doctor_api(
    root: &camino::Utf8Path,
) -> Result<SandboxApi<FakeRuntime>, Box<dyn std::error::Error>> {
    doctor_api_with_report(root, DoctorFacts::all_supported_for_tests().into_report())
}

fn doctor_api_with_report(
    root: &camino::Utf8Path,
    report: gascan_core::doctor::DoctorReport,
) -> Result<SandboxApi<FakeRuntime>, Box<dyn std::error::Error>> {
    let service = SandboxService::new_with_doctor(
        FakeRuntime::default(),
        Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
        report,
    );
    Ok(SandboxApi::new(Arc::new(service), ActivityTracker::new()))
}

fn doctor_workspace(
    path: &std::path::Path,
) -> Result<v1::DoctorRequest, Box<dyn std::error::Error>> {
    Ok(v1::DoctorRequest {
        workspace_result: Some(v1::doctor_request::WorkspaceResult::Workspace(
            path.to_str().ok_or("UTF-8 workspace")?.to_owned(),
        )),
    })
}

#[tokio::test]
async fn doctor_warning_capability_is_available_and_has_no_finding() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = camino::Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
    let mut facts = DoctorFacts::all_supported_for_tests();
    facts.version = DoctorFact::warning("untested 1.2.0");
    let api = doctor_api_with_report(root, facts.into_report())?;

    let response = GasCan::doctor(
        &api,
        tonic::Request::new(doctor_workspace(root.as_std_path())?),
    )
    .await?
    .into_inner();
    let version = response
        .capabilities
        .iter()
        .find(|capability| capability.name == "runtime.version")
        .ok_or("runtime.version capability missing")?;

    assert!(version.available);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&version.detail)?["status"],
        "warning"
    );
    assert!(response.findings.is_empty());
    Ok(())
}

#[tokio::test]
async fn refreshed_ssh_doctor_keeps_warning_loopback_publish_nonblocking() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = camino::Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
    let home = root.join("home");
    std::fs::create_dir(&home)?;
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))?;
    let home = home.canonicalize()?;
    let paths = SshPaths::for_environment(None, Some(home.as_os_str()))?;
    let mut facts = DoctorFacts::all_supported_for_tests();
    facts.loopback_publish = DoctorFact::warning("loopback publication is untested");
    let service = SandboxService::new_with_doctor(
        FakeRuntime::default(),
        Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
        facts.into_report(),
    )
    .with_ssh_paths_for_e2e(paths);
    let api = SandboxApi::new(Arc::new(service), ActivityTracker::new());

    let response = GasCan::doctor(
        &api,
        tonic::Request::new(doctor_workspace(root.as_std_path())?),
    )
    .await?
    .into_inner();
    let native_publish = response
        .capabilities
        .iter()
        .find(|capability| capability.name == "ssh.native_publish")
        .ok_or("ssh.native_publish capability missing")?;

    assert!(native_publish.available);
    assert!(response.findings.is_empty());
    Ok(())
}

#[tokio::test]
async fn doctor_workspace_access_is_scoped_to_each_request() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = camino::Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
    let existing = root.join("existing");
    std::fs::create_dir(&existing)?;
    let missing = root.join("missing");
    let api = doctor_api(root)?;

    let existing = GasCan::doctor(
        &api,
        tonic::Request::new(doctor_workspace(existing.as_std_path())?),
    )
    .await?
    .into_inner();
    let missing = GasCan::doctor(
        &api,
        tonic::Request::new(doctor_workspace(missing.as_std_path())?),
    )
    .await?
    .into_inner();

    let existing_workspace = existing
        .capabilities
        .iter()
        .find(|capability| capability.name == "workspace.access")
        .ok_or("workspace capability missing")?;
    let missing_workspace = missing
        .capabilities
        .iter()
        .find(|capability| capability.name == "workspace.access")
        .ok_or("workspace capability missing")?;
    assert!(existing_workspace.available);
    assert!(!missing_workspace.available);
    assert!(missing_workspace.detail.contains("inaccessible"));
    Ok(())
}

#[tokio::test]
async fn doctor_workspace_reports_an_unknown_check_when_the_caller_omits_it() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = camino::Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
    let api = doctor_api(root)?;

    let response = GasCan::doctor(
        &api,
        tonic::Request::new(v1::DoctorRequest {
            workspace_result: None,
        }),
    )
    .await?
    .into_inner();
    let workspace = response
        .capabilities
        .iter()
        .find(|capability| capability.name == "workspace.access")
        .ok_or("workspace capability missing")?;
    assert!(!workspace.available);
    assert!(workspace.detail.contains("not provided"));
    Ok(())
}

#[tokio::test]
async fn doctor_workspace_rejects_relative_or_malformed_requests() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = camino::Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
    let api = doctor_api(root)?;
    for request in [
        v1::DoctorRequest {
            workspace_result: Some(v1::doctor_request::WorkspaceResult::Workspace(
                "relative".to_owned(),
            )),
        },
        v1::DoctorRequest {
            workspace_result: Some(v1::doctor_request::WorkspaceResult::Workspace(String::new())),
        },
        v1::DoctorRequest {
            workspace_result: Some(v1::doctor_request::WorkspaceResult::Workspace(
                "/valid\0suffix".to_owned(),
            )),
        },
    ] {
        let error = GasCan::doctor(&api, tonic::Request::new(request))
            .await
            .err()
            .ok_or("invalid doctor request unexpectedly succeeded")?;
        assert_eq!(error.code(), tonic::Code::InvalidArgument);
        assert_eq!(error.message(), gascan_proto::error_code::INVALID_REQUEST);
    }
    Ok(())
}

fn sandbox_record(
    id: SandboxId,
    root: &camino::Utf8Path,
    actual_state: ActualState,
    ssh_resolution: Option<SshResolution>,
) -> SandboxRecord {
    SandboxRecord {
        id,
        canonical_root: root.to_owned(),
        desired_state: if actual_state == ActualState::Absent {
            DesiredState::Absent
        } else {
            DesiredState::Running
        },
        actual_state,
        setup_resolution: None,
        tool_resolution: None,
        image_resolution: None,
        storage_resolution: None,
        ssh_resolution,
        last_operation_id: None,
        updated_at_millis: 0,
    }
}

fn enabled_resolution(host: &ManagedSshHost) -> SshResolution {
    SshResolution::new(
        1,
        serde_json::json!({
            "enabled": true,
            "host_key_fingerprint": host.active.host_key_fingerprint,
            "client_key_fingerprint": host.active.client_key_fingerprint,
        }),
    )
}

fn enable_transport(state_path: &std::path::Path, id: &SandboxId, host_port: u16) -> TestResult {
    let connection = rusqlite::Connection::open(state_path)?;
    let updated = connection.execute(
        "UPDATE sandboxes
         SET ssh_transport_enabled = 1, ssh_transport_host_port = ?1
         WHERE id = ?2",
        rusqlite::params![i64::from(host_port), id.as_str()],
    )?;
    assert_eq!(updated, 1);
    Ok(())
}

fn managed_host(id: &SandboxId, port: u16, identity: &gascand::HostIdentity) -> ManagedSshHost {
    ManagedSshHost {
        active: ActiveSsh {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port,
            alias: format!("gascan-{id}"),
            host_key_fingerprint: identity.fingerprint().to_owned(),
            client_key_fingerprint: identity.fingerprint().to_owned(),
        },
        host_public_key: identity.public_key().to_owned(),
    }
}

fn configured_generation(config: &str) -> TestResult {
    let path = config
        .lines()
        .find_map(|line| line.trim().strip_prefix("UserKnownHostsFile "))
        .ok_or("generated config did not reference known-hosts")?;
    std::fs::remove_file(path)?;
    Ok(())
}

#[tokio::test]
async fn pending_doctor_callers_converge_on_one_completed_report() {
    let (state, completer) = DoctorState::pending();
    let left = tokio::spawn({
        let state = state.clone();
        async move { state.report().await }
    });
    let right = tokio::spawn({
        let state = state.clone();
        async move { state.report().await }
    });
    tokio::task::yield_now().await;
    assert!(!left.is_finished());
    assert!(!right.is_finished());
    let expected = DoctorFacts::all_supported_for_tests().into_report();
    completer.complete(expected.clone());
    assert_eq!(left.await.unwrap().checks, expected.checks);
    assert_eq!(right.await.unwrap().checks, expected.checks);
}

#[tokio::test]
async fn abandoned_doctor_collection_fails_closed() {
    let (state, completer) = DoctorState::pending();
    drop(completer);
    let report = state.report().await;
    assert!(report.checks.iter().all(|check| {
        check.status != gascan_core::doctor::DoctorStatus::Pass
            && check.detail.contains("was abandoned")
    }));
}

#[tokio::test(start_paused = true)]
async fn producer_timeout_is_cached_for_late_and_concurrent_callers() {
    let expected = DoctorFacts::all_supported_for_tests().into_report();
    let state = DoctorState::collect(Duration::from_secs(60), {
        let expected = expected.clone();
        async move {
            tokio::time::sleep(Duration::from_secs(61)).await;
            expected
        }
    });
    let left = tokio::spawn({
        let state = state.clone();
        async move { state.report().await }
    });
    let right = tokio::spawn({
        let state = state.clone();
        async move { state.report().await }
    });
    tokio::time::advance(Duration::from_secs(60)).await;
    tokio::task::yield_now().await;
    let left = left.await.unwrap();
    let right = right.await.unwrap();
    assert_eq!(left.checks, right.checks);
    assert!(
        left.checks
            .iter()
            .all(|check| check.detail.contains("exceeded its 60 second bound"))
    );

    tokio::time::advance(Duration::from_secs(2)).await;
    let late = state.report().await;
    assert_eq!(late.checks, left.checks);
}

#[test]
fn ssh_doctor_facts_are_release_blocking_and_stably_identified() {
    let report = DoctorFacts::all_supported_for_tests().into_report();
    for id in [
        "ssh.client",
        "ssh.identity",
        "ssh.config",
        "ssh.native_publish",
    ] {
        let check = report.check(id).unwrap();
        assert_eq!(
            check.status,
            gascan_core::doctor::DoctorStatus::Pass,
            "{id}"
        );
        assert!(!check.detail.is_empty(), "{id}");
        assert!(!check.remedy.is_empty(), "{id}");
    }
}

#[tokio::test]
async fn service_doctor_refreshes_managed_ssh_state_after_startup() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = camino::Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
    let home = root.join("home");
    std::fs::create_dir(&home)?;
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))?;
    let home = home.canonicalize()?;
    let paths = SshPaths::for_environment(None, Some(home.as_os_str()))?;
    let service = SandboxService::new_with_ssh_for_tests(
        FakeRuntime::default(),
        Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
        paths.clone(),
        camino::Utf8PathBuf::from("/usr/bin/true"),
    );

    let initial = service.doctor_report().await;
    assert_eq!(
        initial.check("ssh.config").ok_or("ssh.config")?.status,
        gascan_core::doctor::DoctorStatus::Pass
    );

    ensure_host_identity(&paths).await?;

    let refreshed = service.doctor_report().await;
    assert_eq!(
        refreshed.check("ssh.config").ok_or("ssh.config")?.status,
        gascan_core::doctor::DoctorStatus::Fail
    );
    assert!(
        refreshed
            .check("ssh.config")
            .ok_or("ssh.config")?
            .detail
            .contains("missing")
    );
    Ok(())
}

#[tokio::test]
async fn ssh_doctor_defers_partial_artifacts_during_first_pending_create() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = camino::Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
    let home = root.join("home");
    std::fs::create_dir(&home)?;
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))?;
    let home = home.canonicalize()?;
    let paths = SshPaths::for_environment(None, Some(home.as_os_str()))?;
    let client = root.join("ssh");
    std::fs::write(&client, "#!/bin/sh\nexit 0\n")?;
    std::fs::set_permissions(&client, std::fs::Permissions::from_mode(0o755))?;
    let store = Store::open(root.join("state.db"))?;
    let id = SandboxId::test("doctor-pending-create");
    store.begin_operation(
        &sandbox_record(id, root, ActualState::Creating, None),
        OperationKind::Create,
    )?;
    ensure_host_identity(&paths).await?;

    let facts =
        gascand::ssh_doctor_facts_for_paths(&paths, client.as_std_path(), &store, true).await;

    assert_eq!(
        facts.identity.status,
        gascan_core::doctor::DoctorStatus::Unknown
    );
    assert_eq!(
        facts.config.status,
        gascan_core::doctor::DoctorStatus::Unknown
    );
    assert!(facts.config.detail.contains("lifecycle transition"));
    Ok(())
}

#[tokio::test]
async fn ssh_doctor_accepts_absent_managed_state_without_creating_it()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    std::fs::create_dir(&home)?;
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))?;
    let home = home.canonicalize()?;
    let paths = SshPaths::for_environment(None, Some(home.as_os_str()))?;
    let client = temp.path().join("ssh");
    std::fs::write(&client, "#!/bin/sh\nexit 0\n")?;
    std::fs::set_permissions(&client, std::fs::Permissions::from_mode(0o755))?;
    let store = Store::open(temp.path().join("state.db"))?;

    let facts = gascand::ssh_doctor_facts_for_paths(&paths, &client, &store, true).await;

    assert_eq!(facts.client.status, gascan_core::doctor::DoctorStatus::Pass);
    assert_eq!(
        facts.identity.status,
        gascan_core::doctor::DoctorStatus::Pass,
        "{}",
        facts.identity.detail
    );
    assert_eq!(
        facts.config.status,
        gascan_core::doctor::DoctorStatus::Pass,
        "{}",
        facts.config.detail
    );
    assert_eq!(
        facts.native_publish.status,
        gascan_core::doctor::DoctorStatus::Pass
    );
    assert!(!paths.directory().exists());
    Ok(())
}

#[tokio::test]
async fn ssh_doctor_parses_the_generated_config_with_exact_discrete_arguments()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let directory = home.join(".config/gascan/ssh");
    std::fs::create_dir_all(&directory)?;
    for path in [
        home.clone(),
        home.join(".config"),
        home.join(".config/gascan"),
        directory.clone(),
    ] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    let home = home.canonicalize()?;
    let paths = SshPaths::for_environment(None, Some(home.as_os_str()))?;
    let identity = ensure_host_identity(&paths).await?;
    publish_openssh_files(&paths, &identity, &[])?;
    let config = paths.config();
    let capture = temp.path().join("ssh.args");
    let client = temp.path().join("ssh");
    std::fs::write(
        &client,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >{}\nexit 0\n",
            capture.display()
        ),
    )?;
    std::fs::set_permissions(&client, std::fs::Permissions::from_mode(0o755))?;
    let store = Store::open(temp.path().join("state.db"))?;

    let facts = gascand::ssh_doctor_facts_for_paths(&paths, &client, &store, true).await;

    assert_eq!(facts.config.status, gascan_core::doctor::DoctorStatus::Pass);
    assert_eq!(
        std::fs::read_to_string(capture)?,
        format!("-G\n-F\n{config}\ngascan-doctor\n")
    );
    Ok(())
}

#[tokio::test]
async fn ssh_doctor_fails_closed_for_partial_identity_and_rejected_generated_config()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let directory = home.join(".config/gascan/ssh");
    std::fs::create_dir_all(&directory)?;
    for path in [
        home.clone(),
        home.join(".config"),
        home.join(".config/gascan"),
        directory.clone(),
    ] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    let home = home.canonicalize()?;
    let directory = home.join(".config/gascan/ssh");
    std::fs::write(directory.join("identity_ed25519"), "partial")?;
    std::fs::set_permissions(
        directory.join("identity_ed25519"),
        std::fs::Permissions::from_mode(0o600),
    )?;
    std::fs::write(directory.join("config"), "Host broken\n")?;
    std::fs::set_permissions(
        directory.join("config"),
        std::fs::Permissions::from_mode(0o644),
    )?;
    let paths = SshPaths::for_environment(None, Some(home.as_os_str()))?;
    let client = temp.path().join("ssh");
    std::fs::write(&client, "#!/bin/sh\nexit 42\n")?;
    std::fs::set_permissions(&client, std::fs::Permissions::from_mode(0o755))?;
    let store = Store::open(temp.path().join("state.db"))?;

    let facts = gascand::ssh_doctor_facts_for_paths(&paths, &client, &store, false).await;

    assert_eq!(
        facts.identity.status,
        gascan_core::doctor::DoctorStatus::Fail
    );
    assert!(facts.identity.detail.contains(paths.private_key().as_str()));
    assert_eq!(facts.config.status, gascan_core::doctor::DoctorStatus::Fail);
    assert!(facts.config.detail.contains(paths.config().as_str()));
    assert_eq!(
        facts.native_publish.status,
        gascan_core::doctor::DoctorStatus::Fail
    );
    Ok(())
}

#[tokio::test]
async fn ssh_doctor_rejects_expected_publication_with_missing_identity() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = camino::Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
    let home = root.join("home");
    std::fs::create_dir(&home)?;
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))?;
    let home = home.canonicalize()?;
    let paths = SshPaths::for_environment(None, Some(home.as_os_str()))?;
    let identity = ensure_host_identity(&paths).await?;
    let id = SandboxId::test("doctor-missing-identity");
    let host = managed_host(&id, 24_001, &identity);
    publish_openssh_files(&paths, &identity, std::slice::from_ref(&host))?;
    std::fs::remove_file(paths.private_key())?;
    std::fs::remove_file(paths.public_key())?;
    let state_path = root.join("state.db");
    let store = Store::open(&state_path)?;
    store.put_sandbox(&sandbox_record(
        id.clone(),
        root,
        ActualState::Running,
        Some(enabled_resolution(&host)),
    ))?;
    enable_transport(state_path.as_std_path(), &id, host.active.port)?;
    let client = root.join("ssh");
    std::fs::write(&client, "#!/bin/sh\nexit 0\n")?;
    std::fs::set_permissions(&client, std::fs::Permissions::from_mode(0o755))?;

    let facts =
        gascand::ssh_doctor_facts_for_paths(&paths, client.as_std_path(), &store, true).await;

    assert_eq!(
        facts.identity.status,
        gascan_core::doctor::DoctorStatus::Fail
    );
    assert_eq!(facts.config.status, gascan_core::doctor::DoctorStatus::Fail);
    Ok(())
}

#[tokio::test]
async fn ssh_doctor_rejects_expected_publication_with_missing_generation() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = camino::Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
    let home = root.join("home");
    std::fs::create_dir(&home)?;
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))?;
    let home = home.canonicalize()?;
    let paths = SshPaths::for_environment(None, Some(home.as_os_str()))?;
    let identity = ensure_host_identity(&paths).await?;
    let id = SandboxId::test("doctor-missing-generation");
    let host = managed_host(&id, 24_002, &identity);
    publish_openssh_files(&paths, &identity, std::slice::from_ref(&host))?;
    configured_generation(&std::fs::read_to_string(paths.config())?)?;
    let state_path = root.join("state.db");
    let store = Store::open(&state_path)?;
    store.put_sandbox(&sandbox_record(
        id.clone(),
        root,
        ActualState::Running,
        Some(enabled_resolution(&host)),
    ))?;
    enable_transport(state_path.as_std_path(), &id, host.active.port)?;
    let client = root.join("ssh");
    std::fs::write(&client, "#!/bin/sh\nexit 0\n")?;
    std::fs::set_permissions(&client, std::fs::Permissions::from_mode(0o755))?;

    let facts =
        gascand::ssh_doctor_facts_for_paths(&paths, client.as_std_path(), &store, true).await;

    assert_eq!(
        facts.identity.status,
        gascan_core::doctor::DoctorStatus::Pass
    );
    assert_eq!(facts.config.status, gascan_core::doctor::DoctorStatus::Fail);
    Ok(())
}

#[tokio::test]
async fn ssh_doctor_rejects_noncanonical_managed_config() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = camino::Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
    let home = root.join("home");
    std::fs::create_dir(&home)?;
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))?;
    let home = home.canonicalize()?;
    let paths = SshPaths::for_environment(None, Some(home.as_os_str()))?;
    let identity = ensure_host_identity(&paths).await?;
    let id = SandboxId::test("doctor-noncanonical");
    let host = managed_host(&id, 24_003, &identity);
    publish_openssh_files(&paths, &identity, std::slice::from_ref(&host))?;
    let mut config = std::fs::read_to_string(paths.config())?;
    config.push_str("\n# unexpected\n");
    std::fs::write(paths.config(), config)?;
    let state_path = root.join("state.db");
    let store = Store::open(&state_path)?;
    store.put_sandbox(&sandbox_record(
        id.clone(),
        root,
        ActualState::Running,
        Some(enabled_resolution(&host)),
    ))?;
    enable_transport(state_path.as_std_path(), &id, host.active.port)?;
    let client = root.join("ssh");
    std::fs::write(&client, "#!/bin/sh\nexit 0\n")?;
    std::fs::set_permissions(&client, std::fs::Permissions::from_mode(0o755))?;

    let facts =
        gascand::ssh_doctor_facts_for_paths(&paths, client.as_std_path(), &store, true).await;

    assert_eq!(
        facts.identity.status,
        gascan_core::doctor::DoctorStatus::Pass
    );
    assert_eq!(facts.config.status, gascan_core::doctor::DoctorStatus::Fail);
    Ok(())
}

#[tokio::test]
async fn ssh_doctor_rejects_stale_durable_expectation_for_absent_sandbox() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = camino::Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
    let home = root.join("home");
    std::fs::create_dir(&home)?;
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))?;
    let home = home.canonicalize()?;
    let paths = SshPaths::for_environment(None, Some(home.as_os_str()))?;
    let identity = ensure_host_identity(&paths).await?;
    let id = SandboxId::test("doctor-stale-absent");
    let host = managed_host(&id, 24_004, &identity);
    publish_openssh_files(&paths, &identity, std::slice::from_ref(&host))?;
    let state_path = root.join("state.db");
    let store = Store::open(&state_path)?;
    store.put_sandbox(&sandbox_record(
        id.clone(),
        root,
        ActualState::Absent,
        Some(enabled_resolution(&host)),
    ))?;
    enable_transport(state_path.as_std_path(), &id, host.active.port)?;
    let client = root.join("ssh");
    std::fs::write(&client, "#!/bin/sh\nexit 0\n")?;
    std::fs::set_permissions(&client, std::fs::Permissions::from_mode(0o755))?;

    let facts =
        gascand::ssh_doctor_facts_for_paths(&paths, client.as_std_path(), &store, true).await;

    assert_eq!(
        facts.identity.status,
        gascan_core::doctor::DoctorStatus::Pass
    );
    assert_eq!(facts.config.status, gascan_core::doctor::DoctorStatus::Fail);
    assert!(facts.config.detail.contains("durable"));
    Ok(())
}

#[tokio::test]
async fn ssh_doctor_accepts_complete_exact_expected_publication() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = camino::Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
    let home = root.join("home");
    std::fs::create_dir(&home)?;
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))?;
    let home = home.canonicalize()?;
    let paths = SshPaths::for_environment(None, Some(home.as_os_str()))?;
    let identity = ensure_host_identity(&paths).await?;
    let id = SandboxId::test("doctor-complete");
    let host = managed_host(&id, 24_005, &identity);
    publish_openssh_files(&paths, &identity, std::slice::from_ref(&host))?;
    let state_path = root.join("state.db");
    let store = Store::open(&state_path)?;
    store.put_sandbox(&sandbox_record(
        id.clone(),
        root,
        ActualState::Running,
        Some(enabled_resolution(&host)),
    ))?;
    enable_transport(state_path.as_std_path(), &id, host.active.port)?;
    let client = root.join("ssh");
    std::fs::write(&client, "#!/bin/sh\nexit 0\n")?;
    std::fs::set_permissions(&client, std::fs::Permissions::from_mode(0o755))?;

    let facts =
        gascand::ssh_doctor_facts_for_paths(&paths, client.as_std_path(), &store, true).await;

    assert_eq!(
        facts.identity.status,
        gascan_core::doctor::DoctorStatus::Pass
    );
    assert_eq!(facts.config.status, gascan_core::doctor::DoctorStatus::Pass);
    Ok(())
}

#[tokio::test]
async fn ssh_doctor_truncates_multibyte_diagnostics_at_a_utf8_boundary() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = camino::Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
    let home = root.join("home");
    std::fs::create_dir(&home)?;
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))?;
    let home = home.canonicalize()?;
    let paths = SshPaths::for_environment(None, Some(home.as_os_str()))?;
    let identity = ensure_host_identity(&paths).await?;
    publish_openssh_files(&paths, &identity, &[])?;
    let store = Store::open(root.join("state.db"))?;
    let client = root.join("ssh");
    std::fs::write(
        &client,
        "#!/bin/sh\n\
         i=0\n\
         while [ \"$i\" -lt 4095 ]; do\n\
           printf a >&2\n\
           i=$((i + 1))\n\
         done\n\
         printf 'é' >&2\n\
         exit 1\n",
    )?;
    std::fs::set_permissions(&client, std::fs::Permissions::from_mode(0o755))?;

    let facts =
        gascand::ssh_doctor_facts_for_paths(&paths, client.as_std_path(), &store, true).await;

    assert_eq!(facts.config.status, gascan_core::doctor::DoctorStatus::Fail);
    assert!(facts.config.detail.ends_with('…'));
    assert!(
        facts
            .config
            .detail
            .is_char_boundary(facts.config.detail.len())
    );
    Ok(())
}
