#![allow(dead_code)]

use std::{
    collections::HashSet,
    fs,
    io::Read,
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use gascan_apple::{
    AppleAttach, AttachSession, CommandOutput, CommandRunner, CommandSpec, ProcessRunner,
};
use serde_json::Value;

pub type TestError = Box<dyn std::error::Error + Send + Sync>;

const IMAGE: &str = "docker.io/library/alpine:3.20";
const LABEL: &str = "dev.gascan.test=true";
const OWNER_LABEL: &str = "dev.gascan.test.owner";

#[derive(Default)]
struct Records {
    containers: Vec<String>,
    volumes: Vec<String>,
    networks: Vec<String>,
    paths: Vec<PathBuf>,
    owner_token: String,
    usable_containers: HashSet<String>,
    usable_volumes: HashSet<String>,
}

pub struct LiveContext {
    runner: Arc<dyn CommandRunner>,
    prefix: String,
    workspace: PathBuf,
    container: Mutex<String>,
    volume: String,
    publish: Option<(u16, u16)>,
    records: Mutex<Records>,
    owner_token: String,
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
        let owner_token = random_owner_token()?;
        let owner_label = format!("{OWNER_LABEL}={owner_token}");
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
                owner_token: owner_token.clone(),
                ..Records::default()
            }),
            owner_token,
            drop_cleanup: true,
        };

        let volume_result = ctx
            .run_ok([
                "volume",
                "create",
                "--label",
                LABEL,
                "--label",
                &owner_label,
                "-s",
                "104857600",
                &ctx.volume,
            ])
            .await;
        match volume_result {
            Ok(_) => {
                ctx.record_pending_volume(&ctx.volume);
                ctx.verify_and_mark_volume(&ctx.volume, false).await?;
            }
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
        let owner_label = format!("{OWNER_LABEL}={}", self.owner_token);
        let mut args = vec![
            "run".to_owned(),
            "--name".to_owned(),
            name.clone(),
            "--label".to_owned(),
            LABEL.to_owned(),
            "--label".to_owned(),
            owner_label,
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
                self.record_pending_container(&name);
                self.verify_and_mark_container(&name, true, false).await
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

    pub async fn attach<I, A>(&self, argv: I, tty: bool) -> Result<AttachSession, TestError>
    where
        I: IntoIterator<Item = A>,
        A: AsRef<str>,
    {
        let name = self.container.lock().unwrap().clone();
        let helper = std::env::var_os("GASCAN_APPLE_ATTACH_HELPER")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/gascan-apple-attach")
            });
        Ok(AppleAttach::new(helper.to_string_lossy())
            .exec(name, argv, tty)
            .await?)
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
        self.verify_and_mark_container(name, false, true).await
    }

    fn record_pending_container(&self, name: &str) {
        let mut records = self.records.lock().unwrap();
        if !records.containers.iter().any(|item| item == name) {
            records.containers.push(name.to_owned());
        }
    }

    async fn verify_and_mark_container(
        &self,
        name: &str,
        require_present: bool,
        record_on_success: bool,
    ) -> Result<(), TestError> {
        let Some(value) = self.inspect_container_if_present(name).await? else {
            return if require_present {
                Err(
                    format!("created container {name} is absent during ownership verification")
                        .into(),
                )
            } else {
                Ok(())
            };
        };
        require_owned_container(&value, name, &self.prefix, &self.owner_token)?;
        let mut records = self.records.lock().unwrap();
        if record_on_success && !records.containers.iter().any(|item| item == name) {
            records.containers.push(name.to_owned());
        }
        records.usable_containers.insert(name.to_owned());
        Ok(())
    }

    async fn reconcile_volume(&self, name: &str) -> Result<(), TestError> {
        let output = self.run_ok(["volume", "list", "--format", "json"]).await?;
        let value: Value = serde_json::from_slice(&output.stdout)?;
        match volume_record(&value, name) {
            None => Ok(()),
            Some(record) => {
                require_owned_volume(record, name, &self.prefix, &self.owner_token)?;
                let mut records = self.records.lock().unwrap();
                if !records.volumes.iter().any(|item| item == name) {
                    records.volumes.push(name.to_owned());
                }
                records.usable_volumes.insert(name.to_owned());
                Ok(())
            }
        }
    }

    fn record_pending_volume(&self, name: &str) {
        let mut records = self.records.lock().unwrap();
        if !records.volumes.iter().any(|item| item == name) {
            records.volumes.push(name.to_owned());
        }
    }

    async fn verify_and_mark_volume(
        &self,
        name: &str,
        record_on_success: bool,
    ) -> Result<(), TestError> {
        let output = self.run_ok(["volume", "list", "--format", "json"]).await?;
        let value: Value = serde_json::from_slice(&output.stdout)?;
        let record = volume_record(&value, name).ok_or_else(|| {
            format!("created volume {name} is absent during ownership verification")
        })?;
        require_owned_volume(record, name, &self.prefix, &self.owner_token)?;
        let mut records = self.records.lock().unwrap();
        if record_on_success && !records.volumes.iter().any(|item| item == name) {
            records.volumes.push(name.to_owned());
        }
        records.usable_volumes.insert(name.to_owned());
        Ok(())
    }

    async fn delete_container(&self, name: &str) -> Result<(), TestError> {
        let Some(value) = self.inspect_container_if_present(name).await? else {
            let mut records = self.records.lock().unwrap();
            records.containers.retain(|item| item != name);
            records.usable_containers.remove(name);
            return Ok(());
        };
        require_owned_container(&value, name, &self.prefix, &self.owner_token)?;
        if container_state(&value) == Some("running") {
            self.run_ok(["stop", "--time", "5", name]).await?;
        }
        let inspect = self.run_ok(["inspect", name]).await?;
        let value: Value = serde_json::from_slice(&inspect.stdout)?;
        require_owned_container(&value, name, &self.prefix, &self.owner_token)?;
        self.run_ok(["delete", name]).await?;
        let mut records = self.records.lock().unwrap();
        records.containers.retain(|item| item != name);
        records.usable_containers.remove(name);
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
                Ok(false) => {
                    let mut records = self.records.lock().unwrap();
                    records.volumes.retain(|item| item != &name);
                    records.usable_volumes.remove(&name);
                }
                Ok(true) => match self.run_ok(["volume", "delete", &name]).await {
                    Ok(_) => {
                        let mut records = self.records.lock().unwrap();
                        records.volumes.retain(|item| item != &name);
                        records.usable_volumes.remove(&name);
                    }
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
        require_owned_volume(record, name, &self.prefix, &self.owner_token)?;
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
            if blocking_owned_container(name, &self.prefix, &records.owner_token) {
                cleanup_command(["stop", "--time", "5", name]);
                if blocking_owned_container(name, &self.prefix, &records.owner_token) {
                    cleanup_command(["delete", name]);
                }
            }
        }
        for name in records.volumes.iter().rev() {
            if blocking_owned_volume(name, &self.prefix, &records.owner_token) {
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
    let canonical_mount_source = mount
        .get("source")
        .and_then(Value::as_str)
        .and_then(|path| fs::canonicalize(path).ok());
    let broader_source_exists = mounts
        .iter()
        .filter(|candidate| !std::ptr::eq(*candidate, mount))
        .any(|candidate| {
            candidate
                .get("source")
                .and_then(Value::as_str)
                .and_then(|path| fs::canonicalize(path).ok())
                .is_some_and(|candidate_source| {
                    candidate_source != source && source.starts_with(&candidate_source)
                })
        });
    if workspace_mounts.next().is_some()
        || !exact_virtiofs
        || canonical_mount_source.as_deref() != Some(source)
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

fn has_ownership_labels(value: &Value, owner_token: &str) -> bool {
    let Some(labels) = value.get("labels").and_then(Value::as_object) else {
        return false;
    };
    labels.get("dev.gascan.test").and_then(Value::as_str) == Some("true")
        && labels.get(OWNER_LABEL).and_then(Value::as_str) == Some(owner_token)
}

fn require_owned_container<'a>(
    value: &'a Value,
    name: &str,
    prefix: &str,
    owner_token: &str,
) -> Result<&'a Value, TestError> {
    if !name.starts_with(prefix) {
        return Err(format!("unexpected container identity: {name}").into());
    }
    let record = container_record(value).ok_or("inspect did not contain exactly one container")?;
    let configuration = record
        .get("configuration")
        .ok_or("inspect missing configuration")?;
    if configuration.get("id").and_then(Value::as_str) != Some(name)
        || !has_ownership_labels(configuration, owner_token)
    {
        return Err(format!("ownership mismatch for container {name}").into());
    }
    Ok(record)
}

fn volume_record<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    value.as_array()?.iter().find(|record| {
        record.get("id").and_then(Value::as_str) == Some(name)
            && record
                .get("configuration")
                .and_then(|configuration| configuration.get("name"))
                .and_then(Value::as_str)
                == Some(name)
    })
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
    owner_token: &str,
) -> Result<&'a Value, TestError> {
    let configuration = record
        .get("configuration")
        .ok_or("volume record missing configuration")?;
    if !name.starts_with(prefix)
        || record.get("id").and_then(Value::as_str) != Some(name)
        || configuration.get("name").and_then(Value::as_str) != Some(name)
        || !has_ownership_labels(configuration, owner_token)
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

fn blocking_owned_container(name: &str, prefix: &str, owner_token: &str) -> bool {
    blocking_output(["inspect", name])
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|value| require_owned_container(&value, name, prefix, owner_token).is_ok())
}

fn blocking_owned_volume(name: &str, prefix: &str, owner_token: &str) -> bool {
    blocking_output(["volume", "list", "--format", "json"])
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| volume_record(&value, name).cloned())
        .is_some_and(|record| require_owned_volume(&record, name, prefix, owner_token).is_ok())
}

fn random_owner_token() -> Result<String, TestError> {
    let mut bytes = [0_u8; 16];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
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

    const TOKEN: &str = "00112233445566778899aabbccddeeff";

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
        container_with(name, TOKEN, "stopped")
    }

    fn container_with(name: &str, token: &str, state: &str) -> Value {
        json!([{"configuration":{"id":name,"labels":{"dev.gascan.test":"true","dev.gascan.test.owner":token}},"status":{"state":state}}])
    }

    fn volume_with(name: &str, token: &str) -> Value {
        json!([{"configuration":{
            "creationDate":"2026-07-14T00:00:00Z","driver":"local","format":"ext4",
            "labels":{"dev.gascan.test":"true","dev.gascan.test.owner":token},
            "name":name,"options":{"size":"104857600"},"sizeInBytes":104857600,"source":"/tmp/volume.img"
        },"id":name}])
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
                owner_token: TOKEN.into(),
                ..Records::default()
            }),
            owner_token: TOKEN.into(),
            drop_cleanup: false,
        }
    }

    #[tokio::test]
    async fn failed_create_collision_with_wrong_owner_token_is_never_recorded() {
        let name = "gascan-feas-42-case-container";
        let ctx = context(
            vec![output(container_with(
                name,
                "ffeeddccbbaa99887766554433221100",
                "stopped",
            ))],
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
    async fn failed_volume_collision_with_wrong_owner_token_is_never_recorded() {
        let name = "gascan-feas-42-case-volume";
        let ctx = context(
            vec![output(volume_with(
                name,
                "ffeeddccbbaa99887766554433221100",
            ))],
            vec![],
            vec![],
        );
        assert!(
            ctx.reconcile_volume(name)
                .await
                .unwrap_err()
                .to_string()
                .contains("ownership mismatch")
        );
        assert!(ctx.records.lock().unwrap().volumes.is_empty());
    }

    #[test]
    fn apple_volume_schema_requires_matching_id_name_and_nested_token_labels() {
        let name = "gascan-feas-42-case-volume";
        let exact = volume_with(name, TOKEN);
        let record = volume_record(&exact, name).unwrap();
        assert!(require_owned_volume(record, name, "gascan-feas-42-case", TOKEN).is_ok());

        for malformed in [
            json!([{"id":name,"configuration":{"name":"different","labels":{"dev.gascan.test":"true","dev.gascan.test.owner":TOKEN}}}]),
            json!([{"id":"different","configuration":{"name":name,"labels":{"dev.gascan.test":"true","dev.gascan.test.owner":TOKEN}}}]),
        ] {
            assert!(volume_record(&malformed, name).is_none());
        }

        let top_level_labels = json!([{"id":name,"configuration":{"name":name},"labels":{"dev.gascan.test":"true","dev.gascan.test.owner":TOKEN}}]);
        let record = volume_record(&top_level_labels, name).unwrap();
        assert!(require_owned_volume(record, name, "gascan-feas-42-case", TOKEN).is_err());

        let wrong_token = volume_with(name, "ffeeddccbbaa99887766554433221100");
        let record = volume_record(&wrong_token, name).unwrap();
        assert!(require_owned_volume(record, name, "gascan-feas-42-case", TOKEN).is_err());
    }

    #[tokio::test]
    async fn successful_post_create_verification_records_exact_token() {
        let container = "gascan-feas-42-case-container";
        let volume = "gascan-feas-42-case-volume";
        let ctx = context(
            vec![
                output(container_with(container, TOKEN, "running")),
                output(volume_with(volume, TOKEN)),
            ],
            vec![],
            vec![],
        );
        ctx.record_pending_container(container);
        ctx.verify_and_mark_container(container, true, false)
            .await
            .unwrap();
        ctx.record_pending_volume(volume);
        ctx.verify_and_mark_volume(volume, false).await.unwrap();
        let records = ctx.records.lock().unwrap();
        assert_eq!(records.containers, [container]);
        assert_eq!(records.volumes, [volume]);
        assert_eq!(records.owner_token, TOKEN);
        assert_eq!(records.usable_containers.len(), 1);
        assert_eq!(records.usable_volumes.len(), 1);
    }

    #[tokio::test]
    async fn transient_post_create_inspect_failure_leaves_pending_for_safe_cleanup_retry() {
        let name = "gascan-feas-42-case-container";
        let owned = container_with(name, TOKEN, "stopped");
        let listed = json!([{"configuration":{"id":name}}]);
        let ctx = context(
            vec![
                failure(),
                output(listed),
                output(owned.clone()),
                output(owned),
                output(json!(null)),
            ],
            vec![],
            vec![],
        );
        ctx.record_pending_container(name);
        assert!(
            ctx.verify_and_mark_container(name, true, false)
                .await
                .is_err()
        );
        assert_eq!(ctx.records.lock().unwrap().containers, [name]);
        assert!(ctx.records.lock().unwrap().usable_containers.is_empty());
        ctx.cleanup().await.unwrap();
        assert!(ctx.records.lock().unwrap().containers.is_empty());
    }

    #[tokio::test]
    async fn pending_wrong_token_is_retained_and_never_deleted() {
        let name = "gascan-feas-42-case-container";
        let ctx = context(
            vec![output(container_with(
                name,
                "ffeeddccbbaa99887766554433221100",
                "stopped",
            ))],
            vec![name.into()],
            vec![],
        );
        assert!(ctx.cleanup().await.is_err());
        assert_eq!(ctx.records.lock().unwrap().containers, [name]);
        assert!(ctx.records.lock().unwrap().usable_containers.is_empty());
    }

    #[tokio::test]
    async fn owner_token_mismatch_after_stop_prevents_delete() {
        let name = "gascan-feas-42-case-container";
        let ctx = context(
            vec![
                output(container_with(name, TOKEN, "running")),
                output(json!(null)),
                output(container_with(
                    name,
                    "ffeeddccbbaa99887766554433221100",
                    "stopped",
                )),
            ],
            vec![name.into()],
            vec![],
        );
        assert!(
            ctx.cleanup()
                .await
                .unwrap_err()
                .to_string()
                .contains("ownership mismatch")
        );
        assert_eq!(ctx.records.lock().unwrap().containers, [name]);
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
