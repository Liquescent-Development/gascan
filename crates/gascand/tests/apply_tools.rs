use camino::Utf8Path;
use gascan_core::fake_runtime::FakeRuntime;
use gascan_core::manifest::Manifest;
use gascan_core::policy::workspace_environment;
use gascan_core::provision::{AppliedState, ProvisioningPlanner};
use gascan_core::runtime::{
    RemoveRequest, ResourceKind, RuntimeBackend, RuntimeCall, RuntimeError,
};
use gascan_core::sandbox::SandboxSpec;
use gascand::{NoopProvisioner, OperationStatus, SandboxService, UpRequest};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::error::Error;
use std::sync::Arc;

type TestResult = Result<(), Box<dyn Error>>;

fn write_manifest(root: &Utf8Path, tools: &[(&str, &str)]) -> TestResult {
    let mut source = "version = 1\n[tools]\n".to_owned();
    for (tool, version) in tools {
        source.push_str(&format!("{tool} = '{version}'\n"));
    }
    std::fs::write(root.join("gascan.toml"), source)?;
    Ok(())
}

fn spec(root: &Utf8Path, name: &str) -> Result<SandboxSpec, Box<dyn Error>> {
    Ok(SandboxSpec::from_root(name, root, Manifest::load(root)?)?)
}

fn write_shell_manifest(root: &Utf8Path, prompt: &str, setup: Option<&str>) -> TestResult {
    let setup = setup.map_or_else(String::new, |path| format!("setup = '{path}'\n"));
    std::fs::write(
        root.join("gascan.toml"),
        format!("version = 1\n{setup}[shell]\nprompt = '{prompt}'\n"),
    )?;
    Ok(())
}

fn desired_shell_hash(spec: &SandboxSpec) -> Result<String, Box<dyn Error>> {
    Ok(ProvisioningPlanner::plan_for_root(
        spec.canonical_root(),
        spec.manifest(),
        &AppliedState::empty(),
    )?
    .desired_shell_hash()
    .to_owned())
}

fn stored_shell_hash(record: &gascand::SandboxRecord) -> Option<&str> {
    record
        .tool_resolution
        .as_ref()?
        .details
        .get("shell_hash")?
        .as_str()
}

async fn event_details(
    service: &SandboxService<FakeRuntime>,
    operation_id: gascand::OperationId,
) -> Result<Vec<Value>, Box<dyn Error>> {
    Ok(service
        .store()
        .operation_events(operation_id)?
        .into_iter()
        .filter_map(|event| event.details)
        .collect())
}

#[tokio::test]
async fn shell_only_apply_configures_exact_prompt_without_tools_or_setup_and_then_is_noop()
-> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let setup = b"printf shell-only\n";
    std::fs::write(root.join("setup.sh"), setup)?;
    write_shell_manifest(root, "standard", Some("setup.sh"))?;
    let runtime = FakeRuntime::default();
    runtime
        .queue_exec_results([
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (
                format!("{:x}  /workspace/setup.sh\n", Sha256::digest(setup)).into_bytes(),
                Vec::new(),
                0,
            ),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
        ])
        .await;
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    service
        .up(UpRequest::new(spec(root, "shell-only")?))
        .await?;

    write_shell_manifest(root, "starship", Some("setup.sh"))?;
    let desired = spec(root, "shell-only")?;
    let expected_hash = desired_shell_hash(&desired)?;
    let before_up = runtime.calls().await.len();
    let up = service.up(UpRequest::new(desired.clone())).await?;
    assert!(event_details(&service, up.id).await?.iter().any(|event| {
        event.get("phase").and_then(Value::as_str) == Some("apply_required")
            && event.get("reason").and_then(Value::as_str) == Some("shell_changed")
    }));
    assert!(
        runtime.calls().await[before_up..]
            .iter()
            .all(|call| !matches!(call, RuntimeCall::Exec(_)))
    );
    let before_apply = runtime.calls().await.len();
    let operation = service.apply(UpRequest::new(desired.clone())).await?;
    let calls = runtime.calls().await;
    let apply_execs = calls[before_apply..]
        .iter()
        .filter_map(|call| match call {
            RuntimeCall::Exec(request) => Some(request),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(
        apply_execs
            .iter()
            .filter(|request| {
                request.argv.first().map(String::as_str) == Some("/usr/bin/sudo")
                    && request.argv.get(2).map(String::as_str)
                        == Some("/usr/local/bin/configure-shell-home")
            })
            .map(|request| request.argv.as_slice())
            .collect::<Vec<_>>(),
        vec![
            [
                "/usr/bin/sudo",
                "-n",
                "/usr/local/bin/configure-shell-home",
                "starship",
            ]
            .as_slice()
        ]
    );
    assert!(apply_execs.iter().all(|request| {
        !request.argv.iter().any(|arg| arg == "/usr/local/bin/mise")
            && request.argv.first().map(String::as_str) != Some("/bin/bash")
    }));
    assert!(
        event_details(&service, operation.id)
            .await?
            .iter()
            .any(|event| event.get("step").and_then(Value::as_str) == Some("configure_shell"))
    );
    assert_eq!(
        stored_shell_hash(&service.status(desired.id())?.ok_or("record")?),
        Some(expected_hash.as_str())
    );

    let before_noop = runtime.calls().await.len();
    service.apply(UpRequest::new(desired.clone())).await?;
    assert!(
        runtime.calls().await[before_noop..]
            .iter()
            .all(|call| !matches!(call, RuntimeCall::Exec(_)))
    );
    assert_eq!(
        stored_shell_hash(&service.status(desired.id())?.ok_or("record")?),
        Some(expected_hash.as_str())
    );
    Ok(())
}

#[tokio::test]
async fn existing_up_reports_apply_required_without_executing_tool_changes() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    write_manifest(root, &[])?;
    let runtime = FakeRuntime::default();
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    service
        .up(UpRequest::new(spec(root, "apply-required")?))
        .await?;
    let call_count = runtime.calls().await.len();
    write_manifest(root, &[("node", "lts")])?;

    let operation = service
        .up(UpRequest::new(spec(root, "apply-required")?))
        .await?;
    let details = event_details(&service, operation.id).await?;

    assert!(details.iter().any(|event| {
        event.get("phase").and_then(Value::as_str) == Some("apply_required")
            && event.get("reason").and_then(Value::as_str) == Some("tools_changed")
    }));
    assert!(
        runtime.calls().await[call_count..]
            .iter()
            .all(|call| !matches!(call, RuntimeCall::Exec(_)))
    );
    Ok(())
}

#[tokio::test]
async fn image_replace_reuses_unchanged_persistent_tools_without_reinstalling() -> TestResult {
    let old_image = "ghcr.io/liquescent-development/gascan/workspace:old@sha256:\
                     bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    write_manifest(root, &[("node", "lts")])?;
    let runtime = FakeRuntime::default();
    runtime
        .queue_exec_results([
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (
                br#"{"node":[{"version":"24.18.0","installed":true,"active":true,"source":{"path":"/home/workspace/.config/gascan/mise.toml"}}]}"#.to_vec(),
                Vec::new(),
                0,
            ),
            (
                br#"{"source":"bundled","revision":"initial"}"#.to_vec(),
                Vec::new(),
                0,
            ),
        ])
        .await;
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    let desired = spec(root, "image-replace-tools")?;
    service.up(UpRequest::new(desired.clone())).await?;
    let mut record = service.status(desired.id())?.ok_or("record")?;
    let prior_tools = record.tool_resolution.clone();
    record.image_resolution = Some(gascand::ImageResolution::new(
        1,
        json!({"digest":old_image}),
    ));
    service.store().put_sandbox(&record)?;
    runtime
        .queue_exec_results([
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (
                br#"{"source":"bundled","revision":"replacement"}"#.to_vec(),
                Vec::new(),
                0,
            ),
        ])
        .await;
    let installs_before = runtime
        .calls()
        .await
        .iter()
        .filter(|call| {
            matches!(call, RuntimeCall::Exec(request)
                if request.argv.iter().any(|arg| arg == "install"))
        })
        .count();

    service.apply(UpRequest::new(desired.clone())).await?;

    let installs_after = runtime
        .calls()
        .await
        .iter()
        .filter(|call| {
            matches!(call, RuntimeCall::Exec(request)
                if request.argv.iter().any(|arg| arg == "install"))
        })
        .count();
    assert_eq!(installs_after, installs_before);
    assert_eq!(
        service
            .status(desired.id())?
            .ok_or("replacement record")?
            .tool_resolution,
        prior_tools
    );
    Ok(())
}

#[tokio::test]
async fn apply_uses_literal_mise_argv_streams_steps_and_persists_exact_versions() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    write_manifest(root, &[])?;
    let runtime = FakeRuntime::default();
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    service
        .up(UpRequest::new(spec(root, "apply-tools")?))
        .await?;
    let before_apply = runtime.calls().await.len();
    write_manifest(root, &[("python", "3.14"), ("node", "lts")])?;
    runtime
        .queue_exec_results([
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (
                br#"{"node":[{"version":"24.18.0","installed":true,"active":true,"source":{"path":"/home/workspace/.config/gascan/mise.toml"}}],"python":[{"version":"3.14.6","installed":true,"active":true,"source":{"path":"/home/workspace/.config/gascan/mise.toml"}}]}"#.to_vec(),
                Vec::new(),
                0,
            ),
            (br#"{"source":"bundled","revision":"test"}"#.to_vec(), Vec::new(), 0),
        ])
        .await;

    let operation = service
        .apply(UpRequest::new(spec(root, "apply-tools")?))
        .await?;
    let details = event_details(&service, operation.id).await?;
    let calls = runtime.calls().await;
    let execs = calls[before_apply..]
        .iter()
        .filter_map(|call| match call {
            RuntimeCall::Exec(request) => Some(request),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(
        execs
            .iter()
            .map(|request| request.argv.as_slice())
            .collect::<Vec<_>>(),
        vec![
            [
                "/usr/bin/sudo",
                "-n",
                "/usr/bin/install",
                "-d",
                "-o",
                "workspace",
                "-g",
                "workspace",
                "-m",
                "0700",
                "/home/workspace/.local",
                "/home/workspace/.cache",
            ]
            .as_slice(),
            [
                "/usr/bin/sudo",
                "-n",
                "/usr/bin/install",
                "-d",
                "-o",
                "root",
                "-g",
                "workspace",
                "-m",
                "1770",
                "/home/workspace/.config",
                "/home/workspace/.config/gascan",
            ]
            .as_slice(),
            [
                "/usr/bin/env",
                "HOME=/home/workspace",
                "CARGO_HOME=/home/workspace/.local/share/cargo",
                "RUSTUP_HOME=/home/workspace/.local/share/rustup",
                "/usr/local/bin/initialize-rust-home",
            ]
            .as_slice(),
            [
                "/usr/bin/env",
                "HOME=/home/workspace",
                "/usr/local/bin/configure-workstation-home",
            ]
            .as_slice(),
            [
                "/usr/bin/rm",
                "--recursive",
                "--force",
                "--",
                "/home/workspace/.config/gascan/mise-workdir",
            ]
            .as_slice(),
            [
                "/usr/bin/install",
                "-d",
                "-m",
                "0700",
                "/home/workspace/.config/gascan/mise-workdir",
            ]
            .as_slice(),
            [
                "/usr/bin/install",
                "-m",
                "0600",
                "/dev/stdin",
                "/home/workspace/.config/gascan/mise.toml"
            ]
            .as_slice(),
            [
                "/usr/bin/env",
                "HOME=/home/workspace",
                "CARGO_HOME=/home/workspace/.local/share/cargo",
                "GEM_HOME=/home/workspace/.local/share/gem",
                "GOBIN=/home/workspace/.local/bin",
                "GOCACHE=/home/workspace/.cache/go-build",
                "GOMODCACHE=/home/workspace/.cache/go-mod",
                "GOPATH=/home/workspace/.local/share/go",
                "HEX_HOME=/home/workspace/.local/share/hex",
                "MISE_CACHE_DIR=/home/workspace/.cache/mise",
                "MISE_CARGO_HOME=/home/workspace/.local/share/cargo",
                "MISE_CEILING_PATHS=/home/workspace/.config/gascan/mise-workdir",
                "MISE_DATA_DIR=/home/workspace/.local/share/mise",
                "MISE_GLOBAL_CONFIG_FILE=/home/workspace/.config/gascan/mise.toml",
                "MISE_RUSTUP_HOME=/home/workspace/.local/share/rustup",
                "MISE_STATE_DIR=/home/workspace/.config/gascan/mise-state",
                "MISE_SYSTEM_CONFIG_FILE=/etc/mise/config.toml",
                "MISE_SYSTEM_DATA_DIR=/opt/gascan/mise",
                "MIX_HOME=/home/workspace/.local/share/mix",
                "NPM_CONFIG_CACHE=/home/workspace/.cache/npm",
                "NPM_CONFIG_PREFIX=/home/workspace/.local",
                "PATH=/home/workspace/.local/bin:/home/workspace/.local/share/cargo/bin:/home/workspace/.local/share/go/bin:/home/workspace/.local/share/gem/bin:/home/workspace/.local/share/mise/shims:/opt/gascan/mise/shims:/usr/local/sbin:/usr/local/bin:/opt/gascan/workstation/bin:/usr/sbin:/usr/bin:/sbin:/bin",
                "PYTHONUSERBASE=/home/workspace/.local",
                "REBAR_CACHE_DIR=/home/workspace/.cache/rebar3",
                "RUSTUP_HOME=/home/workspace/.local/share/rustup",
                "XDG_CACHE_HOME=/home/workspace/.cache",
                "XDG_CONFIG_HOME=/home/workspace/.config",
                "XDG_DATA_HOME=/home/workspace/.local/share",
                "/usr/local/bin/mise",
                "--cd",
                "/home/workspace/.config/gascan/mise-workdir",
                "--no-env",
                "--no-hooks",
                "install",
                "--yes",
            ]
            .as_slice(),
            [
                "/usr/bin/env",
                "HOME=/home/workspace",
                "CARGO_HOME=/home/workspace/.local/share/cargo",
                "GEM_HOME=/home/workspace/.local/share/gem",
                "GOBIN=/home/workspace/.local/bin",
                "GOCACHE=/home/workspace/.cache/go-build",
                "GOMODCACHE=/home/workspace/.cache/go-mod",
                "GOPATH=/home/workspace/.local/share/go",
                "HEX_HOME=/home/workspace/.local/share/hex",
                "MISE_CACHE_DIR=/home/workspace/.cache/mise",
                "MISE_CARGO_HOME=/home/workspace/.local/share/cargo",
                "MISE_CEILING_PATHS=/home/workspace/.config/gascan/mise-workdir",
                "MISE_DATA_DIR=/home/workspace/.local/share/mise",
                "MISE_GLOBAL_CONFIG_FILE=/home/workspace/.config/gascan/mise.toml",
                "MISE_RUSTUP_HOME=/home/workspace/.local/share/rustup",
                "MISE_STATE_DIR=/home/workspace/.config/gascan/mise-state",
                "MISE_SYSTEM_CONFIG_FILE=/etc/mise/config.toml",
                "MISE_SYSTEM_DATA_DIR=/opt/gascan/mise",
                "MIX_HOME=/home/workspace/.local/share/mix",
                "NPM_CONFIG_CACHE=/home/workspace/.cache/npm",
                "NPM_CONFIG_PREFIX=/home/workspace/.local",
                "PATH=/home/workspace/.local/bin:/home/workspace/.local/share/cargo/bin:/home/workspace/.local/share/go/bin:/home/workspace/.local/share/gem/bin:/home/workspace/.local/share/mise/shims:/opt/gascan/mise/shims:/usr/local/sbin:/usr/local/bin:/opt/gascan/workstation/bin:/usr/sbin:/usr/bin:/sbin:/bin",
                "PYTHONUSERBASE=/home/workspace/.local",
                "REBAR_CACHE_DIR=/home/workspace/.cache/rebar3",
                "RUSTUP_HOME=/home/workspace/.local/share/rustup",
                "XDG_CACHE_HOME=/home/workspace/.cache",
                "XDG_CONFIG_HOME=/home/workspace/.config",
                "XDG_DATA_HOME=/home/workspace/.local/share",
                "/usr/local/bin/mise",
                "--cd",
                "/home/workspace/.config/gascan/mise-workdir",
                "--no-env",
                "--no-hooks",
                "ls",
                "--current",
                "--installed",
                "--json",
            ]
            .as_slice(),
            ["/usr/local/bin/select-gascamp", "bundled"].as_slice(),
        ]
    );
    assert!(execs.iter().all(|request| request.environment.is_empty()));
    let create_environment = calls
        .iter()
        .find_map(|call| match call {
            RuntimeCall::Create(request) => Some(request.environment()),
            _ => None,
        })
        .ok_or("create request")?;
    let mut mise_environment = execs[8].argv[1..]
        .iter()
        .take_while(|arg| arg.as_str() != "/usr/local/bin/mise")
        .map(|entry| {
            entry
                .split_once('=')
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .ok_or("mise environment entry")
        })
        .collect::<Result<std::collections::BTreeMap<_, _>, _>>()?;
    assert_eq!(
        mise_environment.remove("MISE_CEILING_PATHS"),
        Some("/home/workspace/.config/gascan/mise-workdir".to_owned())
    );
    assert_eq!(mise_environment, workspace_environment());
    let mut runtime_environment = create_environment.clone();
    assert_eq!(
        runtime_environment.remove("GASCAN_SSH_ENABLED"),
        Some("0".to_owned())
    );
    assert_eq!(runtime_environment, workspace_environment());
    assert!(
        details
            .iter()
            .any(|event| event.get("step").and_then(Value::as_str)
                == Some("initialize_runtime_home"))
    );
    assert!(
        details.iter().any(
            |event| event.get("step").and_then(Value::as_str) == Some("write_safe_mise_config")
        )
    );
    assert!(
        details
            .iter()
            .any(|event| event.get("step").and_then(Value::as_str) == Some("install_tools"))
    );
    assert!(
        details
            .iter()
            .any(|event| event.get("step").and_then(Value::as_str) == Some("verify_gascamp"))
    );
    let record = service
        .status(spec(root, "apply-tools")?.id())?
        .ok_or("record")?;
    assert_eq!(
        record
            .tool_resolution
            .as_ref()
            .and_then(|resolution| resolution.details.get("resolution")),
        Some(&json!({"node":"24.18.0","python":"3.14.6"}))
    );
    assert_eq!(
        service.latest_operation()?.ok_or("operation")?.status,
        OperationStatus::Completed
    );
    Ok(())
}

#[tokio::test]
async fn failed_install_retains_applied_state_and_retry_can_succeed() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    write_manifest(root, &[])?;
    let runtime = FakeRuntime::default();
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    service
        .up(UpRequest::new(spec(root, "retry-tools")?))
        .await?;
    let id = spec(root, "retry-tools")?.id().clone();
    let prior = service.status(&id)?.ok_or("prior record")?.tool_resolution;
    write_manifest(root, &[("node", "lts")])?;
    runtime
        .queue_exec_results([
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), b"install failed".to_vec(), 23),
        ])
        .await;

    assert!(
        service
            .apply(UpRequest::new(spec(root, "retry-tools")?))
            .await
            .is_err()
    );
    assert_eq!(
        service.status(&id)?.ok_or("failed record")?.tool_resolution,
        prior
    );
    let failure = service.latest_operation()?.ok_or("failed operation")?;
    let details = failure.error_details.ok_or("install failure details")?;
    assert_eq!(details["step"], "install_tools");
    assert_eq!(details["action"], "install_tools");

    runtime
        .queue_exec_results([
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (
                br#"{"node":[{"version":"24.18.0","installed":true,"active":true,"source":{"path":"/home/workspace/.config/gascan/mise.toml"}}]}"#.to_vec(),
                Vec::new(),
                0,
            ),
            (br#"{"source":"bundled"}"#.to_vec(), Vec::new(), 0),
        ])
        .await;
    service
        .apply(UpRequest::new(spec(root, "retry-tools")?))
        .await?;
    assert_eq!(
        service
            .status(&id)?
            .ok_or("retried record")?
            .tool_resolution
            .and_then(|resolution| resolution.details.get("resolution").cloned()),
        Some(json!({"node":"24.18.0"}))
    );
    Ok(())
}

#[tokio::test]
async fn verbose_stderr_does_not_fail_successful_tool_install() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    write_manifest(root, &[])?;
    let runtime = FakeRuntime::default();
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    service
        .up(UpRequest::new(spec(root, "verbose-stderr")?))
        .await?;
    write_manifest(root, &[("node", "lts")])?;
    runtime
        .queue_exec_results([
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), vec![b'x'; 2 * 1024 * 1024], 0),
            (
                br#"{"node":[{"version":"24.18.0","installed":true,"active":true,"source":{"path":"/home/workspace/.config/gascan/mise.toml"}}]}"#.to_vec(),
                Vec::new(),
                0,
            ),
            (br#"{"source":"bundled"}"#.to_vec(), Vec::new(), 0),
        ])
        .await;

    service
        .apply(UpRequest::new(spec(root, "verbose-stderr")?))
        .await?;

    assert_eq!(
        service.latest_operation()?.ok_or("operation")?.status,
        OperationStatus::Completed
    );
    assert_eq!(runtime.exec_cancellations().await, 0);
    Ok(())
}

#[tokio::test]
async fn terminal_stderr_is_retained_in_command_failure() -> TestResult {
    const TERMINAL: &str = "ENOSPC: no space left on device";
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    write_manifest(root, &[])?;
    let runtime = FakeRuntime::default();
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    service
        .up(UpRequest::new(spec(root, "terminal-stderr")?))
        .await?;
    write_manifest(root, &[("node", "lts")])?;
    let mut stderr = vec![b'x'; 2 * 1024 * 1024];
    stderr.extend_from_slice(TERMINAL.as_bytes());
    runtime
        .queue_exec_results([
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), stderr, 23),
        ])
        .await;

    let error = match service
        .apply(UpRequest::new(spec(root, "terminal-stderr")?))
        .await
    {
        Ok(_) => return Err("install unexpectedly succeeded".into()),
        Err(error) => error,
    };
    assert!(error.to_string().contains("exit code 23"));
    assert!(error.to_string().contains(TERMINAL));
    let details = service
        .latest_operation()?
        .ok_or("operation")?
        .error_details
        .ok_or("failure details")?;
    assert!(
        details["stderr_tail"]
            .as_str()
            .is_some_and(|tail| tail.ends_with(TERMINAL))
    );
    assert_eq!(runtime.exec_cancellations().await, 0);
    Ok(())
}

#[tokio::test]
async fn oversized_stdout_cancels_guest_exec_session() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    write_manifest(root, &[])?;
    let runtime = FakeRuntime::default();
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    service
        .up(UpRequest::new(spec(root, "oversized-stdout")?))
        .await?;
    write_manifest(root, &[("node", "lts")])?;
    runtime
        .queue_exec_results([
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (vec![b'x'; 2 * 1024 * 1024], Vec::new(), 0),
        ])
        .await;

    let error = match service
        .apply(UpRequest::new(spec(root, "oversized-stdout")?))
        .await
    {
        Ok(_) => return Err("oversized stdout unexpectedly succeeded".into()),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("guest provisioning stdout exceeded its limit")
    );
    assert_eq!(runtime.exec_cancellations().await, 1);
    Ok(())
}

#[tokio::test]
async fn failed_safe_config_commands_record_fixed_boundary_and_guest_stderr() -> TestResult {
    const DIAGNOSTIC: &str = "guest command diagnostic";
    let cases = [
        (0, "initialize_managed_volume_roots"),
        (1, "secure_managed_config_root"),
        (2, "initialize_rust_home"),
        (3, "initialize_workstation_home"),
        (4, "reset_safe_mise_workdir"),
        (5, "create_safe_mise_workdir"),
        (6, "write_safe_mise_config"),
    ];
    for (failure_index, action) in cases {
        let root = tempfile::tempdir()?;
        let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
        write_manifest(root, &[("node", "lts")])?;
        let runtime = FakeRuntime::default();
        let mut results = vec![(Vec::new(), Vec::new(), 0); failure_index];
        results.push((Vec::new(), DIAGNOSTIC.as_bytes().to_vec(), 23));
        runtime.queue_exec_results(results).await;
        let service = SandboxService::new(
            runtime.clone(),
            gascand::Store::open(root.join("state.db"))?,
            Arc::new(NoopProvisioner),
        );

        let error = match service
            .up(UpRequest::new(spec(
                root,
                &format!("safe-config-{failure_index}"),
            )?))
            .await
        {
            Ok(_) => return Err("safe config command unexpectedly succeeded".into()),
            Err(error) => error,
        };
        assert!(error.to_string().contains(DIAGNOSTIC));
        let operation = service.latest_operation()?.ok_or("operation")?;
        let details = operation.error_details.ok_or("error details")?;
        let expected_step = if action == "initialize_rust_home" {
            "initialize_runtime_home"
        } else {
            "write_safe_mise_config"
        };
        assert_eq!(details["step"], expected_step);
        assert_eq!(details["action"], action);
        assert_eq!(details["exit_code"], 23);
        assert_eq!(details["signal"], 0);
        assert_eq!(details["stderr_tail"], DIAGNOSTIC);
        assert_eq!(runtime.exec_cancellations().await, 0);
    }
    Ok(())
}

#[tokio::test]
async fn empty_noop_apply_executes_no_guest_commands() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    write_manifest(root, &[])?;
    let runtime = FakeRuntime::default();
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    service
        .up(UpRequest::new(spec(root, "noop-tools")?))
        .await?;
    let initial_calls = runtime.calls().await;
    let initial_exec = initial_calls.iter().find_map(|call| match call {
        RuntimeCall::Exec(request) => {
            Some(request.argv.iter().map(String::as_str).collect::<Vec<_>>())
        }
        _ => None,
    });
    assert_eq!(
        initial_exec,
        Some(vec![
            "/usr/bin/sudo",
            "-n",
            "/usr/bin/install",
            "-d",
            "-o",
            "workspace",
            "-g",
            "workspace",
            "-m",
            "0700",
            "/home/workspace/.local",
            "/home/workspace/.cache",
        ])
    );
    let before = runtime.calls().await.len();

    service
        .apply(UpRequest::new(spec(root, "noop-tools")?))
        .await?;

    assert!(
        runtime.calls().await[before..]
            .iter()
            .all(|call| !matches!(call, RuntimeCall::Exec(_)))
    );
    Ok(())
}

#[tokio::test]
async fn managed_config_roots_are_secured_before_workstation_initialization() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    write_manifest(root, &[])?;
    let runtime = FakeRuntime::default();
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );

    service
        .up(UpRequest::new(spec(root, "secure-config-root")?))
        .await?;
    let execs = runtime
        .calls()
        .await
        .into_iter()
        .filter_map(|call| match call {
            RuntimeCall::Exec(request) => Some(request.argv),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(execs.len() >= 3);
    assert_eq!(
        execs[0],
        [
            "/usr/bin/sudo",
            "-n",
            "/usr/bin/install",
            "-d",
            "-o",
            "workspace",
            "-g",
            "workspace",
            "-m",
            "0700",
            "/home/workspace/.local",
            "/home/workspace/.cache",
        ]
    );
    assert_eq!(
        execs[1],
        [
            "/usr/bin/sudo",
            "-n",
            "/usr/bin/install",
            "-d",
            "-o",
            "root",
            "-g",
            "workspace",
            "-m",
            "1770",
            "/home/workspace/.config",
            "/home/workspace/.config/gascan",
        ]
    );
    assert_eq!(
        execs[2],
        [
            "/usr/bin/env",
            "HOME=/home/workspace",
            "CARGO_HOME=/home/workspace/.local/share/cargo",
            "RUSTUP_HOME=/home/workspace/.local/share/rustup",
            "/usr/local/bin/initialize-rust-home",
        ]
    );
    assert_eq!(
        execs[3],
        [
            "/usr/bin/env",
            "HOME=/home/workspace",
            "/usr/local/bin/configure-workstation-home",
        ]
    );
    Ok(())
}

#[tokio::test]
async fn duplicate_mise_tool_keys_are_rejected_without_advancing_state() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    write_manifest(root, &[])?;
    let runtime = FakeRuntime::default();
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    service
        .up(UpRequest::new(spec(root, "duplicate-tools")?))
        .await?;
    let id = spec(root, "duplicate-tools")?.id().clone();
    let prior = service.status(&id)?.ok_or("prior record")?.tool_resolution;
    write_manifest(root, &[("node", "lts")])?;
    runtime
        .queue_exec_results([
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (
                br#"{"node":[{"version":"24.18.0","installed":true,"active":true,"source":{"path":"/home/workspace/.config/gascan/mise.toml"}}],"node":[{"version":"attacker","installed":true,"active":true,"source":{"path":"/home/workspace/.config/gascan/mise.toml"}}]}"#.to_vec(),
                Vec::new(),
                0,
            ),
            (br#"{"source":"bundled"}"#.to_vec(), Vec::new(), 0),
        ])
        .await;

    assert!(
        service
            .apply(UpRequest::new(spec(root, "duplicate-tools")?))
            .await
            .is_err()
    );
    assert_eq!(
        service
            .status(&id)?
            .ok_or("failed duplicate record")?
            .tool_resolution,
        prior
    );
    Ok(())
}

#[tokio::test]
async fn legacy_matching_fingerprint_without_tool_hash_requires_one_explicit_apply() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    write_manifest(root, &[("node", "lts")])?;
    let runtime = FakeRuntime::default();
    runtime
        .queue_exec_results([
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (
                br#"{"node":[{"version":"24.18.0","installed":true,"active":true,"source":{"path":"/home/workspace/.config/gascan/mise.toml"}}]}"#.to_vec(),
                Vec::new(),
                0,
            ),
            (br#"{"source":"bundled"}"#.to_vec(), Vec::new(), 0),
        ])
        .await;
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    let make_spec = || spec(root, "legacy-tools");
    service.up(UpRequest::new(make_spec()?)).await?;
    let id = make_spec()?.id().clone();
    let mut legacy = service.status(&id)?.ok_or("record")?;
    legacy
        .tool_resolution
        .as_mut()
        .and_then(|resolution| resolution.details.as_object_mut())
        .ok_or("tool resolution object")?
        .remove("tool_hash");
    service.store().put_sandbox(&legacy)?;
    let before = runtime.calls().await.len();
    runtime
        .queue_exec_results([
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (
                br#"{"node":[{"version":"24.18.0","installed":true,"active":true,"source":{"path":"/home/workspace/.config/gascan/mise.toml"}}]}"#.to_vec(),
                Vec::new(),
                0,
            ),
            (br#"{"source":"bundled"}"#.to_vec(), Vec::new(), 0),
        ])
        .await;

    service.apply(UpRequest::new(make_spec()?)).await?;

    assert!(runtime.calls().await[before..].iter().any(|call| {
        matches!(call, RuntimeCall::Exec(request) if request.argv.last().map(String::as_str) == Some("--yes"))
    }));
    assert!(
        service
            .status(&id)?
            .and_then(|record| record.tool_resolution)
            .and_then(|resolution| resolution.details.get("tool_hash").cloned())
            .is_some()
    );
    Ok(())
}

#[tokio::test]
async fn removing_last_tool_writes_empty_config_and_persists_empty_resolution() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    write_manifest(root, &[("node", "lts")])?;
    let runtime = FakeRuntime::default();
    runtime
        .queue_exec_results([
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (
                br#"{"node":[{"version":"24.18.0","installed":true,"active":true,"source":{"path":"/home/workspace/.config/gascan/mise.toml"}}]}"#.to_vec(),
                Vec::new(),
                0,
            ),
            (br#"{"source":"bundled"}"#.to_vec(), Vec::new(), 0),
        ])
        .await;
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    let make_spec = || spec(root, "remove-tools");
    service.up(UpRequest::new(make_spec()?)).await?;
    write_manifest(root, &[])?;
    let before = runtime.calls().await.len();
    runtime
        .queue_exec_results([
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (b"{}".to_vec(), Vec::new(), 0),
            (br#"{"source":"bundled"}"#.to_vec(), Vec::new(), 0),
        ])
        .await;

    service.apply(UpRequest::new(make_spec()?)).await?;

    let calls = runtime.calls().await;
    let write = calls[before..]
        .iter()
        .find_map(|call| match call {
            RuntimeCall::Exec(request) if request.argv.iter().any(|arg| arg == "/dev/stdin") => {
                Some(request)
            }
            _ => None,
        })
        .ok_or("config write")?;
    assert_eq!(std::str::from_utf8(&write.stdin)?, "[tools]\n");
    let inventory = calls[before..]
        .iter()
        .find_map(|call| match call {
            RuntimeCall::Exec(request)
                if request.argv.ends_with(&[
                    "ls".to_owned(),
                    "--current".to_owned(),
                    "--installed".to_owned(),
                    "--json".to_owned(),
                ]) =>
            {
                Some(request)
            }
            _ => None,
        })
        .ok_or("empty inventory")?;
    assert_eq!(
        inventory.argv,
        [
            "/usr/bin/env",
            "HOME=/home/workspace",
            "CARGO_HOME=/home/workspace/.local/share/cargo",
            "GEM_HOME=/home/workspace/.local/share/gem",
            "GOBIN=/home/workspace/.local/bin",
            "GOCACHE=/home/workspace/.cache/go-build",
            "GOMODCACHE=/home/workspace/.cache/go-mod",
            "GOPATH=/home/workspace/.local/share/go",
            "HEX_HOME=/home/workspace/.local/share/hex",
            "MISE_CACHE_DIR=/home/workspace/.cache/mise",
            "MISE_CARGO_HOME=/home/workspace/.local/share/cargo",
            "MISE_CEILING_PATHS=/home/workspace/.config/gascan/mise-workdir",
            "MISE_DATA_DIR=/home/workspace/.local/share/mise",
            "MISE_GLOBAL_CONFIG_FILE=/home/workspace/.config/gascan/mise.toml",
            "MISE_RUSTUP_HOME=/home/workspace/.local/share/rustup",
            "MISE_STATE_DIR=/home/workspace/.config/gascan/mise-state",
            "MISE_SYSTEM_CONFIG_FILE=/etc/mise/config.toml",
            "MISE_SYSTEM_DATA_DIR=/opt/gascan/mise",
            "MIX_HOME=/home/workspace/.local/share/mix",
            "NPM_CONFIG_CACHE=/home/workspace/.cache/npm",
            "NPM_CONFIG_PREFIX=/home/workspace/.local",
            "PATH=/home/workspace/.local/bin:/home/workspace/.local/share/cargo/bin:/home/workspace/.local/share/go/bin:/home/workspace/.local/share/gem/bin:/home/workspace/.local/share/mise/shims:/opt/gascan/mise/shims:/usr/local/sbin:/usr/local/bin:/opt/gascan/workstation/bin:/usr/sbin:/usr/bin:/sbin:/bin",
            "PYTHONUSERBASE=/home/workspace/.local",
            "REBAR_CACHE_DIR=/home/workspace/.cache/rebar3",
            "RUSTUP_HOME=/home/workspace/.local/share/rustup",
            "XDG_CACHE_HOME=/home/workspace/.cache",
            "XDG_CONFIG_HOME=/home/workspace/.config",
            "XDG_DATA_HOME=/home/workspace/.local/share",
            "/usr/local/bin/mise",
            "--cd",
            "/home/workspace/.config/gascan/mise-workdir",
            "--no-env",
            "--no-hooks",
            "ls",
            "--current",
            "--installed",
            "--json",
        ]
    );
    assert!(inventory.environment.is_empty());
    let id = make_spec()?.id().clone();
    assert_eq!(
        service
            .status(&id)?
            .and_then(|record| record.tool_resolution)
            .and_then(|resolution| resolution.details.get("resolution").cloned()),
        Some(json!({}))
    );
    Ok(())
}

#[tokio::test]
async fn missing_container_forces_tool_install_even_when_durable_hash_matches() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    write_manifest(root, &[("node", "lts")])?;
    let runtime = FakeRuntime::default();
    runtime
        .queue_exec_results([
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (
                br#"{"node":[{"version":"24.18.0","installed":true,"active":true,"source":{"path":"/home/workspace/.config/gascan/mise.toml"}}]}"#.to_vec(),
                Vec::new(),
                0,
            ),
            (br#"{"source":"bundled"}"#.to_vec(), Vec::new(), 0),
        ])
        .await;
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    let make_spec = || spec(root, "recreated-tools");
    service.up(UpRequest::new(make_spec()?)).await?;
    let container = runtime
        .list_resources()
        .await?
        .into_iter()
        .find(|resource| resource.kind() == ResourceKind::Container)
        .ok_or("container resource")?;
    runtime
        .remove(RemoveRequest::from_resources(vec![container])?)
        .await?;
    let before = runtime.calls().await.len();
    runtime
        .queue_exec_results([
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (
                br#"{"node":[{"version":"24.18.0","installed":true,"active":true,"source":{"path":"/home/workspace/.config/gascan/mise.toml"}}]}"#.to_vec(),
                Vec::new(),
                0,
            ),
            (br#"{"source":"bundled"}"#.to_vec(), Vec::new(), 0),
        ])
        .await;

    service.up(UpRequest::new(make_spec()?)).await?;

    assert!(runtime.calls().await[before..].iter().any(|call| {
        matches!(call, RuntimeCall::Exec(request) if request.argv.last().map(String::as_str) == Some("--yes"))
    }));
    Ok(())
}

#[tokio::test]
async fn provisioning_transport_failures_never_leak_runtime_or_helper_content() -> TestResult {
    const SECRET: &str = "sentinel-provisioning-secret";
    for boundary in ["spawn", "input", "stream"] {
        let root = tempfile::tempdir()?;
        let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
        write_manifest(root, &[])?;
        let runtime = FakeRuntime::default();
        let injected = RuntimeError::HelperError {
            operation: format!("operation-{SECRET}"),
            code: format!("code-{SECRET}"),
            message: format!("message-{SECRET}"),
        };
        match boundary {
            "spawn" => runtime.queue_exec_error(injected).await,
            "input" => runtime.queue_exec_input_failure().await,
            "stream" => runtime.queue_exec_stream_error(injected).await,
            _ => return Err("unknown boundary".into()),
        }
        let service = SandboxService::new(
            runtime,
            gascand::Store::open(root.join("state.db"))?,
            Arc::new(NoopProvisioner),
        );

        let error = match service
            .up(UpRequest::new(spec(root, &format!("sanitize-{boundary}"))?))
            .await
        {
            Ok(_) => return Err("provisioning transport unexpectedly succeeded".into()),
            Err(error) => error,
        };
        let public = error.to_string();
        let durable = format!(
            "{:?}",
            service
                .store()
                .operation_events(service.latest_operation()?.ok_or("operation")?.id,)?
        );
        assert!(public.contains("guest provisioning transport failed"));
        assert!(!public.contains(SECRET));
        assert!(!durable.contains(SECRET));
    }
    Ok(())
}
