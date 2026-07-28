#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod apple_common;

use apple_common::{AppleE2e, TestResult, marker_payload};
use serde::de::{Error as _, MapAccess, Visitor};
use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};
use std::os::unix::fs::MetadataExt as _;
use std::process::{Command, Stdio};

const PERSISTENT_WORKSTATION_SENTINELS: [&str; 3] = [
    "/home/workspace/.local/image-replace-sentinel",
    "/home/workspace/.cache/image-replace-sentinel",
    "/home/workspace/.config/image-replace-sentinel",
];

const SHELL_PROMPT_PROBE: &str = r#"#!/usr/bin/env bash
set -u
printf 'GASCAN_PROMPT_IDENTITY_BEGIN\n'
selector=$(< /home/workspace/.config/gascan/shell/prompt)
printf 'SELECTOR=%s\n' "$selector"
printf 'CONFIG=%s\n' "${STARSHIP_CONFIG:-}"
case "$selector" in
    starship) preset=/opt/gascan/shell/presets/starship.toml ;;
    starship-nerd-font) preset=/opt/gascan/shell/presets/starship-nerd-font.toml ;;
    *) preset=invalid ;;
esac
if test "$preset" != invalid &&
    test -r "${STARSHIP_CONFIG:-}" &&
    cmp -s "$STARSHIP_CONFIG" "$preset"; then
    printf 'CONFIG_IDENTITY=matching\n'
else
    printf 'CONFIG_IDENTITY=mismatch\n'
fi
printf 'STARSHIP_EXECUTABLE=%s\n' "${STARSHIP_EXECUTABLE:-}"
printf 'STARSHIP_FUNCTION=%s\n' "$(type -t starship_precmd || true)"
printf 'GASCAN_PROMPT_IDENTITY_END\n'
"#;

#[test]
#[ignore = "requires supported Apple runtime, candidate and predecessor workspace images, network access, and OpenSSH"]
fn native_ssh_is_loopback_only_durable_reconciled_and_cleaned() -> TestResult {
    let predecessor = std::env::var("GASCAN_E2E_PREDECESSOR_IMAGE")
        .map_err(|_| "GASCAN_E2E_PREDECESSOR_IMAGE must name the compatible predecessor fixture")?;
    let approved = apple_common::approved_workspace_image()?;
    apple_common::validate_distinct_image_fixtures(&predecessor, &approved)?;

    let env = AppleE2e::new_networked("native-ssh")?;
    let default_network_probe = env.start_default_network_probe()?;
    let root = std::path::Path::new(env.root());
    env.success_with_timeout(
        ["up", root.to_str().ok_or("non-UTF-8 root")?],
        std::time::Duration::from_secs(10 * 60),
    )?;
    env.assert_managed_network_attachment()?;
    let (ssh_host, ssh_port) = env.native_ssh_endpoint()?;
    if ssh_host != "127.0.0.1" || ssh_port < 1024 {
        return Err(format!("unexpected native SSH endpoint: {ssh_host}:{ssh_port}").into());
    }
    let before = ssh_status(
        &env.status_json()
            .map_err(|error| format!("initial SSH status failed: {error}"))?,
    )?;

    let argument = "gascan-native-ssh-exact-argument";
    let remote = env.success(["--sandbox", env.id(), "ssh", "--", "printf", "%s", argument])?;
    if remote.stdout != argument.as_bytes() {
        return Err(format!(
            "native SSH remote argument changed: {}",
            String::from_utf8_lossy(&remote.stdout)
        )
        .into());
    }
    env.assert_default_network_cannot_reach_native_ssh(&default_network_probe)?;

    let alias = before.alias.as_str();
    let config = env.account_home().join(".config/gascan/ssh/config");
    let guest_port = 38_181_u16;
    let guest_server = OwnedChild::spawn(
        env.command([
            "--sandbox",
            env.id(),
            "run",
            "--",
            "python3",
            "-m",
            "http.server",
            &guest_port.to_string(),
            "--bind",
            "127.0.0.1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped()),
    )?;
    let forward_port = reserve_loopback_port()?;
    let mut forward = Command::new("/usr/bin/ssh");
    forward
        .args(["-F"])
        .arg(&config)
        .args(["-N", "-o", "ExitOnForwardFailure=yes", "-L"])
        .arg(format!("127.0.0.1:{forward_port}:127.0.0.1:{guest_port}"))
        .arg(alias)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let forward = OwnedChild::spawn(&mut forward)?;
    let body = http_get_with_retry(forward_port, std::time::Duration::from_secs(10))
        .map_err(|error| format!("VS Code-style local forwarding failed: {error}"))?;
    if !body.contains("Directory listing") {
        return Err("VS Code-style local forwarding did not reach guest loopback".into());
    }

    let remote_port = reserve_loopback_port()?;
    let mut remote_forward = Command::new("/usr/bin/ssh");
    remote_forward
        .args(["-F"])
        .arg(&config)
        .args(["-o", "ExitOnForwardFailure=yes", "-R"])
        .arg(format!("127.0.0.1:{remote_port}:127.0.0.1:{guest_port}"))
        .arg(alias)
        .arg("/usr/bin/true")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let rejected =
        apple_common::run_command_bounded(remote_forward, std::time::Duration::from_secs(10))
            .map_err(|error| format!("remote-forward rejection check failed: {error}"))?;
    if rejected.status.success() {
        return Err("OpenSSH remote forwarding unexpectedly succeeded".into());
    }

    let agent_socket = env.runtime_root().join("ssh-agent.sock");
    let mut agent_process = Command::new("/usr/bin/ssh-agent");
    agent_process
        .args(["-D", "-a"])
        .arg(&agent_socket)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let agent_process = OwnedChild::spawn(&mut agent_process)?;
    let agent_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::os::unix::net::UnixStream::connect(&agent_socket).is_err() {
        if std::time::Instant::now() >= agent_deadline {
            return Err("local SSH agent did not become ready".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let mut agent_forward = Command::new("/usr/bin/ssh");
    agent_forward
        .args(["-A", "-F"])
        .arg(&config)
        .arg(alias)
        .arg("test -z \"${SSH_AUTH_SOCK:-}\"")
        .env("SSH_AUTH_SOCK", &agent_socket)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let agent =
        apple_common::run_command_bounded(agent_forward, std::time::Duration::from_secs(10))
            .map_err(|error| format!("agent-forwarding rejection check failed: {error}"))?;
    if !agent.status.success() {
        return Err(format!(
            "server exposed or failed to reject agent forwarding: {}",
            String::from_utf8_lossy(&agent.stderr)
        )
        .into());
    }
    drop(agent_process);

    drop(forward);
    drop(guest_server);
    env.success(["--sandbox", env.id(), "down"])?;
    env.success_with_timeout(
        ["up", root.to_str().ok_or("non-UTF-8 root")?],
        std::time::Duration::from_secs(10 * 60),
    )?;
    let after_restart = ssh_status(
        &env.status_json()
            .map_err(|error| format!("post-restart SSH status failed: {error}"))?,
    )?;
    before.assert_same_fingerprints(&after_restart)?;

    env.replace_owned_container_image(&predecessor, std::time::Duration::from_secs(10 * 60))?;
    env.seed_stored_image_resolution(&predecessor)?;
    env.success_with_timeout(
        [
            "--sandbox",
            env.id(),
            "apply",
            root.to_str().ok_or("non-UTF-8 root")?,
        ],
        std::time::Duration::from_secs(10 * 60),
    )?;
    let after_apply = ssh_status(
        &env.status_json()
            .map_err(|error| format!("post-image-apply SSH status failed: {error}"))?,
    )?;
    before.assert_same_fingerprints(&after_apply)?;

    env.kill_daemon()?;
    let (removed_config, removed_generation) = remove_active_ssh_publication(&env)?;
    let after_daemon_restart = ssh_status(
        &env.status_json()
            .map_err(|error| format!("post-daemon-restart SSH status failed: {error}"))?,
    )?;
    if !removed_config.is_file() || !removed_generation.is_file() {
        return Err("daemon restart did not reconstruct the removed SSH publication".into());
    }
    before.assert_same_fingerprints(&after_daemon_restart)?;
    let reconciled = env.invoke([
        "--sandbox",
        env.id(),
        "ssh",
        "--",
        "printf",
        "%s",
        "reconciled",
    ])?;
    if !reconciled.status.success() {
        let mut direct = Command::new("/usr/bin/ssh");
        direct
            .args(["-vv", "-F"])
            .arg(&config)
            .arg(alias)
            .args(["printf", "%s", "reconciled"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let direct = apple_common::run_command_bounded(direct, std::time::Duration::from_secs(10))?;
        return Err(format!(
            "daemon restart SSH failed: gascan_status={:?} gascan_stdout={} gascan_stderr={} \
             direct_status={:?} direct_stdout={} direct_stderr={}",
            reconciled.status.code(),
            String::from_utf8_lossy(&reconciled.stdout),
            String::from_utf8_lossy(&reconciled.stderr),
            direct.status.code(),
            String::from_utf8_lossy(&direct.stdout),
            String::from_utf8_lossy(&direct.stderr),
        )
        .into());
    }
    if reconciled.stdout != b"reconciled" {
        return Err("daemon restart did not reconstruct a working SSH alias".into());
    }

    let collision_listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let collision_port = collision_listener.local_addr()?.port();
    let collision = AppleE2e::new_networked("native-ssh-collision")?;
    collision.write_manifest(&format!(
        "version = 1\nname = 'native-ssh-collision'\nnetwork = 'networked'\n\
         [ssh]\nenabled = true\nhost_port = {collision_port}\n"
    ))?;
    let collision_output = collision.invoke_with_timeout(
        [
            "up",
            collision.root().to_str().ok_or("non-UTF-8 root")?,
            "--json",
        ],
        std::time::Duration::from_secs(2 * 60),
    )?;
    if collision_output.status.success()
        || !String::from_utf8_lossy(&collision_output.stdout).contains("ssh_port_unavailable")
    {
        return Err(format!(
            "explicit SSH collision was not actionable: stdout={} stderr={}",
            String::from_utf8_lossy(&collision_output.stdout),
            String::from_utf8_lossy(&collision_output.stderr)
        )
        .into());
    }
    collision.assert_no_owned_resources()?;
    drop(collision_listener);

    env.success(["--sandbox", env.id(), "destroy", "--yes"])?;
    env.assert_no_owned_resources()?;
    assert_destroyed_ssh_state(&env, alias)?;

    let offline = AppleE2e::new("native-ssh-offline")?;
    offline.success_with_timeout(
        ["up", offline.root().to_str().ok_or("non-UTF-8 root")?],
        std::time::Duration::from_secs(10 * 60),
    )?;
    offline.assert_no_native_ssh_port()?;
    offline.success(["--sandbox", offline.id(), "destroy", "--yes"])?;
    offline.assert_no_owned_resources()?;
    std::fs::write(
        std::path::Path::new(offline.root()).join("gascan.toml"),
        "version = 1\nname = 'native-ssh-offline'\nnetwork = 'offline'\n\
         [ssh]\nenabled = true\n",
    )?;
    let invalid = offline.invoke(["up", offline.root().to_str().ok_or("non-UTF-8 root")?])?;
    if invalid.status.success()
        || !String::from_utf8_lossy(&invalid.stderr).contains("ssh requires network")
    {
        return Err("explicit offline SSH was not rejected with actionable validation".into());
    }

    if std::net::TcpStream::connect(("127.0.0.1", ssh_port)).is_ok() {
        return Err("destroy retained the native SSH listener".into());
    }
    Ok(())
}

#[test]
#[ignore = "requires supported Apple runtime, the locked workspace image, and OpenSSH"]
fn managed_shell_prompts_match_ssh_and_activate_offline() -> TestResult {
    let networked = AppleE2e::new_networked("shell-prompt-parity")?;
    install_shell_prompt_probe(&networked)?;
    networked.write_manifest(
        "version = 1\nname = 'shell-prompt-parity'\nnetwork = 'networked'\n\
         [shell]\nprompt = 'starship'\n\
         [ssh]\nenabled = true\n",
    )?;
    networked.success_with_timeout(
        ["up", networked.root().to_str().ok_or("non-UTF-8 root")?],
        std::time::Duration::from_secs(10 * 60),
    )?;
    assert_shell_and_ssh_prompt_identity(&networked, "starship")?;

    networked.write_manifest(
        "version = 1\nname = 'shell-prompt-parity'\nnetwork = 'networked'\n\
         [shell]\nprompt = 'starship-nerd-font'\n\
         [ssh]\nenabled = true\n",
    )?;
    networked.success_with_timeout(
        [
            "--sandbox",
            networked.id(),
            "apply",
            networked.root().to_str().ok_or("non-UTF-8 root")?,
        ],
        std::time::Duration::from_secs(10 * 60),
    )?;
    assert_shell_and_ssh_prompt_identity(&networked, "starship-nerd-font")?;
    networked.success(["--sandbox", networked.id(), "destroy", "--yes"])?;
    networked.assert_no_owned_resources()?;

    let offline = AppleE2e::new("shell-prompt-offline")?;
    install_shell_prompt_probe(&offline)?;
    offline.write_manifest(
        "version = 1\nname = 'shell-prompt-offline'\nnetwork = 'offline'\n\
         [shell]\nprompt = 'starship'\n",
    )?;
    offline.success_with_timeout(
        ["up", offline.root().to_str().ok_or("non-UTF-8 root")?],
        std::time::Duration::from_secs(10 * 60),
    )?;
    offline.assert_no_network_attachments()?;
    let identity = default_shell_prompt_identity(&offline)?;
    assert_eq!(identity, expected_prompt_identity("starship"));
    offline.success(["--sandbox", offline.id(), "destroy", "--yes"])?;
    offline.assert_no_owned_resources()
}

fn install_shell_prompt_probe(env: &AppleE2e) -> TestResult {
    let root = std::path::Path::new(env.root());
    std::fs::create_dir(root.join(".gascan"))?;
    std::fs::write(
        root.join(".gascan/shell-prompt-probe.sh"),
        SHELL_PROMPT_PROBE,
    )?;
    Ok(())
}

fn assert_shell_and_ssh_prompt_identity(env: &AppleE2e, selector: &str) -> TestResult {
    let shell = default_shell_prompt_identity(env)?;
    let ssh = env.success_with_timeout(
        [
            "--sandbox",
            env.id(),
            "ssh",
            "--",
            "/bin/bash",
            "--login",
            "-i",
            "/workspace/.gascan/shell-prompt-probe.sh",
        ],
        std::time::Duration::from_secs(90),
    )?;
    let ssh = marker_payload(
        &ssh.stdout,
        "GASCAN_PROMPT_IDENTITY_BEGIN",
        "GASCAN_PROMPT_IDENTITY_END",
    )?;
    let expected = expected_prompt_identity(selector);
    if shell != expected || ssh != expected {
        return Err(format!(
            "managed prompt identity differs: expected={expected:?} shell={shell:?} ssh={ssh:?}"
        )
        .into());
    }
    Ok(())
}

fn default_shell_prompt_identity(env: &AppleE2e) -> TestResult<String> {
    let output = env.run_default_shell_pty_script(
        ". /workspace/.gascan/shell-prompt-probe.sh\nexit 0\n",
        b"GASCAN_PROMPT_IDENTITY_END",
        "gascan-apple-e2e-term",
    )?;
    if !output.status.success() {
        return Err(format!(
            "default shell prompt probe failed with {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout)
        )
        .into());
    }
    marker_payload(
        &output.stdout,
        "GASCAN_PROMPT_IDENTITY_BEGIN",
        "GASCAN_PROMPT_IDENTITY_END",
    )
}

fn expected_prompt_identity(selector: &str) -> String {
    format!(
        "SELECTOR={selector}\n\
         CONFIG=/home/workspace/.config/gascan/shell/starship.toml\n\
         CONFIG_IDENTITY=matching\n\
         STARSHIP_EXECUTABLE=/opt/gascan/shell/bin/starship\n\
         STARSHIP_FUNCTION=function\n"
    )
}

#[derive(Clone)]
struct LiveSshStatus {
    alias: String,
    host_fingerprint: String,
    client_fingerprint: String,
}

impl LiveSshStatus {
    fn assert_same_fingerprints(&self, other: &Self) -> TestResult {
        if self.host_fingerprint != other.host_fingerprint
            || self.client_fingerprint != other.client_fingerprint
        {
            return Err("SSH fingerprints changed across a durable lifecycle transition".into());
        }
        Ok(())
    }
}

fn ssh_status(status: &serde_json::Value) -> TestResult<LiveSshStatus> {
    let ssh = status["ssh"]
        .as_object()
        .ok_or("status omitted structured SSH state")?;
    if ssh.get("state").and_then(serde_json::Value::as_str) != Some("ready")
        || ssh.get("host").and_then(serde_json::Value::as_str) != Some("127.0.0.1")
    {
        return Err(format!("SSH is not ready on IPv4 loopback: {ssh:?}").into());
    }
    Ok(LiveSshStatus {
        alias: ssh
            .get("alias")
            .and_then(serde_json::Value::as_str)
            .ok_or("SSH status omitted alias")?
            .to_owned(),
        host_fingerprint: ssh
            .get("host_key_fingerprint")
            .and_then(serde_json::Value::as_str)
            .ok_or("SSH status omitted host fingerprint")?
            .to_owned(),
        client_fingerprint: ssh
            .get("client_key_fingerprint")
            .and_then(serde_json::Value::as_str)
            .ok_or("SSH status omitted client fingerprint")?
            .to_owned(),
    })
}

fn reserve_loopback_port() -> TestResult<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

fn http_get_with_retry(port: u16, timeout: std::time::Duration) -> TestResult<String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let attempt = (|| -> std::io::Result<String> {
            let mut stream = std::net::TcpStream::connect(("127.0.0.1", port))?;
            stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
            stream.write_all(b"GET / HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n")?;
            let mut response = String::new();
            stream.read_to_string(&mut response)?;
            Ok(response)
        })();
        match attempt {
            Ok(response) => return Ok(response),
            Err(error) if std::time::Instant::now() < deadline => {
                let _ = error;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

struct OwnedChild(Option<std::process::Child>);

impl OwnedChild {
    fn spawn(command: &mut Command) -> TestResult<Self> {
        Ok(Self(Some(command.spawn()?)))
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn assert_destroyed_ssh_state(env: &AppleE2e, alias: &str) -> TestResult {
    let directory = env.account_home().join(".config/gascan/ssh");
    if !directory.join("identity_ed25519").is_file() {
        return Err("destroy removed the install-wide SSH client identity".into());
    }
    let config_path = directory.join("config");
    let config = std::fs::read_to_string(&config_path)?;
    if config.contains(alias) {
        return Err("destroy retained the sandbox alias in active SSH config".into());
    }
    for generation in active_known_hosts_paths(&directory, &config)? {
        let contents = std::fs::read(&generation)?;
        if contents
            .windows(alias.len())
            .any(|window| window == alias.as_bytes())
        {
            return Err(format!(
                "destroy retained active sandbox SSH trust in {}",
                generation.display()
            )
            .into());
        }
    }
    Ok(())
}

fn remove_active_ssh_publication(
    env: &AppleE2e,
) -> TestResult<(std::path::PathBuf, std::path::PathBuf)> {
    let directory = env.account_home().join(".config/gascan/ssh");
    let config = directory.join("config");
    validate_owned_publication_file(&config)?;
    let config_contents = std::fs::read_to_string(&config)?;
    let generations = active_known_hosts_paths(&directory, &config_contents)?;
    let [generation] = generations.as_slice() else {
        return Err("active SSH publication did not reference exactly one trust generation".into());
    };
    validate_owned_publication_file(generation)?;
    let generation = generation.clone();
    std::fs::remove_file(&config)?;
    std::fs::remove_file(&generation)?;
    Ok((config, generation))
}

fn active_known_hosts_paths(
    directory: &std::path::Path,
    config: &str,
) -> TestResult<Vec<std::path::PathBuf>> {
    let mut paths = config
        .lines()
        .filter_map(|line| line.strip_prefix("    UserKnownHostsFile "))
        .map(std::path::PathBuf::from)
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    for path in &paths {
        let name = path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .ok_or("managed known-hosts generation name is invalid")?;
        let digest = name
            .strip_prefix("known_hosts.")
            .ok_or("managed known-hosts generation prefix is invalid")?;
        if path.parent() != Some(directory)
            || digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err("managed known-hosts generation path is unsafe".into());
        }
    }
    Ok(paths)
}

fn validate_owned_publication_file(path: &std::path::Path) -> TestResult {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o644
        || metadata.nlink() != 1
    {
        return Err(format!(
            "refusing unsafe managed SSH publication: {}",
            path.display()
        )
        .into());
    }
    Ok(())
}

#[test]
#[ignore = "requires supported Apple runtime, two compatible digest-qualified workspace images, and network access"]
fn image_replace_preserves_durable_resources_and_rolls_back_failure() -> TestResult {
    let predecessor = std::env::var("GASCAN_E2E_PREDECESSOR_IMAGE")
        .map_err(|_| "GASCAN_E2E_PREDECESSOR_IMAGE must name the compatible predecessor fixture")?;
    let approved = apple_common::approved_workspace_image()?;
    apple_common::validate_distinct_image_fixtures(&predecessor, &approved)?;

    let env = AppleE2e::new_networked("image-replace")?;
    let root = std::path::Path::new(env.root());
    std::fs::create_dir(root.join(".gascan"))?;
    std::fs::write(
        root.join("gascan.toml"),
        "version = 1\nname = 'image-replace'\nnetwork = 'networked'\n\
         setup = './.gascan/setup.sh'\n",
    )?;
    std::fs::write(
        root.join(".gascan/setup.sh"),
        "#!/bin/sh\nset -eu\n\
         count=0\n\
         test ! -f /workspace/setup-count || read -r count </workspace/setup-count\n\
         count=$((count + 1))\n\
         printf '%s\\n' \"$count\" >/workspace/setup-count\n",
    )?;
    std::fs::write(
        root.join(".gascan/workstation-state-persistence.sh"),
        include_str!("../../../tests/image/workstation-state-persistence.sh"),
    )?;
    std::fs::set_permissions(
        root.join(".gascan/workstation-state-persistence.sh"),
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
    )?;
    std::fs::write(
        root.join(".gascan/gascan-pi-extension.js"),
        "export default function gascanPersistenceFixture() {}\n",
    )?;
    env.success_with_timeout(
        ["up", root.to_str().ok_or("non-UTF-8 root")?],
        std::time::Duration::from_secs(10 * 60),
    )?;
    assert_eq!(std::fs::read_to_string(root.join("setup-count"))?, "1\n");

    env.success_with_timeout(
        [
            "--sandbox",
            env.id(),
            "run",
            "--",
            "/workspace/.gascan/workstation-state-persistence.sh",
            "seed",
        ],
        std::time::Duration::from_secs(2 * 60),
    )?;
    for path in PERSISTENT_WORKSTATION_SENTINELS {
        env.success([
            "--sandbox",
            env.id(),
            "run",
            "--",
            "sh",
            "-c",
            &format!("printf durable >{}", shell_quote(path)),
        ])?;
    }
    env.success(["--sandbox", env.id(), "down"])?;
    std::thread::sleep(std::time::Duration::from_secs(6));
    env.success(["up", root.to_str().ok_or("non-UTF-8 root")?])?;
    env.assert_owned_container_running()?;
    env.success(["--sandbox", env.id(), "run", "--", "true"])?;
    assert_compatible_fixture(&env, true)?;

    env.replace_owned_container_image(&predecessor, std::time::Duration::from_secs(10 * 60))?;
    env.seed_stored_image_resolution(&predecessor)?;
    assert_compatible_fixture(&env, false)?;
    let predecessor_snapshot = env.owned_runtime_snapshot()?;
    assert!(gascan_core::runtime::same_immutable_image(
        predecessor_snapshot.container_image(),
        &predecessor
    ));
    env.write_image_replace_root_sentinel()?;
    env.assert_image_replace_root_sentinel(true)?;

    let status = env.status_json()?;
    assert_image_changed(&status, &predecessor, &approved)?;

    let up = env.success(["up", root.to_str().ok_or("non-UTF-8 root")?, "--json"])?;
    assert_json_phase(&up.stdout, "apply_required")?;
    assert_eq!(env.owned_runtime_snapshot()?, predecessor_snapshot);
    assert_eq!(std::fs::read_to_string(root.join("setup-count"))?, "1\n");
    env.assert_image_replace_root_sentinel(true)?;

    let apply = env.success_with_timeout(
        [
            "--sandbox",
            env.id(),
            "apply",
            root.to_str().ok_or("non-UTF-8 root")?,
            "--json",
        ],
        std::time::Duration::from_secs(10 * 60),
    )?;
    for phase in [
        "before_provision",
        "after_provision",
        "before_health",
        "after_health",
        "image_replaced",
    ] {
        assert_json_phase(&apply.stdout, phase)?;
    }
    assert_eq!(std::fs::read_to_string(root.join("setup-count"))?, "2\n");
    let approved_snapshot = env.owned_runtime_snapshot()?;
    assert!(
        gascan_core::runtime::same_immutable_image(approved_snapshot.container_image(), &approved),
        "approved snapshot image mismatch: observed={} expected={approved}",
        approved_snapshot.container_image()
    );
    predecessor_snapshot.assert_retained_identities_equal(&approved_snapshot)?;
    assert_compatible_fixture(&env, true)?;
    env.assert_image_replace_root_sentinel(false)?;

    env.replace_owned_container_image(&predecessor, std::time::Duration::from_secs(10 * 60))?;
    env.seed_stored_image_resolution(&predecessor)?;
    env.write_image_replace_root_sentinel()?;
    std::fs::write(
        root.join(".gascan/setup.sh"),
        "#!/bin/sh\nset -eu\n\
         printf attempted >/workspace/setup-failure-ran\n\
         exit 42\n",
    )?;
    let failed = env.invoke_with_timeout(
        [
            "--sandbox",
            env.id(),
            "apply",
            root.to_str().ok_or("non-UTF-8 root")?,
            "--json",
        ],
        std::time::Duration::from_secs(10 * 60),
    )?;
    env.assert_exit_code(&failed, 70)?;
    assert_json_phase(&failed.stdout, "image_rollback")?;
    assert_json_error(&failed.stdout)?;
    assert_eq!(
        std::fs::read_to_string(root.join("setup-failure-ran"))?,
        "attempted"
    );
    let rolled_back = env.owned_runtime_snapshot()?;
    assert!(gascan_core::runtime::same_immutable_image(
        rolled_back.container_image(),
        &predecessor
    ));
    predecessor_snapshot.assert_retained_identities_equal(&rolled_back)?;
    assert_compatible_fixture(&env, false)?;
    env.assert_image_replace_root_sentinel(false)?;

    env.success(["--sandbox", env.id(), "destroy", "--yes"])?;
    env.assert_no_owned_resources()
}

fn assert_compatible_fixture(env: &AppleE2e, commands_available: bool) -> TestResult {
    let probes = PERSISTENT_WORKSTATION_SENTINELS
        .iter()
        .map(|path| format!("test \"$(cat {})\" = durable", shell_quote(path)))
        .collect::<Vec<_>>()
        .join("; ");
    let persistence_mode = if commands_available { "probe" } else { "files" };
    let output = env.success([
        "--sandbox",
        env.id(),
        "run",
        "--",
        "sh",
        "-c",
        &format!(
            "set -eu; test \"$(id -un)\" = workspace; {probes}; \
             /workspace/.gascan/workstation-state-persistence.sh {persistence_mode} >/dev/null"
        ),
    ])?;
    if output.stdout.is_empty() {
        Ok(())
    } else {
        Err("fixture compatibility probe produced unexpected output".into())
    }
}

#[test]
fn explicit_sentinels_target_the_three_managed_volume_roots() {
    assert_eq!(
        PERSISTENT_WORKSTATION_SENTINELS,
        [
            "/home/workspace/.local/image-replace-sentinel",
            "/home/workspace/.cache/image-replace-sentinel",
            "/home/workspace/.config/image-replace-sentinel",
        ]
    );
}

fn assert_image_changed(status: &serde_json::Value, current: &str, requested: &str) -> TestResult {
    let requirements = status["apply_requirements"]
        .as_array()
        .ok_or("status apply_requirements must be an array")?;
    let exact = requirements
        .iter()
        .filter(|requirement| requirement["reason"] == "image_changed")
        .collect::<Vec<_>>();
    let [requirement] = exact.as_slice() else {
        return Err(format!("expected one image_changed requirement: {requirements:?}").into());
    };
    let observed_current = requirement["current"]
        .as_str()
        .ok_or("image_changed current reference must be a string")?;
    let observed_requested = requirement["requested"]
        .as_str()
        .ok_or("image_changed requested reference must be a string")?;
    if !gascan_core::runtime::same_immutable_image(observed_current, current)
        || !gascan_core::runtime::same_immutable_image(observed_requested, requested)
    {
        return Err(format!("unexpected image replacement requirement: {requirement:?}").into());
    }
    Ok(())
}

fn assert_json_phase(output: &[u8], expected: &str) -> TestResult {
    let found = std::str::from_utf8(output)?
        .lines()
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .any(|event| event["phase"] == expected);
    if found {
        Ok(())
    } else {
        Err(format!("operation stream omitted phase {expected}").into())
    }
}

fn assert_json_error(output: &[u8]) -> TestResult {
    let found = std::str::from_utf8(output)?
        .lines()
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .any(|event| event["error"].is_object());
    if found {
        Ok(())
    } else {
        Err("failed replacement stream omitted its primary error".into())
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[test]
#[ignore = "requires supported Apple runtime and the locked workspace image"]
fn workstation_defaults_are_exact_credential_free_and_offline() -> TestResult {
    let env = AppleE2e::new("workstation-offline")?;
    let root = std::path::Path::new(env.root());
    std::fs::create_dir(root.join(".gascan"))?;
    let persistence = root.join(".gascan/workstation-state-persistence.sh");
    std::fs::write(
        &persistence,
        include_str!("../../../tests/image/workstation-state-persistence.sh"),
    )?;
    std::fs::set_permissions(
        &persistence,
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
    )?;
    std::fs::write(
        root.join(".gascan/gascan-pi-extension.js"),
        "export default function gascanPersistenceFixture() {}\n",
    )?;
    env.success_with_timeout(
        ["up", root.to_str().ok_or("non-UTF-8 root")?],
        std::time::Duration::from_secs(10 * 60),
    )?;
    env.assert_no_network_attachments()?;
    let contract = env.success_with_timeout(
        [
            "--sandbox",
            env.id(),
            "run",
            "--",
            "/opt/gascan/tests/workstation-contract.sh",
        ],
        std::time::Duration::from_secs(5 * 60),
    )?;
    if contract.stdout != b"workstation-contract-ok\n" {
        return Err(format!(
            "unexpected workstation contract output: {}",
            String::from_utf8_lossy(&contract.stdout)
        )
        .into());
    }
    env.success_with_timeout(
        [
            "--sandbox",
            env.id(),
            "run",
            "--",
            "/workspace/.gascan/workstation-state-persistence.sh",
            "seed",
        ],
        std::time::Duration::from_secs(2 * 60),
    )?;
    env.success(["--sandbox", env.id(), "down"])?;
    env.success_with_timeout(
        ["up", root.to_str().ok_or("non-UTF-8 root")?],
        std::time::Duration::from_secs(10 * 60),
    )?;
    env.assert_no_network_attachments()?;
    env.success_with_timeout(
        [
            "--sandbox",
            env.id(),
            "run",
            "--",
            "/workspace/.gascan/workstation-state-persistence.sh",
            "probe",
        ],
        std::time::Duration::from_secs(2 * 60),
    )?;
    env.success(["--sandbox", env.id(), "destroy", "--yes"])?;
    env.assert_no_owned_resources()
}

#[test]
#[ignore = "requires supported Apple runtime, locked workspace image, and network access"]
fn workstation_tools_override_wins_without_mutating_immutable_defaults() -> TestResult {
    const OVERRIDE: &str = "1.26.4";
    let env = AppleE2e::new_networked("workstation-override")?;
    let root = std::path::Path::new(env.root());
    let snapshot = root.join(".gascan/immutable-tree-snapshot.sh");
    std::fs::create_dir_all(
        snapshot
            .parent()
            .ok_or("snapshot helper parent is absent")?,
    )?;
    std::fs::write(
        &snapshot,
        include_str!("../../../tests/image/immutable-tree-snapshot.sh"),
    )?;
    std::fs::set_permissions(
        &snapshot,
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
    )?;
    env.success_with_timeout(
        ["up", root.to_str().ok_or("non-UTF-8 root")?],
        std::time::Duration::from_secs(10 * 60),
    )?;
    let before = env.success([
        "--sandbox",
        env.id(),
        "run",
        "--",
        "/workspace/.gascan/immutable-tree-snapshot.sh",
        "/opt/gascan",
    ])?;

    env.write_manifest(&format!(
        "version = 1\nname = 'workstation-override'\nnetwork = 'networked'\n\
         [tools]\ngo = '{OVERRIDE}'\n"
    ))?;
    env.success_with_timeout(
        [
            "--sandbox",
            env.id(),
            "apply",
            root.to_str().ok_or("non-UTF-8 root")?,
        ],
        std::time::Duration::from_secs(20 * 60),
    )?;
    let proof = env.success([
        "--sandbox",
        env.id(),
        "run",
        "--",
        "sh",
        "-c",
        "set -eu; test \"$(command -v go)\" = /home/workspace/.local/share/mise/shims/go; go version; \
         /workspace/.gascan/immutable-tree-snapshot.sh /opt/gascan",
    ])?;
    let text = std::str::from_utf8(&proof.stdout)?;
    let mut lines = text.lines();
    if lines.next() != Some(&format!("go version go{OVERRIDE} linux/arm64")) {
        return Err(format!("mise override did not win exactly: {text}").into());
    }
    let after = lines
        .next()
        .ok_or("immutable /opt/gascan digest is absent")?;
    if lines.next().is_some() || before.stdout != format!("{after}\n").as_bytes() {
        return Err("immutable /opt/gascan content changed during override".into());
    }

    env.success(["--sandbox", env.id(), "destroy", "--yes"])?;
    env.assert_no_owned_resources()
}

#[test]
#[ignore = "requires supported Apple runtime, locked workspace image, and network access"]
fn apply_installs_large_npm_tool_and_neovim_with_storage_override() -> TestResult {
    let env = AppleE2e::new_networked("storage-tools")?;
    let root = std::path::Path::new(env.root());
    env.write_manifest(
        "version = 1\nname = 'storage-tools'\nnetwork = 'networked'\n\
         [storage]\ntools = '11GiB'\ncache = '12GiB'\nconfig = '2GiB'\n",
    )?;
    env.success_with_timeout(
        ["up", root.to_str().ok_or("non-UTF-8 root")?],
        std::time::Duration::from_secs(10 * 60),
    )?;

    env.write_manifest(
        "version = 1\nname = 'storage-tools'\nnetwork = 'networked'\n\
         [storage]\ntools = '11GiB'\ncache = '12GiB'\nconfig = '2GiB'\n\
         [tools]\nnode = '24.18.0'\n\"npm:@openai/codex\" = '0.10.0'\nneovim = '0.11.3'\n",
    )?;
    env.success_with_timeout(
        [
            "--sandbox",
            env.id(),
            "apply",
            root.to_str().ok_or("non-UTF-8 root")?,
        ],
        std::time::Duration::from_secs(20 * 60),
    )?;

    let inventory = env.success([
        "--sandbox",
        env.id(),
        "run",
        "--",
        "mise",
        "ls",
        "--current",
        "--installed",
        "--json",
    ])?;
    assert_exact_active_tools(&inventory.stdout, EXPECTED_APPLIED_TOOLS)?;

    env.success(["--sandbox", env.id(), "destroy", "--yes"])?;
    env.assert_no_owned_resources()
}

#[test]
#[ignore = "requires supported Apple runtime and the locked workspace image"]
fn changed_setup_is_reported_but_not_run_by_up_or_shell() -> TestResult {
    let env = AppleE2e::new("gate4-apply-setup")?;
    let root = std::path::Path::new(env.root());
    std::fs::create_dir(root.join(".gascan"))?;
    std::fs::write(
        root.join("gascan.toml"),
        "version = 1\nname = 'gate4-apply-setup'\nsetup = './.gascan/setup.sh'\n",
    )?;
    std::fs::write(
        root.join(".gascan/setup.sh"),
        "printf first > /workspace/result\n",
    )?;

    env.success(["up", root.to_str().ok_or("non-UTF-8 root")?])?;
    assert_eq!(std::fs::read_to_string(root.join("result"))?, "first");

    std::fs::write(
        root.join(".gascan/setup.sh"),
        "printf second > /workspace/result\n",
    )?;
    let up = env.success(["up", root.to_str().ok_or("non-UTF-8 root")?, "--json"])?;
    assert!(
        String::from_utf8_lossy(&up.stdout).contains("apply_required"),
        "changed setup was not reported: {}",
        String::from_utf8_lossy(&up.stdout)
    );
    env.success(["--sandbox", env.id(), "shell", "--", "true"])?;
    assert_eq!(std::fs::read_to_string(root.join("result"))?, "first");

    env.success([
        "--sandbox",
        env.id(),
        "apply",
        root.to_str().ok_or("non-UTF-8 root")?,
    ])?;
    assert_eq!(std::fs::read_to_string(root.join("result"))?, "second");
    env.success(["--sandbox", env.id(), "destroy", "--yes"])?;
    env.assert_no_owned_resources()
}

#[derive(serde::Deserialize)]
struct MiseToolRecord {
    version: String,
    installed: bool,
    active: bool,
}

struct MiseInventory(BTreeMap<String, Vec<MiseToolRecord>>);

impl<'de> serde::Deserialize<'de> for MiseInventory {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct InventoryVisitor;

        impl<'de> Visitor<'de> for InventoryVisitor {
            type Value = MiseInventory;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a mise tool inventory object with unique tool keys")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut records = BTreeMap::new();
                while let Some((tool, versions)) =
                    map.next_entry::<String, Vec<MiseToolRecord>>()?
                {
                    if records.insert(tool, versions).is_some() {
                        return Err(A::Error::custom("duplicate mise tool key"));
                    }
                }
                Ok(MiseInventory(records))
            }
        }

        deserializer.deserialize_map(InventoryVisitor)
    }
}

fn assert_exact_active_tools<const N: usize>(
    output: &[u8],
    expected: [(&str, &str); N],
) -> TestResult {
    let MiseInventory(records) = serde_json::from_slice(output)?;
    let expected =
        BTreeMap::from(expected.map(|(tool, version)| (tool.to_owned(), version.to_owned())));
    if !records.keys().eq(expected.keys()) {
        return Err(format!(
            "unexpected active tool set: {:?}",
            records.keys().collect::<Vec<_>>()
        )
        .into());
    }
    for (tool, expected_version) in expected {
        let entries = &records[&tool];
        let [entry] = entries.as_slice() else {
            return Err(format!("mise returned multiple records for {tool}").into());
        };
        if !entry.installed || !entry.active || entry.version != expected_version {
            return Err(format!(
                "mise returned an inactive or unexpected version for {tool}: {}",
                entry.version
            )
            .into());
        }
    }
    Ok(())
}

const EXPECTED_TOOLS: [(&str, &str); 3] = [
    ("neovim", "0.11.3"),
    ("node", "24.18.0"),
    ("npm:@openai/codex", "0.10.0"),
];

const EXPECTED_APPLIED_TOOLS: [(&str, &str); 10] = [
    ("elixir", "1.20.2-otp-29"),
    ("erlang", "29.0.3"),
    ("go", "1.26.5"),
    ("java", "25.0.2"),
    ("neovim", "0.11.3"),
    ("node", "24.18.0"),
    ("npm:@openai/codex", "0.10.0"),
    ("python", "3.14.6"),
    ("ruby", "3.4.10"),
    ("rust", "1.97.0"),
];

#[test]
fn exact_active_tools_accepts_exact_minimal_inventory() {
    let exact = br#"{
        "neovim":[{"installed":true,"active":true,"version":"0.11.3"}],
        "node":[{"installed":true,"active":true,"version":"24.18.0"}],
        "npm:@openai/codex":[{"installed":true,"active":true,"version":"0.10.0"}]
    }"#;
    assert!(assert_exact_active_tools(exact, EXPECTED_TOOLS).is_ok());
}

#[test]
fn exact_active_tools_rejects_tool_set_flags_and_version_mismatches() {
    let extra = br#"{
        "go":[{"installed":true,"active":true,"version":"1.26.5"}],
        "neovim":[{"installed":true,"active":true,"version":"0.11.3"}],
        "node":[{"installed":true,"active":true,"version":"24.18.0"}],
        "npm:@openai/codex":[{"installed":true,"active":true,"version":"0.10.0"}]
    }"#;
    assert!(assert_exact_active_tools(extra, EXPECTED_TOOLS).is_err());

    let inactive = br#"{
        "neovim":[{"installed":true,"active":false,"version":"0.11.3"}],
        "node":[{"installed":true,"active":true,"version":"24.18.0"}],
        "npm:@openai/codex":[{"installed":true,"active":true,"version":"0.10.0"}]
    }"#;
    assert!(assert_exact_active_tools(inactive, EXPECTED_TOOLS).is_err());

    let wrong_version = br#"{
        "neovim":[{"installed":true,"active":true,"version":"0.11.4"}],
        "node":[{"installed":true,"active":true,"version":"24.18.0"}],
        "npm:@openai/codex":[{"installed":true,"active":true,"version":"0.10.0"}]
    }"#;
    assert!(assert_exact_active_tools(wrong_version, EXPECTED_TOOLS).is_err());
}

#[test]
fn exact_active_tools_accepts_realistic_mise_metadata() {
    let realistic_metadata = br#"{
        "neovim":[{
            "installed":true,
            "active":true,
            "version":"0.11.3",
            "source":{"type":"global","path":"/home/workspace/.config/gascan/mise.toml"},
            "requested_version":"0.11.3",
            "install_path":"/home/workspace/.local/share/mise/installs/neovim/0.11.3",
            "symlinked_to":null
        }],
        "node":[{
            "installed":true,
            "active":true,
            "version":"24.18.0",
            "source":{"type":"global","path":"/home/workspace/.config/gascan/mise.toml"},
            "requested_version":"24.18.0",
            "install_path":"/opt/gascan/mise/installs/node/24.18.0",
            "symlinked_to":null
        }],
        "npm:@openai/codex":[{
            "installed":true,
            "active":true,
            "version":"0.10.0",
            "source":{"type":"global","path":"/home/workspace/.config/gascan/mise.toml"},
            "requested_version":"0.10.0",
            "install_path":"/home/workspace/.local/share/mise/installs/npm-openai-codex/0.10.0",
            "symlinked_to":null
        }]
    }"#;
    assert!(assert_exact_active_tools(realistic_metadata, EXPECTED_TOOLS).is_ok());
}

#[test]
fn exact_active_tools_rejects_duplicate_tools_and_multiple_records() {
    let duplicate_tool = br#"{
        "neovim":[{"installed":true,"active":true,"version":"0.11.3"}],
        "node":[{"installed":true,"active":true,"version":"24.18.0"}],
        "node":[{"installed":true,"active":true,"version":"24.18.0"}],
        "npm:@openai/codex":[{"installed":true,"active":true,"version":"0.10.0"}]
    }"#;
    assert!(assert_exact_active_tools(duplicate_tool, EXPECTED_TOOLS).is_err());

    let multiple_records = br#"{
        "neovim":[{"installed":true,"active":true,"version":"0.11.3"}],
        "node":[
            {"installed":true,"active":true,"version":"24.18.0"},
            {"installed":true,"active":true,"version":"24.18.0"}
        ],
        "npm:@openai/codex":[{"installed":true,"active":true,"version":"0.10.0"}]
    }"#;
    assert!(assert_exact_active_tools(multiple_records, EXPECTED_TOOLS).is_err());
}
