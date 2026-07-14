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

impl HostDnsMapping {
    async fn create() -> Result<Self, TestError> {
        Self::create_with(Arc::new(ProcessRunner), random_owner_token()?).await
    }

    async fn create_with(runner: Arc<dyn CommandRunner>, token: String) -> Result<Self, TestError> {
        let domain = format!("gascan-{token}.test");
        if !owned_domain(&domain) {
            return Err("temporary DNS owner token is not 128-bit lowercase hexadecimal".into());
        }
        if list_domains(runner.as_ref())
            .await?
            .iter()
            .any(|item| item == &domain)
        {
            return Err(format!("temporary DNS domain already exists: {domain}").into());
        }
        run_dns(
            runner.as_ref(),
            ["create", "--localhost", "203.0.113.113", &domain],
        )
        .await?;
        let mapping = Self {
            runner,
            domain,
            pending: true,
            drop_cleanup: true,
        };
        if !mapping.is_exactly_present().await? {
            return Err(format!(
                "created DNS domain is absent or ambiguous: {}",
                mapping.domain
            )
            .into());
        }
        Ok(mapping)
    }

    fn url(&self, port: u16) -> String {
        format!("http://{}:{port}", self.domain)
    }

    async fn is_exactly_present(&self) -> Result<bool, TestError> {
        let domains = list_domains(self.runner.as_ref()).await?;
        Ok(domains.iter().filter(|item| *item == &self.domain).count() == 1)
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
    let mut full = vec!["system", "dns"];
    full.extend(args);
    run_sudo_container(runner, full).await
}

async fn run_sudo_container<I, A>(
    runner: &dyn CommandRunner,
    args: I,
) -> Result<CommandOutput, TestError>
where
    I: IntoIterator<Item = A>,
    A: Into<String>,
{
    let mut argv = vec!["-n".to_owned(), "container".to_owned()];
    argv.extend(args.into_iter().map(Into::into));
    Ok(tokio::time::timeout(
        Duration::from_secs(30),
        runner.run(CommandSpec::new("sudo", argv)),
    )
    .await
    .map_err(|_| "host DNS command exceeded 30-second timeout")??)
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
    let targets = [
        "https://example.com".to_owned(),
        "http://1.1.1.1".to_owned(),
        "http://192.0.2.1".to_owned(),
        mapping.url(host.port()),
    ];
    for target in [&targets[0], &targets[1], &targets[3]] {
        assert!(
            control.can_reach(target).await?,
            "networked positive control could not reach: {target}"
        );
    }
    control.cleanup().await?;

    let ctx = LiveContext::offline("network").await?;
    assert!(has_no_network_attachments(&ctx.inspect().await?));
    for target in &targets {
        assert!(
            !ctx.can_reach(target).await?,
            "offline target unexpectedly reachable: {target}"
        );
    }
    ctx.exec("test -d /workspace && ip link show lo >/dev/null")
        .await?;
    ctx.exec("ip link add gascan0 type dummy 2>/dev/null || true; ip route add default via 192.0.2.1 2>/dev/null || true").await?;
    for target in &targets {
        assert!(
            !ctx.can_reach(target).await?,
            "guest-root mutation made target reachable: {target}"
        );
    }
    ctx.cleanup().await?;
    mapping.cleanup().await
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, sync::Mutex};

    use async_trait::async_trait;
    use gascan_core::runtime::RuntimeError;

    use super::*;

    struct ScriptedRunner {
        responses: Mutex<VecDeque<Result<CommandOutput, RuntimeError>>>,
        specs: Mutex<Vec<CommandSpec>>,
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
