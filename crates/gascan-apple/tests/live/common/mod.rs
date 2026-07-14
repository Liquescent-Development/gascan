#![allow(dead_code)]

use std::{
    fs,
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
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
    cleaned: bool,
}

pub struct LiveContext {
    runner: ProcessRunner,
    prefix: String,
    workspace: PathBuf,
    container: Mutex<String>,
    volume: String,
    publish: Option<(u16, u16)>,
    records: Mutex<Records>,
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
            runner: ProcessRunner,
            prefix,
            workspace: workspace.clone(),
            container: Mutex::new(container),
            volume: volume.clone(),
            publish,
            records: Mutex::new(Records {
                volumes: vec![volume],
                paths: vec![workspace],
                ..Records::default()
            }),
        };

        ctx.run_ok([
            "volume",
            "create",
            "--label",
            LABEL,
            "-s",
            "104857600",
            &ctx.volume,
        ])
        .await?;
        if let Err(error) = ctx.create_container().await {
            let _ = ctx.cleanup().await;
            return Err(error);
        }
        Ok(ctx)
    }

    async fn create_container(&self) -> Result<(), TestError> {
        let name = self.container.lock().unwrap().clone();
        self.records.lock().unwrap().containers.push(name.clone());
        let mount = format!(
            "type=bind,source={},target=/workspace",
            self.workspace.display()
        );
        let volume = format!("{}:/opt/gascan", self.volume);
        let mut args = vec![
            "run".to_owned(),
            "--name".to_owned(),
            name,
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
        args.extend([
            IMAGE.to_owned(), "sh".to_owned(), "-c".to_owned(),
            if self.publish.is_some() {
                "while :; do printf 'HTTP/1.0 200 OK\\r\\nContent-Length: 6\\r\\n\\r\\ngascan' | nc -l -p 8080; done".to_owned()
            } else {
                "while :; do sleep 3600; done".to_owned()
            },
        ]);
        self.run_vec(args).await?;
        Ok(())
    }

    pub async fn recreate_container(&self) -> Result<(), TestError> {
        let old = self.container.lock().unwrap().clone();
        self.run_ok(["stop", "--time", "5", &old]).await?;
        self.run_ok(["delete", &old]).await?;
        self.records
            .lock()
            .unwrap()
            .containers
            .retain(|item| item != &old);
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
        Ok(find_status(&self.inspect().await?).is_some_and(|status| {
            status.eq_ignore_ascii_case("running") || status.starts_with("Up ")
        }))
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

    pub async fn cleanup(&self) -> Result<(), TestError> {
        let (containers, volumes, networks, paths) = {
            let mut records = self.records.lock().unwrap();
            if records.cleaned {
                return Ok(());
            }
            records.cleaned = true;
            (
                std::mem::take(&mut records.containers),
                std::mem::take(&mut records.volumes),
                std::mem::take(&mut records.networks),
                std::mem::take(&mut records.paths),
            )
        };
        for name in containers.into_iter().rev() {
            let _ = self.run_ok(["stop", "--time", "5", &name]).await;
            let _ = self.run_ok(["delete", &name]).await;
        }
        for name in networks.into_iter().rev() {
            let _ = self.run_ok(["network", "delete", &name]).await;
        }
        for name in volumes.into_iter().rev() {
            let _ = self.run_ok(["volume", "delete", &name]).await;
        }
        for path in paths.into_iter().rev() {
            if path.starts_with(std::env::temp_dir())
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("gascan-feas-"))
            {
                let _ = fs::remove_dir_all(path);
            }
        }
        Ok(())
    }
}

impl Drop for LiveContext {
    fn drop(&mut self) {
        let Ok(mut records) = self.records.lock() else {
            return;
        };
        if records.cleaned {
            return;
        }
        records.cleaned = true;
        for name in records.containers.iter().rev() {
            cleanup_command(["stop", "--time", "5", name]);
            cleanup_command(["delete", name]);
        }
        for name in records.networks.iter().rev() {
            cleanup_command(["network", "delete", name]);
        }
        for name in records.volumes.iter().rev() {
            cleanup_command(["volume", "delete", name]);
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

fn find_status(value: &Value) -> Option<&str> {
    match value {
        Value::Object(fields) => fields
            .iter()
            .find(|(key, value)| key.eq_ignore_ascii_case("status") && value.is_string())
            .and_then(|(_, value)| value.as_str())
            .or_else(|| fields.values().find_map(find_status)),
        Value::Array(values) => values.iter().find_map(find_status),
        _ => None,
    }
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

pub async fn wait_until(mut check: impl FnMut() -> bool) -> bool {
    for _ in 0..50 {
        if check() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}
