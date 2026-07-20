use camino::Utf8Path;
use gascan_core::fake_runtime::FakeRuntime;
use gascan_core::manifest::Manifest;
use gascan_core::runtime::{
    RemoveRequest, ResourceKind, RuntimeBackend, RuntimeCall, RuntimeError,
};
use gascan_core::sandbox::SandboxSpec;
use gascand::{NoopProvisioner, OperationStatus, SandboxService, UpRequest};
use serde_json::{Value, json};
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
            (
                br#"{"node":[{"version":"24.18.0","installed":true,"active":true}],"python":[{"version":"3.14.6","installed":true,"active":true}]}"#.to_vec(),
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
                "/home/workspace/.local/share/mise",
                "/home/workspace/.cache",
                "/home/workspace/.config/gascan",
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
                "MISE_CACHE_DIR=/home/workspace/.cache/mise",
                "MISE_CEILING_PATHS=/home/workspace/.config/gascan/mise-workdir",
                "MISE_DATA_DIR=/home/workspace/.local/share/mise",
                "MISE_GLOBAL_CONFIG_FILE=/home/workspace/.config/gascan/mise.toml",
                "MISE_SYSTEM_CONFIG_FILE=/home/workspace/.config/gascan/mise.toml",
                "MISE_SYSTEM_DATA_DIR=/opt/gascan/mise",
                "PATH=/home/workspace/.local/share/mise/shims:/opt/gascan/mise/shims:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
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
                "MISE_CACHE_DIR=/home/workspace/.cache/mise",
                "MISE_CEILING_PATHS=/home/workspace/.config/gascan/mise-workdir",
                "MISE_DATA_DIR=/home/workspace/.local/share/mise",
                "MISE_GLOBAL_CONFIG_FILE=/home/workspace/.config/gascan/mise.toml",
                "MISE_SYSTEM_CONFIG_FILE=/home/workspace/.config/gascan/mise.toml",
                "MISE_SYSTEM_DATA_DIR=/opt/gascan/mise",
                "PATH=/home/workspace/.local/share/mise/shims:/opt/gascan/mise/shims:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
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

    runtime
        .queue_exec_results([
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (
                br#"{"node":[{"version":"24.18.0","installed":true,"active":true}]}"#.to_vec(),
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
async fn failed_safe_config_commands_record_fixed_boundary_without_guest_content() -> TestResult {
    const SECRET: &str = "guest-stderr-must-not-escape";
    let cases = [
        (0, "initialize_managed_volume_roots"),
        (1, "reset_safe_mise_workdir"),
        (2, "create_safe_mise_workdir"),
        (3, "write_safe_mise_config"),
    ];
    for (failure_index, action) in cases {
        let root = tempfile::tempdir()?;
        let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
        write_manifest(root, &[("node", "lts")])?;
        let runtime = FakeRuntime::default();
        let mut results = vec![(Vec::new(), Vec::new(), 0); failure_index];
        results.push((Vec::new(), SECRET.as_bytes().to_vec(), 23));
        runtime.queue_exec_results(results).await;
        let service = SandboxService::new(
            runtime,
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
        assert_eq!(
            error.to_string(),
            "provisioning failed: guest provisioning command failed"
        );
        let operation = service.latest_operation()?.ok_or("operation")?;
        let details = operation.error_details.ok_or("error details")?;
        assert_eq!(details["step"], "write_safe_mise_config");
        assert_eq!(details["action"], action);
        assert_eq!(details["exit_code"], 23);
        assert_eq!(details["signal"], 0);
        assert!(!format!("{details:?}").contains(SECRET));
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
            (
                br#"{"node":[{"version":"24.18.0","installed":true,"active":true}],"node":[{"version":"attacker","installed":true,"active":true}]}"#.to_vec(),
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
            (
                br#"{"node":[{"version":"24.18.0","installed":true,"active":true}]}"#.to_vec(),
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
            (
                br#"{"node":[{"version":"24.18.0","installed":true,"active":true}]}"#.to_vec(),
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
            (
                br#"{"node":[{"version":"24.18.0","installed":true,"active":true}]}"#.to_vec(),
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
            "MISE_CACHE_DIR=/home/workspace/.cache/mise",
            "MISE_CEILING_PATHS=/home/workspace/.config/gascan/mise-workdir",
            "MISE_DATA_DIR=/home/workspace/.local/share/mise",
            "MISE_GLOBAL_CONFIG_FILE=/home/workspace/.config/gascan/mise.toml",
            "MISE_SYSTEM_CONFIG_FILE=/home/workspace/.config/gascan/mise.toml",
            "MISE_SYSTEM_DATA_DIR=/opt/gascan/mise",
            "PATH=/home/workspace/.local/share/mise/shims:/opt/gascan/mise/shims:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
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
            (
                br#"{"node":[{"version":"24.18.0","installed":true,"active":true}]}"#.to_vec(),
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
            (
                br#"{"node":[{"version":"24.18.0","installed":true,"active":true}]}"#.to_vec(),
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
