#![allow(dead_code)]

use std::{
    fs,
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use gascan_apple::{CommandOutput, CommandRunner, CommandSpec, ProcessRunner};
use serde_json::Value;

pub type TestError = Box<dyn std::error::Error + Send + Sync>;

const IMAGE: &str = "docker.io/library/alpine:3.20";
const LABEL: &str = "dev.gascan.test=true";

#[derive(Default)]
struct Records {
    containers: Vec<String>,
    volumes: Vec<String>,
    networks: Vec<String>,
    paths: Vec<PathBuf>,
}

pub struct LiveContext {
    runner: Arc<dyn CommandRunner>,
    prefix: String,
    workspace: PathBuf,
    container: Mutex<String>,
    volume: String,
    publish: Option<(u16, u16)>,
    records: Mutex<Records>,
    drop_cleanup: bool,
}

impl LiveContext {
    pub async fn new(case: &str) -> Result<Self, TestError> {
        Self::new_inner(case, None).await
    }

    pub async fn new_published(case: &str, host_port: u16) -> Result<Self, TestError> {
        Self::new_inner(case, Some((host_port, 8080))).await
    }

    async fn new_inner(case: &str, publish: Option<(u16, u16)>) -> Result<Self, TestError> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let prefix = format!("gascan-feas-{}-{case}-{nonce}", std::process::id());
        let workspace = std::env::temp_dir().join(&prefix);
        fs::create_dir(&workspace)?;
        let volume = format!("{prefix}-volume");
        let container = format!("{prefix}-container");
        let ctx = Self {
            runner: Arc::new(ProcessRunner),
            prefix,
            workspace: workspace.clone(),
            container: Mutex::new(container),
            volume: volume.clone(),
            publish,
            records: Mutex::new(Records {
                paths: vec![workspace],
                ..Records::default()
            }),
            drop_cleanup: true,
        };

        let volume_result = ctx
            .run_ok([
                "volume",
                "create",
                "--label",
                LABEL,
                "-s",
                "104857600",
                &ctx.volume,
            ])
            .await;
        match volume_result {
            Ok(_) => ctx.records.lock().unwrap().volumes.push(ctx.volume.clone()),
            Err(create_error) => {
                if let Err(reconcile_error) = ctx.reconcile_volume(&ctx.volume).await {
                    return Err(format!(
                        "{create_error}; volume reconciliation failed: {reconcile_error}"
                    )
                    .into());
                }
                return Err(create_error);
            }
        }
        if let Err(error) = ctx.create_container().await {
            return match ctx.cleanup().await {
                Ok(()) => Err(error),
                Err(cleanup_error) => {
                    Err(format!("{error}; cleanup failed: {cleanup_error}").into())
                }
            };
        }
        Ok(ctx)
    }

    async fn create_container(&self) -> Result<(), TestError> {
        let name = self.container.lock().unwrap().clone();
        let mount = format!(
            "type=bind,source={},target=/workspace",
            self.workspace.display()
        );
        let volume = format!("{}:/opt/gascan", self.volume);
        let mut args = vec![
            "run".to_owned(),
            "--name".to_owned(),
            name.clone(),
            "--label".to_owned(),
            LABEL.to_owned(),
            "--mount".to_owned(),
            mount,
            "--volume".to_owned(),
            volume,
            "--cpus".to_owned(),
            "1".to_owned(),
            "--memory".to_owned(),
            "268435456".to_owned(),
            "--init".to_owned(),
            "--detach".to_owned(),
        ];
        if let Some((host, guest)) = self.publish {
            args.extend(publish_args(IpAddr::V4(Ipv4Addr::LOCALHOST), host, guest)?);
        }
        args.extend(guest_argv(self.publish.is_some()));
        match self.run_vec(args).await {
            Ok(_) => {
                self.records.lock().unwrap().containers.push(name);
                Ok(())
            }
            Err(create_error) => {
                if let Err(reconcile_error) = self.reconcile_container(&name).await {
                    return Err(format!(
                        "{create_error}; container reconciliation failed: {reconcile_error}"
                    )
                    .into());
                }
                Err(create_error)
            }
        }
    }

    pub async fn recreate_container(&self) -> Result<(), TestError> {
        let old = self.container.lock().unwrap().clone();
        self.delete_container(&old).await?;
        *self.container.lock().unwrap() = format!("{}-recreated", self.prefix);
        self.create_container().await
    }

    pub async fn exec(&self, command: &str) -> Result<CommandOutput, TestError> {
        let name = self.container.lock().unwrap().clone();
        self.run_ok(["exec", &name, "sh", "-c", command]).await
    }

    pub async fn write_host(&self, name: &str, contents: &str) -> Result<(), TestError> {
        fs::write(safe_child(&self.workspace, name)?, contents)?;
        Ok(())
    }

    pub async fn read_host(&self, name: &str) -> Result<String, TestError> {
        Ok(fs::read_to_string(safe_child(&self.workspace, name)?)?)
    }

    pub fn canonical_workspace(&self) -> Result<PathBuf, TestError> {
        Ok(fs::canonicalize(&self.workspace)?)
    }

    pub async fn write_cache(&self, name: &str, contents: &str) -> Result<(), TestError> {
        let command = format!(
            "printf %s {} > /opt/gascan/{}",
            shell_word(contents)?,
            shell_word(name)?
        );
        self.exec(&command).await?;
        Ok(())
    }

    pub async fn read_cache(&self, name: &str) -> Result<String, TestError> {
        let output = self
            .exec(&format!("cat /opt/gascan/{}", shell_word(name)?))
            .await?;
        Ok(String::from_utf8(output.stdout)?)
    }

    pub async fn inspect(&self) -> Result<Value, TestError> {
        let name = self.container.lock().unwrap().clone();
        let output = self.run_ok(["inspect", &name]).await?;
        Ok(serde_json::from_slice(&output.stdout)?)
    }

    pub async fn logs(&self) -> Result<String, TestError> {
        let name = self.container.lock().unwrap().clone();
        let output = self.run_ok(["logs", &name]).await?;
        Ok(format!(
            "stdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))
    }

    pub async fn stop(&self) -> Result<(), TestError> {
        let name = self.container.lock().unwrap().clone();
        if self.is_running().await? {
            self.run_ok(["stop", "--time", "5", &name]).await?;
        }
        Ok(())
    }

    pub async fn start(&self) -> Result<(), TestError> {
        let name = self.container.lock().unwrap().clone();
        if !self.is_running().await? {
            self.run_ok(["start", &name]).await?;
        }
        Ok(())
    }

    pub async fn is_running(&self) -> Result<bool, TestError> {
        Ok(
            container_state(&self.inspect().await?).is_some_and(|status| {
                status.eq_ignore_ascii_case("running") || status.starts_with("Up ")
            }),
        )
    }

    async fn run_ok<const N: usize>(&self, args: [&str; N]) -> Result<CommandOutput, TestError> {
        Ok(tokio::time::timeout(
            Duration::from_secs(30),
            self.runner.run(CommandSpec::new("container", args)),
        )
        .await
        .map_err(|_| "Apple container command exceeded 30-second live-test timeout")??)
    }

    async fn run_vec(&self, args: Vec<String>) -> Result<CommandOutput, TestError> {
        Ok(tokio::time::timeout(
            Duration::from_secs(30),
            self.runner.run(CommandSpec::new("container", args)),
        )
        .await
        .map_err(|_| "Apple container command exceeded 30-second live-test timeout")??)
    }

    async fn reconcile_container(&self, name: &str) -> Result<(), TestError> {
        let Some(value) = self.inspect_container_if_present(name).await? else {
            return Ok(());
        };
        require_owned_container(&value, name, &self.prefix)?;
        self.records
            .lock()
            .unwrap()
            .containers
            .push(name.to_owned());
        Ok(())
    }

    async fn reconcile_volume(&self, name: &str) -> Result<(), TestError> {
        let output = self.run_ok(["volume", "list", "--format", "json"]).await?;
        let value: Value = serde_json::from_slice(&output.stdout)?;
        match volume_record(&value, name) {
            None => Ok(()),
            Some(record) => {
                require_owned_volume(record, name, &self.prefix)?;
                self.records.lock().unwrap().volumes.push(name.to_owned());
                Ok(())
            }
        }
    }

    async fn delete_container(&self, name: &str) -> Result<(), TestError> {
        let Some(value) = self.inspect_container_if_present(name).await? else {
            self.records
                .lock()
                .unwrap()
                .containers
                .retain(|item| item != name);
            return Ok(());
        };
        require_owned_container(&value, name, &self.prefix)?;
        if container_state(&value) == Some("running") {
            self.run_ok(["stop", "--time", "5", name]).await?;
        }
        let inspect = self.run_ok(["inspect", name]).await?;
        let value: Value = serde_json::from_slice(&inspect.stdout)?;
        require_owned_container(&value, name, &self.prefix)?;
        self.run_ok(["delete", name]).await?;
        self.records
            .lock()
            .unwrap()
            .containers
            .retain(|item| item != name);
        Ok(())
    }

    async fn inspect_container_if_present(&self, name: &str) -> Result<Option<Value>, TestError> {
        match self.run_ok(["inspect", name]).await {
            Ok(output) => Ok(Some(serde_json::from_slice(&output.stdout)?)),
            Err(inspect_error) => {
                let list = self
                    .run_ok(["list", "--all", "--format", "json"])
                    .await
                    .map_err(|list_error| {
                        format!(
                            "inspect failed: {inspect_error}; absence check failed: {list_error}"
                        )
                    })?;
                let value: Value = serde_json::from_slice(&list.stdout)?;
                if listed_container(&value, name).is_some() {
                    Err(
                        format!("inspect failed for listed container {name}: {inspect_error}")
                            .into(),
                    )
                } else {
                    Ok(None)
                }
            }
        }
    }

    pub async fn cleanup(&self) -> Result<(), TestError> {
        let mut errors = Vec::new();
        let containers = self.records.lock().unwrap().containers.clone();
        for name in containers.into_iter().rev() {
            if let Err(error) = self.delete_container(&name).await {
                errors.push(format!("container {name}: {error}"));
            }
        }
        let volumes = self.records.lock().unwrap().volumes.clone();
        for name in volumes.into_iter().rev() {
            match self.verify_volume(&name).await {
                Ok(false) => self
                    .records
                    .lock()
                    .unwrap()
                    .volumes
                    .retain(|item| item != &name),
                Ok(true) => match self.run_ok(["volume", "delete", &name]).await {
                    Ok(_) => self
                        .records
                        .lock()
                        .unwrap()
                        .volumes
                        .retain(|item| item != &name),
                    Err(error) => errors.push(format!("volume {name}: {error}")),
                },
                Err(error) => errors.push(format!("volume {name}: {error}")),
            }
        }
        if errors.is_empty() {
            let paths = self.records.lock().unwrap().paths.clone();
            for path in paths.into_iter().rev() {
                if path.starts_with(std::env::temp_dir())
                    && path
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with("gascan-feas-"))
                {
                    match fs::remove_dir_all(&path) {
                        Ok(_) => self
                            .records
                            .lock()
                            .unwrap()
                            .paths
                            .retain(|item| item != &path),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => self
                            .records
                            .lock()
                            .unwrap()
                            .paths
                            .retain(|item| item != &path),
                        Err(error) => errors.push(format!("path {}: {error}", path.display())),
                    }
                }
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; ").into())
        }
    }

    async fn verify_volume(&self, name: &str) -> Result<bool, TestError> {
        let output = self.run_ok(["volume", "list", "--format", "json"]).await?;
        let value: Value = serde_json::from_slice(&output.stdout)?;
        let Some(record) = volume_record(&value, name) else {
            return Ok(false);
        };
        require_owned_volume(record, name, &self.prefix)?;
        Ok(true)
    }
}

impl Drop for LiveContext {
    fn drop(&mut self) {
        if !self.drop_cleanup {
            return;
        }
        let Ok(records) = self.records.lock() else {
            return;
        };
        for name in records.containers.iter().rev() {
            if blocking_owned_container(name, &self.prefix) {
                cleanup_command(["stop", "--time", "5", name]);
                if blocking_owned_container(name, &self.prefix) {
                    cleanup_command(["delete", name]);
                }
            }
        }
        for name in records.volumes.iter().rev() {
            if blocking_owned_volume(name, &self.prefix) {
                cleanup_command(["volume", "delete", name]);
            }
        }
        for path in records.paths.iter().rev() {
            if path.starts_with(std::env::temp_dir())
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("gascan-feas-"))
            {
                let _ = fs::remove_dir_all(path);
            }
        }
    }
}

pub fn container_record(value: &Value) -> Option<&Value> {
    match value {
        Value::Object(_) => Some(value),
        Value::Array(values) if values.len() == 1 => values.first(),
        _ => None,
    }
}

pub fn container_state(value: &Value) -> Option<&str> {
    container_record(value)?
        .get("status")?
        .get("state")?
        .as_str()
}

pub fn configured_resource(value: &Value, name: &str) -> Option<u64> {
    container_record(value)?
        .get("configuration")?
        .get("resources")?
        .get(name)?
        .as_u64()
}

pub fn exact_workspace_bind<'a>(value: &'a Value, source: &Path) -> Option<&'a Value> {
    let mounts = container_record(value)?
        .get("configuration")?
        .get("mounts")?
        .as_array()?;
    let mut workspace_mounts = mounts
        .iter()
        .filter(|mount| mount.get("destination").and_then(Value::as_str) == Some("/workspace"));
    let mount = workspace_mounts.next()?;
    let exact_virtiofs = mount
        .get("type")
        .and_then(Value::as_object)
        .is_some_and(|kind| {
            kind.len() == 1 && kind.get("virtiofs") == Some(&Value::Object(Default::default()))
        });
    let broader_source_exists = mounts
        .iter()
        .filter(|candidate| !std::ptr::eq(*candidate, mount))
        .any(|candidate| {
            candidate
                .get("source")
                .and_then(Value::as_str)
                .map(Path::new)
                .is_some_and(|candidate_source| {
                    candidate_source != source && source.starts_with(candidate_source)
                })
        });
    if workspace_mounts.next().is_some()
        || !exact_virtiofs
        || mount.get("source").and_then(Value::as_str) != source.to_str()
        || mount
            .get("options")
            .and_then(Value::as_array)
            .is_some_and(|options| options.iter().any(|option| option.as_str() == Some("ro")))
        || broader_source_exists
    {
        return None;
    }
    Some(mount)
}

fn has_ownership_label(value: &Value) -> bool {
    value
        .get("labels")
        .and_then(Value::as_object)
        .and_then(|labels| labels.get("dev.gascan.test"))
        .and_then(Value::as_str)
        == Some("true")
}

fn require_owned_container<'a>(
    value: &'a Value,
    name: &str,
    prefix: &str,
) -> Result<&'a Value, TestError> {
    if !name.starts_with(prefix) {
        return Err(format!("unexpected container identity: {name}").into());
    }
    let record = container_record(value).ok_or("inspect did not contain exactly one container")?;
    let configuration = record
        .get("configuration")
        .ok_or("inspect missing configuration")?;
    if configuration.get("id").and_then(Value::as_str) != Some(name)
        || !has_ownership_label(configuration)
    {
        return Err(format!("ownership mismatch for container {name}").into());
    }
    Ok(record)
}

fn volume_record<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    value
        .as_array()?
        .iter()
        .find(|record| record.get("name").and_then(Value::as_str) == Some(name))
}

fn listed_container<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    value.as_array()?.iter().find(|record| {
        record
            .get("configuration")
            .and_then(|configuration| configuration.get("id"))
            .and_then(Value::as_str)
            == Some(name)
            || record.get("id").and_then(Value::as_str) == Some(name)
    })
}

fn require_owned_volume<'a>(
    record: &'a Value,
    name: &str,
    prefix: &str,
) -> Result<&'a Value, TestError> {
    if !name.starts_with(prefix)
        || record.get("name").and_then(Value::as_str) != Some(name)
        || !has_ownership_label(record)
    {
        return Err(format!("ownership mismatch for volume {name}").into());
    }
    Ok(record)
}

pub fn guest_argv(published: bool) -> Vec<String> {
    let script = if published {
        "while :; do printf 'HTTP/1.1 200 OK\\r\\nContent-Length: 2\\r\\nConnection: close\\r\\n\\r\\nok' | nc -l -p 8080; done"
    } else {
        "while :; do sleep 3600; done"
    };
    vec![
        IMAGE.to_owned(),
        "sh".to_owned(),
        "-c".to_owned(),
        script.to_owned(),
    ]
}

fn cleanup_command<const N: usize>(args: [&str; N]) {
    let Ok(mut child) = Command::new("container").args(args).spawn() else {
        return;
    };
    for _ in 0..100 {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn blocking_output<const N: usize>(args: [&str; N]) -> Option<Vec<u8>> {
    let mut child = Command::new("container")
        .args(args)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    for _ in 0..100 {
        if let Some(status) = child.try_wait().ok()? {
            if !status.success() {
                return None;
            }
            return child.stdout.take().and_then(|mut stdout| {
                use std::io::Read;
                let mut bytes = Vec::new();
                stdout.read_to_end(&mut bytes).ok().map(|_| bytes)
            });
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    None
}

fn blocking_owned_container(name: &str, prefix: &str) -> bool {
    blocking_output(["inspect", name])
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|value| require_owned_container(&value, name, prefix).is_ok())
}

fn blocking_owned_volume(name: &str, prefix: &str) -> bool {
    blocking_output(["volume", "list", "--format", "json"])
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| volume_record(&value, name).cloned())
        .is_some_and(|record| require_owned_volume(&record, name, prefix).is_ok())
}

pub fn publish_args(
    host: IpAddr,
    host_port: u16,
    guest_port: u16,
) -> Result<Vec<String>, TestError> {
    if host != IpAddr::V4(Ipv4Addr::LOCALHOST) {
        return Err("published ports must bind to IPv4 loopback".into());
    }
    Ok(vec![
        "--publish".into(),
        format!("{host}:{host_port}:{guest_port}"),
    ])
}

fn safe_child(root: &Path, name: &str) -> Result<PathBuf, TestError> {
    let path = Path::new(name);
    if path.components().count() != 1 {
        return Err("test file name must be one path component".into());
    }
    Ok(root.join(path))
}

fn shell_word(value: &str) -> Result<String, TestError> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        return Err("test shell word contains unsupported characters".into());
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, sync::Arc};

    use async_trait::async_trait;
    use gascan_core::runtime::RuntimeError;
    use serde_json::json;

    use super::*;

    struct ScriptedRunner(Mutex<VecDeque<Result<CommandOutput, RuntimeError>>>);

    #[async_trait]
    impl CommandRunner for ScriptedRunner {
        async fn run(&self, _spec: CommandSpec) -> Result<CommandOutput, RuntimeError> {
            self.0
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected command")
        }
    }

    fn output(value: Value) -> Result<CommandOutput, RuntimeError> {
        Ok(CommandOutput {
            status: 0,
            stdout: serde_json::to_vec(&value).unwrap(),
            stderr: Vec::new(),
        })
    }

    fn failure() -> Result<CommandOutput, RuntimeError> {
        Err(RuntimeError::CommandFailed {
            operation: "container".into(),
            exit_code: Some(1),
            stderr: "injected".into(),
        })
    }

    fn owned_container(name: &str) -> Value {
        json!([{"configuration":{"id":name,"labels":{"dev.gascan.test":"true"}},"status":{"state":"stopped"}}])
    }

    fn context(
        responses: Vec<Result<CommandOutput, RuntimeError>>,
        containers: Vec<String>,
        volumes: Vec<String>,
    ) -> LiveContext {
        LiveContext {
            runner: Arc::new(ScriptedRunner(Mutex::new(responses.into()))),
            prefix: "gascan-feas-42-case".into(),
            workspace: std::env::temp_dir().join("gascan-feas-42-case-workspace"),
            container: Mutex::new("gascan-feas-42-case-container".into()),
            volume: "gascan-feas-42-case-volume".into(),
            publish: None,
            records: Mutex::new(Records {
                containers,
                volumes,
                ..Records::default()
            }),
            drop_cleanup: false,
        }
    }

    #[tokio::test]
    async fn failed_create_collision_with_wrong_label_is_never_recorded() {
        let name = "gascan-feas-42-case-container";
        let ctx = context(
            vec![output(
                json!([{"configuration":{"id":name,"labels":{"dev.gascan.test":"false"}}}]),
            )],
            vec![],
            vec![],
        );
        assert!(
            ctx.reconcile_container(name)
                .await
                .unwrap_err()
                .to_string()
                .contains("ownership mismatch")
        );
        assert!(ctx.records.lock().unwrap().containers.is_empty());
    }

    #[tokio::test]
    async fn partial_cleanup_failure_is_retained_and_retry_succeeds() {
        let name = "gascan-feas-42-case-container";
        let owned = owned_container(name);
        let ctx = context(
            vec![
                output(owned.clone()),
                output(owned.clone()),
                failure(),
                output(owned.clone()),
                output(owned),
                output(json!(null)),
            ],
            vec![name.into()],
            vec![],
        );
        assert!(ctx.cleanup().await.is_err());
        assert_eq!(ctx.records.lock().unwrap().containers, [name]);
        ctx.cleanup().await.unwrap();
        assert!(ctx.records.lock().unwrap().containers.is_empty());
    }

    #[tokio::test]
    async fn absent_volume_is_removed_from_records_without_delete() {
        let name = "gascan-feas-42-case-volume";
        let ctx = context(vec![output(json!([]))], vec![], vec![name.into()]);
        ctx.cleanup().await.unwrap();
        assert!(ctx.records.lock().unwrap().volumes.is_empty());
    }

    #[tokio::test]
    async fn absent_container_is_removed_only_after_structured_list_confirmation() {
        let name = "gascan-feas-42-case-container";
        let ctx = context(
            vec![failure(), output(json!([]))],
            vec![name.into()],
            vec![],
        );
        ctx.cleanup().await.unwrap();
        assert!(ctx.records.lock().unwrap().containers.is_empty());
    }
}
