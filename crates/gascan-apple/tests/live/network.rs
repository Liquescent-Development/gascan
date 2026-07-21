#[path = "../../../../tests/fixtures/network/host-server.rs"]
mod host_server;

use std::{
    process::{Command, Stdio},
    sync::Arc,
    time::Duration,
};

use gascan_apple::{CommandOutput, CommandRunner, CommandSpec, ProcessRunner};

use super::common::{LiveContext, TestError, container_record, random_owner_token};

struct HostDnsMapping {
    runner: Arc<dyn CommandRunner>,
    domain: String,
    pending: bool,
    drop_cleanup: bool,
}

impl std::fmt::Debug for HostDnsMapping {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostDnsMapping")
            .field("domain", &self.domain)
            .field("pending", &self.pending)
            .finish_non_exhaustive()
    }
}

struct HostDnsCreateFailure {
    mapping: Option<HostDnsMapping>,
    error: TestError,
}

impl std::fmt::Debug for HostDnsCreateFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostDnsCreateFailure")
            .field(
                "domain",
                &self.mapping.as_ref().map(|mapping| &mapping.domain),
            )
            .field("error", &self.error.to_string())
            .finish()
    }
}

impl std::fmt::Display for HostDnsCreateFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(formatter)
    }
}

impl std::error::Error for HostDnsCreateFailure {}

impl HostDnsMapping {
    async fn create() -> Result<Self, TestError> {
        Self::create_with(Arc::new(ProcessRunner), random_owner_token()?)
            .await
            .map_err(|error| Box::new(error) as TestError)
    }

    async fn create_with(
        runner: Arc<dyn CommandRunner>,
        token: String,
    ) -> Result<Self, HostDnsCreateFailure> {
        Self::create_with_timeout(runner, token, Duration::from_secs(30)).await
    }

    async fn create_with_timeout(
        runner: Arc<dyn CommandRunner>,
        token: String,
        create_timeout: Duration,
    ) -> Result<Self, HostDnsCreateFailure> {
        let domain = format!("gascan-{token}.test");
        if !owned_domain(&domain) {
            return Err(HostDnsCreateFailure {
                mapping: None,
                error: "temporary DNS owner token is not 128-bit lowercase hexadecimal".into(),
            });
        }
        let existing =
            list_domains(runner.as_ref())
                .await
                .map_err(|error| HostDnsCreateFailure {
                    mapping: None,
                    error,
                })?;
        if existing.iter().any(|item| item == &domain) {
            return Err(HostDnsCreateFailure {
                mapping: None,
                error: format!("temporary DNS domain already exists: {domain}").into(),
            });
        }
        let mut mapping = Self {
            runner,
            domain,
            pending: true,
            drop_cleanup: true,
        };
        let create_result = run_dns_with_timeout(
            mapping.runner.as_ref(),
            ["create", "--localhost", "203.0.113.113", &mapping.domain],
            create_timeout,
        )
        .await;
        let reconcile_result = list_domains(mapping.runner.as_ref()).await;
        let exactly_present = reconcile_result.as_ref().is_ok_and(|domains| {
            domains
                .iter()
                .filter(|item| *item == &mapping.domain)
                .count()
                == 1
        });
        if reconcile_result
            .as_ref()
            .is_ok_and(|domains| !domains.iter().any(|item| item == &mapping.domain))
        {
            mapping.pending = false;
        }
        match (create_result, reconcile_result, exactly_present) {
            (Ok(_), Ok(_), true) => Ok(mapping),
            (create, reconcile, _) => {
                let detail = match (create, reconcile) {
                    (Err(create), Err(reconcile)) => {
                        format!("create failed: {create}; reconciliation failed: {reconcile}")
                    }
                    (Err(create), Ok(_)) => format!("create failed: {create}"),
                    (Ok(_), Err(reconcile)) => format!("reconciliation failed: {reconcile}"),
                    (Ok(_), Ok(_)) => "created DNS domain is absent or ambiguous".to_owned(),
                };
                Err(HostDnsCreateFailure {
                    error: format!("{detail}: {}", mapping.domain).into(),
                    mapping: Some(mapping),
                })
            }
        }
    }

    fn url(&self, port: u16) -> String {
        format!("http://{}:{port}", self.domain)
    }

    async fn cleanup(&mut self) -> Result<(), TestError> {
        if !self.pending {
            return Ok(());
        }
        let domains = list_domains(self.runner.as_ref()).await?;
        let count = domains.iter().filter(|item| *item == &self.domain).count();
        if count == 0 {
            self.pending = false;
            return Ok(());
        }
        if count != 1 || !owned_domain(&self.domain) {
            return Err(format!("DNS ownership mismatch: {}", self.domain).into());
        }
        run_dns(self.runner.as_ref(), ["delete", &self.domain]).await?;
        let domains = list_domains(self.runner.as_ref()).await?;
        if domains.iter().any(|item| item == &self.domain) {
            return Err(format!("deleted DNS domain remains present: {}", self.domain).into());
        }
        self.pending = false;
        Ok(())
    }
}

impl Drop for HostDnsMapping {
    fn drop(&mut self) {
        if !self.drop_cleanup || !self.pending || !owned_domain(&self.domain) {
            return;
        }
        let Some(output) = blocking_sudo_container(["system", "dns", "list", "--format", "json"])
        else {
            return;
        };
        let Ok(domains) = serde_json::from_slice::<Vec<String>>(&output) else {
            return;
        };
        if domains.iter().filter(|item| *item == &self.domain).count() == 1 {
            let _ = blocking_sudo_container(["system", "dns", "delete", &self.domain]);
        }
    }
}

fn owned_domain(domain: &str) -> bool {
    let Some(token) = domain
        .strip_prefix("gascan-")
        .and_then(|value| value.strip_suffix(".test"))
    else {
        return false;
    };
    token.len() == 32
        && token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

async fn list_domains(runner: &dyn CommandRunner) -> Result<Vec<String>, TestError> {
    let output = run_sudo_container(runner, ["system", "dns", "list", "--format", "json"]).await?;
    Ok(serde_json::from_slice(&output.stdout)?)
}

async fn run_dns<const N: usize>(
    runner: &dyn CommandRunner,
    args: [&str; N],
) -> Result<CommandOutput, TestError> {
    run_dns_with_timeout(runner, args, Duration::from_secs(30)).await
}

async fn run_dns_with_timeout<const N: usize>(
    runner: &dyn CommandRunner,
    args: [&str; N],
    timeout: Duration,
) -> Result<CommandOutput, TestError> {
    let mut full = vec!["system", "dns"];
    full.extend(args);
    run_sudo_container_with_timeout(runner, full, timeout).await
}

async fn run_sudo_container<I, A>(
    runner: &dyn CommandRunner,
    args: I,
) -> Result<CommandOutput, TestError>
where
    I: IntoIterator<Item = A>,
    A: Into<String>,
{
    run_sudo_container_with_timeout(runner, args, Duration::from_secs(30)).await
}

async fn run_sudo_container_with_timeout<I, A>(
    runner: &dyn CommandRunner,
    args: I,
    timeout: Duration,
) -> Result<CommandOutput, TestError>
where
    I: IntoIterator<Item = A>,
    A: Into<String>,
{
    let mut argv = vec!["-n".to_owned(), "container".to_owned()];
    argv.extend(args.into_iter().map(Into::into));
    Ok(
        tokio::time::timeout(timeout, runner.run(CommandSpec::new("sudo", argv)))
            .await
            .map_err(|_| "host DNS command exceeded 30-second timeout")??,
    )
}

fn blocking_sudo_container<const N: usize>(args: [&str; N]) -> Option<Vec<u8>> {
    let mut child = Command::new("sudo")
        .args(["-n", "container"])
        .args(args)
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;
    for _ in 0..100 {
        if let Some(status) = child.try_wait().ok()? {
            if !status.success() {
                return None;
            }
            return child.wait_with_output().ok().map(|output| output.stdout);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    None
}

fn has_no_network_attachments(value: &serde_json::Value) -> bool {
    container_record(value)
        .and_then(|record| record.get("configuration"))
        .and_then(|configuration| configuration.get("networks"))
        .and_then(serde_json::Value::as_array)
        .is_some_and(Vec::is_empty)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProbeRole {
    Diagnostic,
    DenialOnly,
    RequiredPositive,
}

#[derive(Debug, Eq, PartialEq)]
struct NetworkTarget {
    mechanism: &'static str,
    url: String,
    role: ProbeRole,
}

fn network_targets(host_url: String) -> [NetworkTarget; 4] {
    [
        NetworkTarget {
            mechanism: "DNS plus external HTTP",
            url: "http://example.com".to_owned(),
            role: ProbeRole::Diagnostic,
        },
        NetworkTarget {
            mechanism: "direct external IPv4",
            url: "http://1.1.1.1".to_owned(),
            role: ProbeRole::Diagnostic,
        },
        NetworkTarget {
            mechanism: "TEST-NET IPv4",
            url: "http://192.0.2.1".to_owned(),
            role: ProbeRole::DenialOnly,
        },
        NetworkTarget {
            mechanism: "owned host DNS/PF mapping",
            url: host_url,
            role: ProbeRole::RequiredPositive,
        },
    ]
}

fn guest_mutation_command() -> &'static str {
    "ip link add gascan0 type dummy 2>&1; printf 'link-add-exit=%s\\n' $?; \
     ip route add default via 192.0.2.1 2>&1; printf 'route-add-exit=%s\\n' $?; \
     printf '%s\\n' 'post-mutation-ip-link:'; ip link show; \
     printf '%s\\n' 'post-mutation-ip-route:'; ip route show"
}

#[test]
fn exact_none_form_has_empty_structured_attachments() {
    let value = serde_json::json!([{"configuration":{"networks":[]}}]);
    assert!(has_no_network_attachments(&value));
    assert!(!has_no_network_attachments(
        &serde_json::json!([{"configuration":{"networks":[{"network":"default"}]}}])
    ));
}

#[tokio::test]
#[ignore = "requires supported Apple runtime and adversarial network probes"]
async fn offline_workspace_cannot_reach_external_or_host_networks() -> Result<(), TestError> {
    let host = host_server::HostServer::start()?;
    let mut mapping = HostDnsMapping::create().await?;
    let control = LiveContext::new("network-control").await?;
    let targets = network_targets(mapping.url(host.port()));
    for target in targets
        .iter()
        .filter(|target| target.role == ProbeRole::Diagnostic)
    {
        let reachable = control.can_reach(&target.url).await?;
        eprintln!(
            "networked diagnostic for {}: reachable={reachable} ({})",
            target.mechanism, target.url
        );
    }
    let host_target = targets
        .iter()
        .find(|target| target.role == ProbeRole::RequiredPositive)
        .ok_or("owned host positive control is missing")?;
    assert!(
        control.can_reach(&host_target.url).await?,
        "networked positive control failed for {}: {}",
        host_target.mechanism,
        host_target.url
    );
    control.cleanup().await?;

    let ctx = LiveContext::offline("network").await?;
    assert!(has_no_network_attachments(&ctx.inspect().await?));
    for target in &targets {
        assert!(
            !ctx.can_reach(&target.url).await?,
            "offline target unexpectedly reachable through {}: {}",
            target.mechanism,
            target.url
        );
    }
    ctx.exec("test -d /workspace && ip link show lo >/dev/null")
        .await?;
    let mutation_state = ctx.exec(guest_mutation_command()).await?;
    eprintln!(
        "guest-root network mutation evidence:\n{}",
        String::from_utf8_lossy(&mutation_state.stdout)
    );
    assert!(has_no_network_attachments(&ctx.inspect().await?));
    for target in &targets {
        assert!(
            !ctx.can_reach(&target.url).await?,
            "guest-root mutation made {} reachable: {}",
            target.mechanism,
            target.url
        );
    }
    ctx.cleanup().await?;
    mapping.cleanup().await
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use async_trait::async_trait;
    use gascan_core::runtime::RuntimeError;

    use super::*;

    #[test]
    fn target_matrix_uses_busybox_http_and_keeps_all_offline_mechanisms() {
        let targets = network_targets("http://gascan-owner.test:1234".into());
        assert_eq!(targets[0].role, ProbeRole::Diagnostic);
        assert_eq!(targets[1].role, ProbeRole::Diagnostic);
        assert_eq!(targets[2].role, ProbeRole::DenialOnly);
        assert_eq!(targets[3].role, ProbeRole::RequiredPositive);
        assert_eq!(
            targets.map(|target| (target.mechanism, target.url)),
            [
                ("DNS plus external HTTP", "http://example.com".to_owned()),
                ("direct external IPv4", "http://1.1.1.1".to_owned()),
                ("TEST-NET IPv4", "http://192.0.2.1".to_owned()),
                (
                    "owned host DNS/PF mapping",
                    "http://gascan-owner.test:1234".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn guest_mutation_command_reports_attempt_results_and_final_network_state() {
        let command = guest_mutation_command();
        assert!(command.contains("link-add-exit=%s"));
        assert!(command.contains("route-add-exit=%s"));
        assert!(command.contains("ip link show"));
        assert!(command.contains("ip route show"));
        assert!(!command.contains("|| true"));
    }

    struct ScriptedRunner {
        responses: Mutex<VecDeque<Result<CommandOutput, RuntimeError>>>,
        specs: Mutex<Vec<CommandSpec>>,
    }

    struct TimeoutThenAmbiguousRunner {
        calls: AtomicUsize,
        domain: String,
    }

    #[async_trait]
    impl CommandRunner for TimeoutThenAmbiguousRunner {
        async fn run(&self, _spec: CommandSpec) -> Result<CommandOutput, RuntimeError> {
            match self.calls.fetch_add(1, Ordering::SeqCst) {
                0 => output(serde_json::json!([])),
                1 => std::future::pending().await,
                2 => output(serde_json::json!([
                    self.domain.clone(),
                    self.domain.clone()
                ])),
                _ => panic!("unexpected DNS command"),
            }
        }
    }

    #[async_trait]
    impl CommandRunner for ScriptedRunner {
        async fn run(&self, spec: CommandSpec) -> Result<CommandOutput, RuntimeError> {
            self.specs.lock().unwrap().push(spec);
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected DNS command")
        }
    }

    fn output(value: serde_json::Value) -> Result<CommandOutput, RuntimeError> {
        Ok(CommandOutput {
            status: 0,
            stdout: serde_json::to_vec(&value).unwrap(),
            stderr: Vec::new(),
        })
    }

    fn runner(responses: Vec<Result<CommandOutput, RuntimeError>>) -> Arc<ScriptedRunner> {
        Arc::new(ScriptedRunner {
            responses: Mutex::new(responses.into()),
            specs: Mutex::new(Vec::new()),
        })
    }

    fn command_error() -> Result<CommandOutput, RuntimeError> {
        Err(RuntimeError::CommandFailed {
            operation: "sudo".into(),
            exit_code: Some(1),
            stderr: "reported failure after mutation".into(),
        })
    }

    #[tokio::test]
    async fn preexisting_domain_collision_is_never_created_or_deleted() {
        let token = "00112233445566778899aabbccddeeff";
        let domain = format!("gascan-{token}.test");
        let runner = runner(vec![output(serde_json::json!([domain]))]);
        assert!(
            HostDnsMapping::create_with(runner.clone(), token.into())
                .await
                .is_err()
        );
        assert_eq!(runner.specs.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn malformed_owner_token_is_rejected_before_any_command() {
        let runner = runner(Vec::new());
        assert!(
            HostDnsMapping::create_with(runner.clone(), "NOT-AN-OWNER".into())
                .await
                .is_err()
        );
        assert!(runner.specs.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn exact_domain_schema_and_cleanup_use_literal_commands() {
        let token = "00112233445566778899aabbccddeeff";
        let domain = format!("gascan-{token}.test");
        let runner = runner(vec![
            output(serde_json::json!([])),
            output(serde_json::json!(null)),
            output(serde_json::json!([domain.clone()])),
            output(serde_json::json!([domain.clone()])),
            output(serde_json::json!(null)),
            output(serde_json::json!([])),
        ]);
        let mut mapping = HostDnsMapping::create_with(runner.clone(), token.into())
            .await
            .unwrap();
        mapping.drop_cleanup = false;
        mapping.cleanup().await.unwrap();
        let specs = runner.specs.lock().unwrap();
        assert_eq!(
            specs[1],
            CommandSpec::new(
                "sudo",
                [
                    "-n",
                    "container",
                    "system",
                    "dns",
                    "create",
                    "--localhost",
                    "203.0.113.113",
                    &domain
                ]
            )
        );
        assert_eq!(
            specs[4],
            CommandSpec::new(
                "sudo",
                ["-n", "container", "system", "dns", "delete", &domain]
            )
        );
    }

    #[tokio::test]
    async fn create_error_after_side_effect_retains_pending_guard() {
        let token = "00112233445566778899aabbccddeeff";
        let domain = format!("gascan-{token}.test");
        let runner = runner(vec![
            output(serde_json::json!([])),
            command_error(),
            output(serde_json::json!([domain.clone()])),
            output(serde_json::json!([domain])),
            output(serde_json::json!(null)),
            output(serde_json::json!([])),
        ]);
        let mut failure = HostDnsMapping::create_with(runner, token.into())
            .await
            .unwrap_err();
        let mapping = failure
            .mapping
            .as_mut()
            .expect("side effect must retain guard");
        assert!(mapping.pending);
        mapping.drop_cleanup = false;
        mapping.cleanup().await.unwrap();
        assert!(!mapping.pending);
    }

    #[tokio::test]
    async fn create_timeout_with_ambiguous_state_retains_pending_guard() {
        let token = "00112233445566778899aabbccddeeff";
        let runner = Arc::new(TimeoutThenAmbiguousRunner {
            calls: AtomicUsize::new(0),
            domain: format!("gascan-{token}.test"),
        });
        let mut failure = HostDnsMapping::create_with_timeout(
            runner.clone(),
            token.into(),
            Duration::from_millis(1),
        )
        .await
        .unwrap_err();
        let mapping = failure
            .mapping
            .as_mut()
            .expect("ambiguous timeout must retain guard");
        assert!(mapping.pending);
        mapping.drop_cleanup = false;
        assert_eq!(runner.calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn delete_success_without_structured_removal_retains_pending_guard() {
        let token = "00112233445566778899aabbccddeeff";
        let domain = format!("gascan-{token}.test");
        let runner = runner(vec![
            output(serde_json::json!([])),
            output(serde_json::json!(null)),
            output(serde_json::json!([domain.clone()])),
            output(serde_json::json!([domain.clone()])),
            output(serde_json::json!(null)),
            output(serde_json::json!([domain])),
        ]);
        let mut mapping = HostDnsMapping::create_with(runner, token.into())
            .await
            .unwrap();
        mapping.drop_cleanup = false;
        assert!(mapping.cleanup().await.is_err());
        assert!(mapping.pending);
    }

    #[tokio::test]
    async fn unowned_domain_is_retained_and_not_deleted() {
        let runner = runner(vec![output(serde_json::json!(["shared.test"]))]);
        let mut mapping = HostDnsMapping {
            runner: runner.clone(),
            domain: "shared.test".into(),
            pending: true,
            drop_cleanup: false,
        };
        assert!(mapping.cleanup().await.is_err());
        assert!(mapping.pending);
        assert_eq!(runner.specs.lock().unwrap().len(), 1);
    }
}
