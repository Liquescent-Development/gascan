#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use gascan_apple::{AppleBackend, AppleProbe, ProcessRunner};
use gascan_core::doctor::{DoctorFact, DoctorFacts, DoctorReport};
#[cfg(debug_assertions)]
use gascan_core::fake_runtime::FakeRuntime;
use gascan_core::runtime::RuntimeBackend;
use gascand::{
    BackendSelection, Daemon, DaemonConfig, DoctorState, ProvisionRequest, ProvisionResolution,
    Provisioner, SandboxApi, SandboxService, ServiceError, SocketPaths, Store, backend_selection,
};
use std::{sync::Arc, time::Duration};

struct ConfiguredProvisioner {
    delay: Duration,
    fail: bool,
}
#[async_trait::async_trait]
impl Provisioner for ConfiguredProvisioner {
    async fn provision(
        &self,
        _request: ProvisionRequest<'_>,
    ) -> Result<ProvisionResolution, ServiceError> {
        tokio::time::sleep(self.delay).await;
        if self.fail {
            return Err(ServiceError::Provision("configured failure".to_owned()));
        }
        Ok(ProvisionResolution::default())
    }
    async fn health_check(
        &self,
        _id: &gascan_core::sandbox::SandboxId,
    ) -> Result<(), ServiceError> {
        Ok(())
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let idle_timeout = std::env::var("GASCAN_IDLE_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map_or(Duration::from_secs(300), Duration::from_millis);
    let paths = SocketPaths::for_user()?;
    paths.prepare_directory()?;
    let state_path = std::env::var_os("GASCAN_STATE_PATH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| paths.directory().join("state.sqlite3"));
    let store = Store::open(state_path)?;
    #[cfg(debug_assertions)]
    let fake_requested = std::env::var_os(gascand::TEST_FAKE_BACKEND_ENV).is_some();
    #[cfg(not(debug_assertions))]
    let fake_requested = false;
    match backend_selection(fake_requested) {
        BackendSelection::Apple => {
            let (doctor, completer) = DoctorState::pending();
            tokio::spawn(async move {
                completer.complete(production_doctor_report().await);
            });
            run_daemon(
                AppleBackend::new(ProcessRunner),
                store,
                paths,
                idle_timeout,
                Duration::ZERO,
                false,
                doctor,
            )
            .await
        }
        #[cfg(debug_assertions)]
        BackendSelection::Fake => {
            let provision_delay = std::env::var("GASCAN_FAKE_PROVISION_DELAY_MS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .map_or(Duration::ZERO, Duration::from_millis);
            let provision_fail = std::env::var_os("GASCAN_FAKE_PROVISION_FAIL").is_some();
            let fake_state_path = std::env::var_os("GASCAN_FAKE_STATE_PATH")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| paths.directory().join("fake-runtime.json"));
            let runtime = FakeRuntime::persistent(
                gascan_core::fake_runtime::fixture_capabilities(),
                fake_state_path,
            )
            .await?;
            if std::env::var_os("GASCAN_FAKE_CAPABILITIES_FAIL").is_some() {
                runtime
                    .inject_failure(gascan_core::fake_runtime::FailureBoundary::Capabilities)
                    .await;
            }
            if let Some(delay) = std::env::var("GASCAN_FAKE_LOGS_FAIL_AFTER_MS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
            {
                let failing = runtime.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    failing
                        .inject_failure(gascan_core::fake_runtime::FailureBoundary::Logs)
                        .await;
                });
            }
            run_daemon(
                runtime,
                store,
                paths,
                idle_timeout,
                provision_delay,
                provision_fail,
                DoctorState::ready(DoctorFacts::all_supported_for_tests().into_report()),
            )
            .await
        }
    }
}

async fn run_daemon<B: RuntimeBackend + 'static>(
    runtime: B,
    store: Store,
    paths: SocketPaths,
    idle_timeout: Duration,
    provision_delay: Duration,
    provision_fail: bool,
    doctor: DoctorState,
) -> Result<(), Box<dyn std::error::Error>> {
    let service = Arc::new(SandboxService::new_with_doctor_state(
        runtime,
        store,
        Arc::new(ConfiguredProvisioner {
            delay: provision_delay,
            fail: provision_fail,
        }),
        doctor,
    ));
    let config = DaemonConfig::new(paths, idle_timeout);
    let api = SandboxApi::new(service, config.activity());
    Daemon::serve(config, api).await?;
    Ok(())
}

async fn production_doctor_report() -> DoctorReport {
    let mut facts = DoctorFacts::unavailable("evidence was not collected");
    facts.architecture = architecture_fact(std::env::consts::ARCH);
    facts.macos = macos_fact();

    let probe = AppleProbe::new(ProcessRunner);
    match probe.base_capabilities().await {
        Ok(capabilities) => {
            facts.cli = DoctorFact::pass("container executable returned structured JSON");
            let exact = capabilities.version == gascan_core::runtime::RuntimeVersion::new(1, 1, 0);
            let matrix = capabilities.bind_mounts
                && capabilities.named_volumes
                && capabilities.tty
                && capabilities.signals
                && capabilities.loopback_publish
                && capabilities.resource_limits
                && capabilities.offline == gascan_core::runtime::NetworkIsolation::Proven;
            facts.version = if exact && matrix {
                DoctorFact::pass(gate2_evidence(
                    "exact release client 1.1.0 revision matched",
                ))
            } else {
                DoctorFact::fail(format!(
                    "unsupported Apple container CLI/revision {}.{}.{}; Gate 2 requires exact 1.1.0 at {}",
                    capabilities.version.major,
                    capabilities.version.minor,
                    capabilities.version.patch,
                    gascan_apple::APPLE_1_1_COMMIT,
                ))
            };
            facts.schema = if exact && matrix {
                DoctorFact::pass(gate2_evidence("client version schema matched"))
            } else {
                DoctorFact::fail("capability schema is not signed off for this CLI version")
            };
            facts.bind_mounts = capability_fact(capabilities.bind_mounts, "bind mounts");
            facts.named_volumes = capability_fact(capabilities.named_volumes, "named volumes");
            facts.tty = capability_fact(capabilities.tty, "TTY attachment");
            facts.signals = capability_fact(capabilities.signals, "signal forwarding");
            facts.loopback_publish =
                capability_fact(capabilities.loopback_publish, "loopback publication");
            facts.resource_limits =
                capability_fact(capabilities.resource_limits, "resource limits");
            facts.offline = capability_fact(
                capabilities.offline == gascan_core::runtime::NetworkIsolation::Proven,
                "hard offline isolation",
            );
        }
        Err(error) => apply_cli_error(&mut facts, &error),
    }

    match probe.status().await {
        Ok(status) => {
            let exact_service = status.api_server_version
                == gascan_core::runtime::RuntimeVersion::new(1, 1, 0)
                && status.api_server_commit == gascan_apple::APPLE_1_1_COMMIT;
            facts.service = if exact_service {
                DoctorFact::pass(gate2_evidence(
                    "structured system status reports the exact running API server",
                ))
            } else {
                DoctorFact::fail(format!(
                    "running API server identity/revision is not exact Gate 2 revision {}",
                    gascan_apple::APPLE_1_1_COMMIT
                ))
            };
            if status.api_server_version == gascan_core::runtime::RuntimeVersion::new(1, 1, 0)
                && exact_service
                && facts.schema.status == gascan_core::doctor::DoctorStatus::Pass
            {
                facts.schema =
                    DoctorFact::pass("CLI and API server match the Apple 1.1 structured schemas");
            } else {
                facts.schema =
                    DoctorFact::fail("API server version does not match Apple container 1.1.0");
            }
            facts.kernel = if exact_service
                && facts.architecture.status == gascan_core::doctor::DoctorStatus::Pass
                && facts.macos.status == gascan_core::doctor::DoctorStatus::Pass
            {
                DoctorFact::pass(gate2_evidence(
                    "Gate 2 kernel/live lifecycle proof plus current exact running service establishes MVP kernel readiness",
                ))
            } else {
                DoctorFact::unknown(
                    "kernel readiness requires the supported host and exact running Gate 2 API server revision",
                )
            };
            facts.state_storage =
                storage_fact(std::path::Path::new(&status.app_root), "application root");
            facts.image_storage = storage_fact(
                std::path::Path::new(&status.app_root),
                "shared Apple application/state/image",
            );
        }
        Err(error) => facts.service = service_error_fact(&error),
    }
    if [
        &facts.cli,
        &facts.version,
        &facts.service,
        &facts.schema,
        &facts.offline,
    ]
    .into_iter()
    .all(|fact| fact.status == gascan_core::doctor::DoctorStatus::Pass)
    {
        facts.workspace = workspace_fact(&std::env::current_dir());
    } else {
        facts.workspace = DoctorFact::unknown(
            "workspace was not accessed because an earlier runtime prerequisite failed",
        );
    }
    facts.into_report()
}

fn capability_fact(supported: bool, name: &str) -> DoctorFact {
    if supported {
        DoctorFact::pass(gate2_evidence(&format!(
            "signed-off live matrix proves {name}"
        )))
    } else {
        DoctorFact::fail(format!("{name} is unsupported"))
    }
}

fn gate2_evidence(evidence: &str) -> String {
    format!(
        "{evidence}; Gate 2 report commit {}, report sha256 {}, status fixture sha256 {}, Apple revision {}",
        gascan_apple::GATE2_REPORT_COMMIT,
        gascan_apple::GATE2_REPORT_SHA256,
        gascan_apple::STATUS_FIXTURE_SHA256,
        gascan_apple::APPLE_1_1_COMMIT,
    )
}

fn architecture_fact(architecture: &str) -> DoctorFact {
    if architecture == "aarch64" {
        DoctorFact::pass("current process target is aarch64")
    } else {
        DoctorFact::fail(format!("current process target is {architecture}"))
    }
}

fn apply_cli_error(facts: &mut DoctorFacts, error: &gascan_core::runtime::RuntimeError) {
    use gascan_core::runtime::RuntimeError;
    match error {
        RuntimeError::CommandIo { .. } => {
            facts.cli = DoctorFact::fail(format!("container executable unavailable: {error}"))
        }
        RuntimeError::UnsupportedVersion { .. } => {
            facts.cli = DoctorFact::pass("container executable returned structured JSON");
            facts.version = DoctorFact::fail(error.to_string());
        }
        RuntimeError::InvalidOutput { .. } => {
            facts.cli = DoctorFact::pass("container executable ran");
            facts.schema =
                DoctorFact::fail(format!("malformed structured version response: {error}"));
        }
        _ => facts.cli = DoctorFact::fail(format!("container version command failed: {error}")),
    }
}

fn service_error_fact(error: &gascan_core::runtime::RuntimeError) -> DoctorFact {
    DoctorFact::fail(format!("structured system status failed: {error}"))
}

fn macos_fact() -> DoctorFact {
    macos_fact_at(std::path::Path::new(
        "/System/Library/CoreServices/SystemVersion.plist",
    ))
}

fn macos_fact_at(path: &std::path::Path) -> DoctorFact {
    let result = plist::Value::from_file(path).ok().and_then(|value| {
        value
            .as_dictionary()
            .and_then(|dictionary| dictionary.get("ProductVersion"))
            .and_then(plist::Value::as_string)
            .map(str::to_owned)
    });
    match result {
        Some(version)
            if version
                .split('.')
                .next()
                .and_then(|major| major.parse::<u64>().ok())
                .is_some_and(|major| major >= 26) =>
        {
            DoctorFact::pass(format!("SystemVersion.plist ProductVersion is {version}"))
        }
        Some(version) => DoctorFact::fail(format!(
            "SystemVersion.plist ProductVersion is {version}; macOS 26+ required"
        )),
        None => DoctorFact::fail("could not parse ProductVersion from SystemVersion.plist"),
    }
}

fn workspace_fact(result: &std::io::Result<std::path::PathBuf>) -> DoctorFact {
    let metadata = result
        .as_ref()
        .map_err(ToString::to_string)
        .and_then(|path| path.canonicalize().map_err(|error| error.to_string()))
        .and_then(|path| std::fs::metadata(path).map_err(|error| error.to_string()));
    match metadata {
        Ok(metadata) if metadata.is_dir() => {
            DoctorFact::pass("current canonical workspace directory is accessible")
        }
        Ok(_) => DoctorFact::fail("current workspace is not a directory"),
        Err(error) => DoctorFact::fail(format!("current workspace is inaccessible: {error}")),
    }
}

fn storage_fact(path: &std::path::Path, label: &str) -> DoctorFact {
    const MIN_FREE_BYTES: u64 = 10 * 1024 * 1024 * 1024;
    match rustix::fs::statvfs(path) {
        Ok(stat) => {
            let free = stat.f_bavail.saturating_mul(stat.f_frsize);
            if free >= MIN_FREE_BYTES {
                DoctorFact::pass(format!("{label} filesystem has {free} free bytes"))
            } else {
                DoctorFact::fail(format!(
                    "{label} filesystem has only {free} free bytes; {MIN_FREE_BYTES} required"
                ))
            }
        }
        Err(error) => DoctorFact::fail(format!(
            "cannot inspect {label} filesystem at {}: {error}",
            path.display()
        )),
    }
}

#[cfg(test)]
mod doctor_tests {
    use super::*;

    #[test]
    fn host_architecture_mismatch_fails() {
        assert_eq!(
            architecture_fact("x86_64").status,
            gascan_core::doctor::DoctorStatus::Fail
        );
    }

    #[test]
    fn plist_product_version_is_structured_and_requires_26()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("SystemVersion.plist");
        let mut dictionary = plist::Dictionary::new();
        dictionary.insert(
            "ProductVersion".to_owned(),
            plist::Value::String("25.9".to_owned()),
        );
        plist::Value::Dictionary(dictionary).to_file_xml(&path)?;
        assert_eq!(
            macos_fact_at(&path).status,
            gascan_core::doctor::DoctorStatus::Fail
        );
        Ok(())
    }

    #[test]
    fn missing_storage_and_workspace_evidence_fail() {
        let missing = std::path::PathBuf::from("/definitely/not/a/gascan/path");
        assert_eq!(
            storage_fact(&missing, "test").status,
            gascan_core::doctor::DoctorStatus::Fail
        );
        assert_eq!(
            workspace_fact(&Ok(missing)).status,
            gascan_core::doctor::DoctorStatus::Fail
        );
    }

    #[test]
    fn cli_failures_are_distinguished_from_version_and_schema_failures() {
        let mut missing = DoctorFacts::unavailable("test");
        apply_cli_error(
            &mut missing,
            &gascan_core::runtime::RuntimeError::CommandIo {
                operation: "container".to_owned(),
                message: "not found".to_owned(),
            },
        );
        assert_eq!(missing.cli.status, gascan_core::doctor::DoctorStatus::Fail);

        let mut unsupported = DoctorFacts::unavailable("test");
        apply_cli_error(
            &mut unsupported,
            &gascan_core::runtime::RuntimeError::UnsupportedVersion {
                found: gascan_core::runtime::RuntimeVersion::new(2, 0, 0),
                supported: "1.1.0".to_owned(),
            },
        );
        assert_eq!(
            unsupported.version.status,
            gascan_core::doctor::DoctorStatus::Fail
        );

        let mut malformed = DoctorFacts::unavailable("test");
        apply_cli_error(
            &mut malformed,
            &gascan_core::runtime::RuntimeError::InvalidOutput {
                operation: "version".to_owned(),
                message: "bad json".to_owned(),
            },
        );
        assert_eq!(
            malformed.schema.status,
            gascan_core::doctor::DoctorStatus::Fail
        );
    }

    #[test]
    fn service_command_failure_is_a_stable_failed_fact() {
        let fact = service_error_fact(&gascan_core::runtime::RuntimeError::CommandFailed {
            operation: "container system status".to_owned(),
            exit_code: Some(1),
            stderr: "service unavailable".to_owned(),
        });
        assert_eq!(fact.status, gascan_core::doctor::DoctorStatus::Fail);
        assert!(fact.detail.contains("system status"));
    }

    #[test]
    fn gate2_fact_names_the_frozen_report_fixture_and_apple_revision() {
        let detail = gate2_evidence("verified");
        assert!(detail.contains(gascan_apple::GATE2_REPORT_COMMIT));
        assert!(detail.contains(gascan_apple::GATE2_REPORT_SHA256));
        assert!(detail.contains(gascan_apple::STATUS_FIXTURE_SHA256));
        assert!(detail.contains(gascan_apple::APPLE_1_1_COMMIT));
    }
}
