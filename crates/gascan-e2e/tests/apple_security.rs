#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod apple_common;

use apple_common::{AppleE2e, TestResult};
use std::{
    fs,
    io::{Read as _, Write as _},
    net::{Ipv4Addr, TcpListener, TcpStream},
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const SECRET: &str = "gascan-security-synthetic-secret-7f29b364";
const SECURITY_SCRIPTS: [&str; 5] = [
    "assert-unreachable.sh",
    "host-boundary.sh",
    "offline-network.sh",
    "ports.sh",
    "resources.sh",
];

struct LoopbackServer {
    port: u16,
    stopping: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

struct CleanupFile(PathBuf);

impl Drop for CleanupFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

impl LoopbackServer {
    fn start() -> TestResult<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let stopping = Arc::new(AtomicBool::new(false));
        let thread_stopping = Arc::clone(&stopping);
        let thread = thread::spawn(move || {
            while !thread_stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nhost",
                        );
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            port,
            stopping,
            thread: Some(thread),
        })
    }
}

impl Drop for LoopbackServer {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        let _ = TcpStream::connect((Ipv4Addr::LOCALHOST, self.port));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct OwnedDnsRoute<'a> {
    domain: String,
    pending: bool,
    env: &'a AppleE2e,
}

impl<'a> OwnedDnsRoute<'a> {
    fn create(env: &'a AppleE2e) -> TestResult<Self> {
        let domain = format!("gascan-{}.test", owner_token()?);
        let before = dns_domains()?;
        if before.iter().any(|item| item == &domain) {
            return Err("test-owned DNS route already exists".into());
        }
        let mut route = Self {
            domain,
            pending: true,
            env,
        };
        route.env.record_dns_domain(Some(&route.domain))?;
        let create = bounded_output(
            Command::new("sudo")
                .args([
                    "-n",
                    "container",
                    "system",
                    "dns",
                    "create",
                    "--localhost",
                    "203.0.113.113",
                    &route.domain,
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
            Duration::from_secs(30),
        );
        let inventory = dns_domains();
        let count = inventory.as_ref().map_or(usize::MAX, |domains| {
            domains.iter().filter(|item| *item == &route.domain).count()
        });
        if count == 0 {
            route.pending = false;
            route.env.record_dns_domain(None)?;
        }
        if create.as_ref().is_ok_and(|output| output.status.success()) && count == 1 {
            return Ok(route);
        }
        let detail = match (create, inventory) {
            (Err(create), Err(inventory)) => {
                format!("create failed: {create}; reconciliation failed: {inventory}")
            }
            (Err(create), Ok(_)) => format!("create failed: {create}"),
            (Ok(output), Err(inventory)) => format!(
                "create status {:?}: {}; reconciliation failed: {inventory}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr)
            ),
            (Ok(output), Ok(_)) => format!(
                "create status {:?}: {}; route count after create was {count}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr)
            ),
        };
        match route.cleanup() {
            Ok(()) => Err(format!("test-owned DNS route creation failed: {detail}").into()),
            Err(cleanup) => Err(format!(
                "test-owned DNS route creation failed: {detail}; cleanup failed: {cleanup}"
            )
            .into()),
        }
    }

    fn url(&self, port: u16) -> String {
        format!("http://{}:{port}", self.domain)
    }

    fn cleanup(&mut self) -> TestResult {
        if !self.pending {
            self.env.record_dns_domain(None)?;
            return Ok(());
        }
        let count = dns_domains()?
            .iter()
            .filter(|item| *item == &self.domain)
            .count();
        if count == 0 {
            self.pending = false;
            self.env.record_dns_domain(None)?;
            return Ok(());
        }
        if count != 1 || !owned_domain(&self.domain) {
            return Err("refusing ambiguous DNS route cleanup".into());
        }
        let output = bounded_output(
            Command::new("sudo")
                .args(["-n", "container", "system", "dns", "delete", &self.domain])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
            Duration::from_secs(30),
        )?;
        if !output.status.success() || dns_domains()?.iter().any(|item| item == &self.domain) {
            return Err("test-owned DNS route cleanup failed".into());
        }
        self.pending = false;
        self.env.record_dns_domain(None)?;
        Ok(())
    }
}

impl Drop for OwnedDnsRoute<'_> {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

#[test]
#[ignore = "requires supported Apple runtime and the locked workspace image"]
fn real_macos_security_acceptance() -> TestResult {
    let env = AppleE2e::new("gate4-security")?;
    install_security_fixture(&env)?;
    let root = Path::new(env.root());
    let outside = root
        .parent()
        .ok_or("security root has no session parent")?
        .join(format!("synthetic-outside-{}", owner_token()?));
    fs::write(&outside, "synthetic-outside-only")?;
    let outside_cleanup = CleanupFile(outside.clone());

    write_manifest(root, "offline", "root", None, false, false)?;
    let root_request = env.invoke(["up", root.to_str().ok_or("non-UTF-8 root")?, "--json"])?;
    require_failure_code("root user request", &root_request, "unsupported_user")?;
    env.assert_no_owned_resources()?;

    write_manifest(root, "offline", "workspace", None, false, false)?;
    env.success(["up", root.to_str().ok_or("non-UTF-8 root")?])?;
    let boundary = env.invoke_with_env(
        [
            "--sandbox",
            env.id(),
            "run",
            "--",
            "bash",
            "/workspace/.gascan/security/host-boundary.sh",
            outside.to_str().ok_or("non-UTF-8 sentinel path")?,
            &std::env::var("USER")?,
        ],
        "GASCAN_SECURITY_SENTINEL",
        SECRET,
    )?;
    require_success("host-boundary", &boundary)?;
    assert_private_daemon_socket(&env)?;
    assert_runtime_policy(&env, true, None)?;
    let resources = env.success([
        "--sandbox",
        env.id(),
        "run",
        "--",
        "bash",
        "/workspace/.gascan/security/resources.sh",
    ])?;
    require_success("resources", &resources)?;
    env.success(["--sandbox", env.id(), "run", "--", "true"])?;
    assert_secret_absent(&env, &boundary)?;

    env.success(["--sandbox", env.id(), "destroy", "--yes"])?;
    let host = LoopbackServer::start()?;
    let mut route = OwnedDnsRoute::create(&env)?;
    if std::env::var_os("GASCAN_SECURITY_ABORT_AFTER_DNS_CREATE")
        .is_some_and(|value| value == std::ffi::OsStr::new("1"))
    {
        std::process::abort();
    }
    let host_url = route.url(host.port);
    let route_result = (|| -> TestResult {
        write_manifest(root, "networked", "workspace", None, false, false)?;
        env.success(["up", root.to_str().ok_or("non-UTF-8 root")?])?;
        env.success([
            "--sandbox",
            env.id(),
            "run",
            "--",
            "curl",
            "--silent",
            "--show-error",
            "--fail",
            "--max-time",
            "4",
            &host_url,
        ])?;

        env.success(["--sandbox", env.id(), "destroy", "--yes"])?;
        write_manifest(root, "offline", "workspace", None, false, false)?;
        env.success(["up", root.to_str().ok_or("non-UTF-8 root")?])?;
        assert_runtime_policy(&env, true, None)?;
        run_offline_probe(&env, &host_url)?;
        let root_id =
            env.success(["--sandbox", env.id(), "run", "--", "sudo", "-n", "id", "-u"])?;
        if root_id.stdout != b"0\r\n" && root_id.stdout != b"0\n" {
            return Err(format!("root guest reported unexpected uid: {:?}", root_id.stdout).into());
        }
        let root_offline = env.invoke([
            "--sandbox",
            env.id(),
            "run",
            "--",
            "sudo",
            "-n",
            "bash",
            "/workspace/.gascan/security/offline-network.sh",
            &host_url,
        ])?;
        require_success("offline-network-as-root", &root_offline)
    })();
    let route_cleanup = route.cleanup();
    combine_test_and_cleanup("test-owned DNS route", route_result, route_cleanup)?;

    env.success(["--sandbox", env.id(), "destroy", "--yes"])?;
    let reservation = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = reservation.local_addr()?.port();
    drop(reservation);
    write_manifest(root, "networked", "workspace", Some(port), false, false)?;
    env.success(["up", root.to_str().ok_or("non-UTF-8 root")?])?;
    env.success([
        "--sandbox",
        env.id(),
        "run",
        "--",
        "bash",
        "/workspace/.gascan/security/ports.sh",
        &port.to_string(),
    ])?;
    wait_for_http(port, true)?;
    assert_runtime_policy(&env, false, Some(port))?;

    env.success(["--sandbox", env.id(), "destroy", "--yes"])?;
    write_manifest(root, "networked", "workspace", None, false, false)?;
    env.success(["up", root.to_str().ok_or("non-UTF-8 root")?])?;
    env.success([
        "--sandbox",
        env.id(),
        "run",
        "--",
        "bash",
        "/workspace/.gascan/security/ports.sh",
        &port.to_string(),
    ])?;
    wait_for_http(port, false)?;

    env.success(["--sandbox", env.id(), "destroy", "--yes"])?;
    write_manifest(root, "offline", "workspace", None, true, false)?;
    let disk = env.invoke(["up", root.to_str().ok_or("non-UTF-8 root")?, "--json"])?;
    require_failure_code("disk request", &disk, "disk_control_unsupported")?;
    env.assert_no_owned_resources()?;

    write_manifest(root, "offline", "workspace", None, false, true)?;
    let process = env.invoke(["up", root.to_str().ok_or("non-UTF-8 root")?, "--json"])?;
    require_failure_code("process request", &process, "invalid_request")?;
    env.assert_no_owned_resources()?;
    drop(outside_cleanup);
    Ok(())
}

fn install_security_fixture(env: &AppleE2e) -> TestResult {
    let root = Path::new(env.root());
    let destination = root.join(".gascan/security");
    fs::create_dir_all(&destination)?;
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    for name in SECURITY_SCRIPTS {
        let target = destination.join(name);
        fs::copy(repository.join("tests/security").join(name), &target)?;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755))?;
    }
    fs::write(root.join("workspace-sentinel"), "workspace-visible")?;
    Ok(())
}

fn write_manifest(
    root: &Path,
    network: &str,
    user: &str,
    port: Option<u16>,
    disk: bool,
    process: bool,
) -> TestResult {
    let mut source = format!(
        "version = 1\nname = 'gate4-security'\nnetwork = '{network}'\nuser = '{user}'\n\n[resources]\ncpus = 1\nmemory = '256MiB'\n"
    );
    if disk {
        source.push_str("disk = '1GiB'\n");
    }
    if process {
        source.push_str("process_count = 32\n");
    }
    if let Some(port) = port {
        source.push_str(&format!("\n[ports]\nsecurity = {port}\n"));
    }
    fs::write(root.join("gascan.toml"), source)?;
    Ok(())
}

fn run_offline_probe(env: &AppleE2e, host_url: &str) -> TestResult {
    let output = env.invoke([
        "--sandbox",
        env.id(),
        "run",
        "--",
        "bash",
        "/workspace/.gascan/security/offline-network.sh",
        host_url,
    ])?;
    require_success("offline-network", &output)
}

fn assert_private_daemon_socket(env: &AppleE2e) -> TestResult {
    let socket = env.runtime_root().join("gascan/gascand.sock");
    let metadata = fs::symlink_metadata(&socket)?;
    if metadata.mode() & 0o777 != 0o600 || metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err("daemon socket is not exact mode 0600 and owned by the effective uid".into());
    }
    std::os::unix::net::UnixStream::connect(&socket)?;
    let current_uid = rustix::process::geteuid().as_raw();
    let current = gascand::PeerUid::new(current_uid);
    let foreign = gascand::PeerUid::new(current_uid.wrapping_add(1));
    if gascand::validate_peer_uid(foreign, current).is_ok() {
        return Err("daemon peer validator accepted a foreign uid".into());
    }
    Ok(())
}

fn assert_secret_absent(env: &AppleE2e, output: &Output) -> TestResult {
    let logs = env.success(["--sandbox", env.id(), "logs"])?;
    for bytes in [
        &output.stdout[..],
        &output.stderr,
        &logs.stdout,
        &logs.stderr,
    ] {
        if bytes
            .windows(SECRET.len())
            .any(|window| window == SECRET.as_bytes())
        {
            return Err("synthetic secret appeared in CLI output or logs".into());
        }
    }
    if env.bounded_daemon_stderr().contains(SECRET) {
        return Err("synthetic secret appeared in bounded daemon diagnostics".into());
    }
    Ok(())
}

fn assert_runtime_policy(env: &AppleE2e, offline: bool, port: Option<u16>) -> TestResult {
    let inspect = owned_container_inspect(env.id())?;
    let configuration = inspect
        .get("configuration")
        .and_then(serde_json::Value::as_object)
        .ok_or("container inspect lacks configuration object")?;
    let resources = configuration
        .get("resources")
        .and_then(serde_json::Value::as_object)
        .ok_or("container inspect lacks configured resources")?;
    if resources.get("cpus").and_then(serde_json::Value::as_u64) != Some(1)
        || resources
            .get("memoryInBytes")
            .and_then(serde_json::Value::as_u64)
            != Some(268_435_456)
    {
        return Err("runtime CPU/memory configuration differs from policy".into());
    }
    if resources.keys().any(|key| {
        let key = key.to_ascii_lowercase();
        key.contains("disk") || key.contains("process") || key.contains("pid")
    }) {
        return Err("runtime inspect claims an unsupported disk/process control".into());
    }
    let networks = configuration
        .get("networks")
        .and_then(serde_json::Value::as_array)
        .ok_or("container inspect lacks structured network attachments")?;
    if offline != networks.is_empty() {
        return Err("structured network attachments contradict manifest policy".into());
    }
    if let Some(port) = port {
        exact_published_port(configuration, port)?;
    } else if !configuration
        .get("publishedPorts")
        .and_then(serde_json::Value::as_array)
        .is_some_and(Vec::is_empty)
    {
        return Err("runtime configuration has undeclared or malformed published ports".into());
    }
    Ok(())
}

fn exact_published_port(
    configuration: &serde_json::Map<String, serde_json::Value>,
    port: u16,
) -> TestResult {
    let published = configuration
        .get("publishedPorts")
        .and_then(serde_json::Value::as_array)
        .ok_or("runtime configuration lacks structured publishedPorts")?;
    let [mapping] = published.as_slice() else {
        return Err("runtime configuration does not contain exactly one published port".into());
    };
    let exact = mapping
        .get("hostAddress")
        .and_then(serde_json::Value::as_str)
        == Some("127.0.0.1")
        && mapping.get("hostPort").and_then(serde_json::Value::as_u64) == Some(u64::from(port))
        && mapping
            .get("containerPort")
            .and_then(serde_json::Value::as_u64)
            == Some(u64::from(port))
        && mapping.get("proto").and_then(serde_json::Value::as_str) == Some("tcp")
        && mapping.get("count").and_then(serde_json::Value::as_u64) == Some(1);
    if !exact {
        return Err("published port is not the exact loopback TCP one-to-one mapping".into());
    }
    Ok(())
}

fn owned_container_inspect(id: &str) -> TestResult<serde_json::Value> {
    let output = bounded_output(
        Command::new("container")
            .args(["inspect", id])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
        Duration::from_secs(15),
    )?;
    if !output.status.success() {
        return Err("unable to inspect exact security container".into());
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let record = value
        .as_array()
        .and_then(|records| (records.len() == 1).then(|| &records[0]))
        .ok_or("security container inspect is absent or ambiguous")?;
    let labels = record
        .pointer("/configuration/labels")
        .and_then(serde_json::Value::as_object)
        .ok_or("security container lacks ownership labels")?;
    if labels
        .get("dev.gascan.managed-by")
        .and_then(serde_json::Value::as_str)
        != Some("gascan")
        || labels
            .get("dev.gascan.sandbox-id")
            .and_then(serde_json::Value::as_str)
            != Some(id)
    {
        return Err("security container ownership mismatch".into());
    }
    Ok(record.clone())
}

fn wait_for_http(port: u16, expected: bool) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let reachable = TcpStream::connect_timeout(
            &(Ipv4Addr::LOCALHOST, port).into(),
            Duration::from_millis(200),
        )
        .is_ok();
        if reachable == expected {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(
                format!("loopback port {port} reachability did not become {expected}").into(),
            );
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn require_success(name: &str, output: &Output) -> TestResult {
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{name} failed with {:?}: stdout={} stderr={}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into())
    }
}

fn combine_test_and_cleanup(resource: &str, test: TestResult, cleanup: TestResult) -> TestResult {
    match (test, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(test), Ok(())) => Err(test),
        (Ok(()), Err(cleanup)) => Err(format!("{resource} cleanup failed: {cleanup}").into()),
        (Err(test), Err(cleanup)) => Err(format!(
            "security assertion failed: {test}; {resource} cleanup also failed: {cleanup}"
        )
        .into()),
    }
}

fn require_failure_code(name: &str, output: &Output, code: &str) -> TestResult {
    if output.status.success() {
        return Err(format!("{name} unexpectedly succeeded").into());
    }
    let observed = structured_error_code(&output.stdout).map_err(|error| {
        format!(
            "{name} lacked a valid structured error: {error}; stderr={}",
            String::from_utf8_lossy(&output.stderr)
        )
    })?;
    if observed != code {
        return Err(format!(
            "{name} returned structured error {observed}, expected {code}; stderr={}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(())
}

fn structured_error_code(stdout: &[u8]) -> TestResult<String> {
    let source = std::str::from_utf8(stdout)?;
    let mut codes = Vec::new();
    for line in source.lines().filter(|line| !line.trim().is_empty()) {
        let record: serde_json::Value = serde_json::from_str(line)?;
        match record.get("error") {
            None | Some(serde_json::Value::Null) => {}
            Some(error) => {
                let code = error
                    .get("code")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("structured error lacks a string code")?;
                codes.push(code.to_owned());
            }
        }
    }
    match codes.as_slice() {
        [code] => Ok(code.clone()),
        [] => Err("structured output lacks an error code".into()),
        _ => Err("structured output contains multiple error codes".into()),
    }
}

fn dns_domains() -> TestResult<Vec<String>> {
    let output = bounded_output(
        Command::new("container")
            .args(["system", "dns", "list", "--format", "json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
        Duration::from_secs(30),
    )?;
    if !output.status.success() {
        return Err(format!(
            "test-owned DNS inventory failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn owner_token() -> TestResult<String> {
    let mut bytes = [0_u8; 16];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn owned_domain(domain: &str) -> bool {
    domain
        .strip_prefix("gascan-")
        .and_then(|value| value.strip_suffix(".test"))
        .is_some_and(|token| {
            token.len() == 32
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

fn bounded_output(command: &mut Command, timeout: Duration) -> TestResult<Output> {
    let mut child = command.spawn()?;
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait()?.is_some() {
            return Ok(child.wait_with_output()?);
        }
        if Instant::now() >= deadline {
            child.kill()?;
            let output = child.wait_with_output()?;
            return Err(format!(
                "host command exceeded {timeout:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(test)]
mod security_regressions {
    use super::{
        TestResult, combine_test_and_cleanup, exact_published_port, structured_error_code,
    };
    use serde_json::json;

    #[test]
    fn published_port_requires_one_exact_loopback_tcp_mapping() -> TestResult {
        let exact = json!({
            "publishedPorts": [{
                "hostAddress": "127.0.0.1",
                "hostPort": 18080,
                "containerPort": 18080,
                "proto": "tcp",
                "count": 1
            }]
        });
        assert!(
            exact_published_port(
                exact.as_object().ok_or("exact fixture is not an object")?,
                18080
            )
            .is_ok()
        );

        for mutation in [
            json!({"publishedPorts": [], "note": "127.0.0.1:18080"}),
            json!({"publishedPorts": [{"hostAddress":"0.0.0.0","hostPort":18080,"containerPort":18080,"proto":"tcp","count":1}]}),
            json!({"publishedPorts": [{"hostAddress":"::","hostPort":18080,"containerPort":18080,"proto":"tcp","count":1}]}),
            json!({"publishedPorts": [{"hostAddress":"127.0.0.1","hostPort":18080,"containerPort":18081,"proto":"tcp","count":1}]}),
            json!({"publishedPorts": [{"hostAddress":"127.0.0.1","hostPort":18080,"containerPort":18080,"proto":"udp","count":1}]}),
            json!({"publishedPorts": [{"hostAddress":"127.0.0.1","hostPort":18080,"containerPort":18080,"proto":"tcp","count":2}]}),
            json!({"publishedPorts": [
                {"hostAddress":"127.0.0.1","hostPort":18080,"containerPort":18080,"proto":"tcp","count":1},
                {"hostAddress":"127.0.0.1","hostPort":18081,"containerPort":18081,"proto":"tcp","count":1}
            ]}),
        ] {
            assert!(
                exact_published_port(
                    mutation
                        .as_object()
                        .ok_or("mutation fixture is not an object")?,
                    18080
                )
                .is_err()
            );
        }
        Ok(())
    }

    #[test]
    fn structured_error_code_uses_only_exact_json_error_field() -> TestResult {
        let output = br#"{"phase":"create","error":null}
{"phase":"create","status":3,"error":{"code":"disk_control_unsupported","message":"denied"}}
"#;
        assert_eq!(structured_error_code(output)?, "disk_control_unsupported");
        assert!(structured_error_code(b"not json disk_control_unsupported\n").is_err());
        assert!(
            structured_error_code(
                br#"{"message":"disk_control_unsupported","error":{"code":"invalid_request"}}
"#
            )
            .is_ok_and(|code| code != "disk_control_unsupported")
        );
        assert!(
            structured_error_code(
                br#"{"error":{"code":"invalid_request"}}
{"error":{"code":"disk_control_unsupported"}}
"#
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn assertion_and_cleanup_failures_are_both_returned() -> TestResult {
        let test: TestResult = Err("synthetic assertion failure".into());
        let cleanup: TestResult = Err("synthetic cleanup failure".into());
        let Err(error) = combine_test_and_cleanup("synthetic route", test, cleanup) else {
            return Err("combined failure unexpectedly succeeded".into());
        };
        let message = error.to_string();
        assert!(message.contains("synthetic assertion failure"));
        assert!(message.contains("synthetic cleanup failure"));
        Ok(())
    }
}
