use std::{
    collections::BTreeSet, fs, os::unix::fs::PermissionsExt, os::unix::fs::symlink, path::Path,
    process::Command,
};

use sha2::{Digest, Sha256};

const MISE_LS_FILTER: &str = r#"if ((keys|sort) != ["elixir","erlang","go","java","node","python","ruby","rust"]) then error("unexpected mise tool set") else to_entries | map(if ((.value|type)!="array") or ((.value|length)!=1) or (.value[0].installed != true) or (.value[0].active != true) or ((.value[0].version|type)!="string") or (.value[0].version=="") then error("invalid mise ls record") else {key:.key,value:.value[0].version} end) | from_entries end"#;
const RUNTIME_PATH: &str = concat!(
    "/home/workspace/.local/bin:",
    "/home/workspace/.local/share/cargo/bin:",
    "/home/workspace/.local/share/go/bin:",
    "/home/workspace/.local/share/gem/bin:",
    "/home/workspace/.local/share/mise/shims:",
    "/opt/gascan/mise/shims:",
    "/usr/local/sbin:/usr/local/bin:",
    "/opt/gascan/workstation/bin:",
    "/usr/sbin:/usr/bin:/sbin:/bin"
);
const EXPECTED_SYSTEM_TOOLS: &str = "\
autoconf
bash-completion
bind9-dnsutils
bison
build-essential
ca-certificates
curl
emacs-nox
fd-find
file
fonts-liberation
fzf
gh
git
iproute2
iputils-ping
jq
less
libasound2t64
libatk-bridge2.0-0
libatk1.0-0
libcups2
libdbus-1-3
libdrm2
libffi-dev
libgbm1
libgdbm-dev
libglib2.0-0t64
libgtk-3-0t64
libncurses-dev
libnspr4
libnss3
libpango-1.0-0
libreadline-dev
libssl-dev
libx11-6
libxcb1
libxcomposite1
libxdamage1
libxext6
libxfixes3
libxkbcommon0
libxrandr2
libyaml-dev
lsof
nano
net-tools
netcat-openbsd
openssh-client
openssh-server
patch
pkg-config
procps
psmisc
python3
ripgrep
rsync
sudo
tini
tmux
traceroute
tree
unzip
vim
wget
xz-utils
zlib1g-dev
zstd
";

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

fn ssh_public_key(directory: &Path, name: &str) -> String {
    let private_key = directory.join(name);
    let status = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(&private_key)
        .status()
        .unwrap();
    assert!(status.success());
    fs::read_to_string(private_key.with_extension("pub"))
        .unwrap()
        .trim()
        .to_owned()
}

fn prepare_guest_ssh(test_root: &Path, key: &str) -> std::process::Output {
    fs::create_dir_all(test_root.join("run")).unwrap();
    let fake_sshd = test_root.join("fake-sshd");
    fs::write(
        &fake_sshd,
        "#!/bin/sh\nset -eu\ntest \"$1\" = -t\ntest \"$2\" = -f\ntest -r \"$3\"\n",
    )
    .unwrap();
    fs::set_permissions(&fake_sshd, fs::Permissions::from_mode(0o755)).unwrap();
    Command::new(root().join("images/workspace/bin/start-gascan-sshd"))
        .env("GASCAN_SSH_AUTHORIZED_KEY", key)
        .env("GASCAN_SSH_CONTRACT_TEST_ROOT", test_root)
        .env("GASCAN_SSH_CONTRACT_TEST_SSHD", &fake_sshd)
        .env("GASCAN_SSH_CONTRACT_TEST_PREPARE_ONLY", "1")
        .output()
        .unwrap()
}

#[test]
fn ssh_guest_layer_is_offline_fixed_and_network_isolated() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    let entrypoint =
        fs::read_to_string(root().join("images/workspace/bin/gascan-entrypoint")).unwrap();
    let initializer =
        fs::read_to_string(root().join("images/workspace/bin/start-gascan-sshd")).unwrap();

    for required in [
        "COPY --chmod=0555 images/workspace/bin/start-gascan-sshd /usr/local/bin/start-gascan-sshd",
        "COPY --chmod=0555 images/workspace/tests/ssh-contract.sh /opt/gascan/tests/ssh-contract.sh",
    ] {
        assert!(
            dockerfile.contains(required),
            "missing sealed SSH image input: {required}"
        );
    }
    assert!(entrypoint.contains("exec \"$@\""));
    assert!(entrypoint.contains("exec sleep infinity"));
    assert!(entrypoint.contains("GASCAN_SSH_ENABLED"));
    assert!(entrypoint.contains("/usr/bin/sudo"));
    assert!(entrypoint.contains("/usr/local/bin/start-gascan-sshd"));

    for required in [
        "ListenAddress 0.0.0.0",
        "Port 22",
        "PasswordAuthentication no",
        "KbdInteractiveAuthentication no",
        "PermitRootLogin no",
        "PubkeyAuthentication yes",
        "AuthenticationMethods publickey",
        "AuthorizedKeysFile none",
        "AuthorizedKeysCommand /bin/cat $authorized_keys",
        "AuthorizedKeysCommandUser root",
        "AllowUsers workspace",
        "PermitUserEnvironment no",
        "AllowAgentForwarding no",
        "AllowTcpForwarding local",
        "AllowStreamLocalForwarding no",
        "PermitOpen 127.0.0.1:*",
        "GatewayPorts no",
        "PermitTunnel no",
        "X11Forwarding no",
        "StrictModes yes",
        "Subsystem sftp internal-sftp",
        "findmnt -n -o TARGET -T \"$managed_config_root\"",
        "/home/workspace/.config/gascan/ssh/host/ssh_host_ed25519_key",
        "/home/workspace/.config/gascan/ssh/authorized_keys",
        "/home/workspace/.config/gascan/ssh/sshd_config",
    ] {
        assert!(
            initializer.contains(required),
            "missing locked SSH directive or managed path: {required}"
        );
    }
    for forbidden in [
        "ListenAddress 127.0.0.1",
        "ListenAddress ::",
        "/home/workspace/.ssh",
        "/etc/ssh/ssh_host_",
        "chpasswd",
        "passwd ",
        "ssh-keygen -A",
        "AllowTcpForwarding yes",
        "GatewayPorts yes",
        "PermitRootLogin yes",
    ] {
        assert!(
            !initializer.contains(forbidden),
            "unsafe SSH initialization policy: {forbidden}"
        );
    }
    let ssh_layer = dockerfile
        .split_once(
            "COPY --chmod=0555 images/workspace/bin/start-gascan-sshd /usr/local/bin/start-gascan-sshd",
        )
        .unwrap()
        .1;
    for network_acquisition in ["curl ", "wget ", "apt-get ", "git clone", "npm "] {
        assert!(
            !ssh_layer.contains(network_acquisition),
            "SSH guest layer performs acquisition: {network_acquisition}"
        );
    }
}

#[test]
fn ssh_guest_initialization_is_behavioral_atomic_and_idempotent() {
    use std::os::unix::fs::{MetadataExt, symlink};

    let temporary = tempfile::tempdir().unwrap();
    let test_root = temporary.path().join("root");
    let managed_config_root = test_root.join("home/workspace/.config");
    let config_root = test_root.join("home/workspace/.config/gascan");
    fs::create_dir_all(&config_root).unwrap();
    fs::set_permissions(&managed_config_root, fs::Permissions::from_mode(0o1770)).unwrap();
    fs::set_permissions(&config_root, fs::Permissions::from_mode(0o700)).unwrap();
    let key_one = ssh_public_key(temporary.path(), "client-one");
    let key_two = ssh_public_key(temporary.path(), "client-two");

    let first = prepare_guest_ssh(&test_root, &key_one);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let ssh_root = config_root.join("ssh");
    let private_key = ssh_root.join("host/ssh_host_ed25519_key");
    let public_key = ssh_root.join("host/ssh_host_ed25519_key.pub");
    let authorized_keys = ssh_root.join("authorized_keys");
    let config = ssh_root.join("sshd_config");
    let original_private_key = fs::read(&private_key).unwrap();
    let original_public_key = fs::read(&public_key).unwrap();
    let generated_config = fs::read_to_string(&config).unwrap();
    assert!(
        generated_config
            .lines()
            .any(|line| line == "ListenAddress 0.0.0.0"),
        "sshd must listen on the isolated sandbox network interface for Apple port publication"
    );
    assert!(
        !generated_config
            .lines()
            .any(|line| line == "ListenAddress 127.0.0.1"),
        "guest loopback is unreachable through Apple port publication"
    );
    let authorized_keys_command = format!(
        "AuthorizedKeysCommand /bin/cat {}",
        authorized_keys.display()
    );
    for directive in [
        "AuthorizedKeysFile none",
        authorized_keys_command.as_str(),
        "AuthorizedKeysCommandUser root",
        "StrictModes yes",
    ] {
        assert!(
            generated_config.lines().any(|line| line == directive),
            "missing safe authorized-key directive: {directive}"
        );
    }
    assert!(
        !generated_config.lines().any(|line| {
            line.starts_with("AuthorizedKeysFile ") && line != "AuthorizedKeysFile none"
        }),
        "sshd must not open the root-only authorized key as workspace"
    );
    let setenv_lines = generated_config
        .lines()
        .filter(|line| line.starts_with("SetEnv "))
        .collect::<Vec<_>>();
    assert_eq!(
        setenv_lines.len(),
        1,
        "OpenSSH retains only the first SetEnv directive"
    );
    for assignment in [
        "HOME=/home/workspace",
        "USER=workspace",
        "LOGNAME=workspace",
        "LANG=C.UTF-8",
        "LC_ALL=C.UTF-8",
        "XDG_DATA_HOME=/home/workspace/.local/share",
        "XDG_CACHE_HOME=/home/workspace/.cache",
        "XDG_CONFIG_HOME=/home/workspace/.config",
        "CARGO_HOME=/home/workspace/.local/share/cargo",
        "MISE_CARGO_HOME=/home/workspace/.local/share/cargo",
        "RUSTUP_HOME=/home/workspace/.local/share/rustup",
        "MISE_RUSTUP_HOME=/home/workspace/.local/share/rustup",
        "NPM_CONFIG_PREFIX=/home/workspace/.local",
        "NPM_CONFIG_CACHE=/home/workspace/.cache/npm",
        "GOPATH=/home/workspace/.local/share/go",
        "GOBIN=/home/workspace/.local/bin",
        "GOCACHE=/home/workspace/.cache/go-build",
        "GOMODCACHE=/home/workspace/.cache/go-mod",
        "PYTHONUSERBASE=/home/workspace/.local",
        "GEM_HOME=/home/workspace/.local/share/gem",
        "MIX_HOME=/home/workspace/.local/share/mix",
        "HEX_HOME=/home/workspace/.local/share/hex",
        "REBAR_CACHE_DIR=/home/workspace/.cache/rebar3",
        "MISE_CACHE_DIR=/home/workspace/.cache/mise",
        "MISE_DATA_DIR=/home/workspace/.local/share/mise",
        "MISE_GLOBAL_CONFIG_FILE=/home/workspace/.config/gascan/mise.toml",
        "MISE_SYSTEM_CONFIG_FILE=/etc/mise/config.toml",
        "MISE_STATE_DIR=/home/workspace/.config/gascan/mise-state",
        "MISE_SYSTEM_DATA_DIR=/opt/gascan/mise",
    ] {
        assert!(
            setenv_lines[0]
                .split_ascii_whitespace()
                .any(|field| field == assignment),
            "combined SetEnv omits exact assignment: {assignment}"
        );
    }
    assert!(
        setenv_lines[0]
            .split_ascii_whitespace()
            .any(|field| field == format!("PATH={RUNTIME_PATH}")),
        "combined SetEnv omits exact PATH"
    );

    let second = prepare_guest_ssh(&test_root, &key_two);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(fs::read(&private_key).unwrap(), original_private_key);
    assert_eq!(fs::read(&public_key).unwrap(), original_public_key);
    assert_eq!(
        fs::read_to_string(&authorized_keys).unwrap(),
        format!("{key_two}\n")
    );
    assert_eq!(
        fs::symlink_metadata(&config_root).unwrap().mode() & 0o7777,
        0o1770,
        "Gas Can config boundary must be sticky and root-controlled while remaining group-writable"
    );
    assert_eq!(
        fs::symlink_metadata(&managed_config_root).unwrap().mode() & 0o7777,
        0o1770,
        "managed config root must remain sticky and group-writable"
    );
    let non_ssh_state = config_root.join("agent-state");
    fs::create_dir(&non_ssh_state).unwrap();
    fs::write(non_ssh_state.join("config"), "workspace-writable\n").unwrap();

    for (path, mode) in [
        (&ssh_root, 0o700),
        (&ssh_root.join("host"), 0o700),
        (&private_key, 0o600),
        (&public_key, 0o644),
        (&authorized_keys, 0o600),
        (&config, 0o600),
    ] {
        let metadata = fs::symlink_metadata(path).unwrap();
        assert!(
            !metadata.file_type().is_symlink(),
            "symlink: {}",
            path.display()
        );
        assert_eq!(metadata.mode() & 0o777, mode, "mode: {}", path.display());
        if metadata.is_file() {
            assert_eq!(metadata.nlink(), 1, "hard-linked path: {}", path.display());
        }
    }

    fs::set_permissions(&private_key, fs::Permissions::from_mode(0o644)).unwrap();
    let unsafe_existing = prepare_guest_ssh(&test_root, &key_one);
    assert!(
        !unsafe_existing.status.success(),
        "unsafe existing host private key was accepted"
    );
    assert_eq!(fs::read(&private_key).unwrap(), original_private_key);

    let symlink_root = temporary.path().join("symlink-root");
    let symlink_config = symlink_root.join("home/workspace/.config/gascan");
    let outside = temporary.path().join("outside");
    fs::create_dir_all(&symlink_config).unwrap();
    fs::create_dir(&outside).unwrap();
    symlink(&outside, symlink_config.join("ssh")).unwrap();
    let symlink_result = prepare_guest_ssh(&symlink_root, &key_one);
    assert!(
        !symlink_result.status.success(),
        "managed SSH symlink was followed"
    );
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);

    for invalid in [
        "",
        "ssh-rsa AAAA",
        "command=\"id\" ssh-ed25519 AAAA",
        "ssh-ed25519 AAAA\nssh-ed25519 AAAA",
        "ssh-ed25519 not-base64",
    ] {
        let invalid_root = temporary.path().join(format!(
            "invalid-{}",
            invalid
                .as_bytes()
                .iter()
                .map(|byte| *byte as u64)
                .sum::<u64>()
        ));
        let invalid_config = invalid_root.join("home/workspace/.config/gascan");
        fs::create_dir_all(&invalid_config).unwrap();
        let output = prepare_guest_ssh(&invalid_root, invalid);
        assert!(
            !output.status.success(),
            "invalid key was accepted: {invalid:?}"
        );
        assert!(!invalid_config.join("ssh").exists());
    }

    let rsa_root = temporary.path().join("rsa-root");
    let rsa_config = rsa_root.join("home/workspace/.config/gascan");
    let rsa_host = rsa_config.join("ssh/host");
    fs::create_dir_all(&rsa_host).unwrap();
    fs::create_dir_all(rsa_root.join("run")).unwrap();
    fs::set_permissions(&rsa_config, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(rsa_config.join("ssh"), fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&rsa_host, fs::Permissions::from_mode(0o700)).unwrap();
    let rsa_private = rsa_host.join("ssh_host_ed25519_key");
    let rsa_status = Command::new("ssh-keygen")
        .args(["-q", "-t", "rsa", "-N", "", "-f"])
        .arg(&rsa_private)
        .status()
        .unwrap();
    assert!(rsa_status.success());
    let derived_rsa = Command::new("ssh-keygen")
        .args(["-y", "-P", "", "-f"])
        .arg(&rsa_private)
        .output()
        .unwrap();
    assert!(derived_rsa.status.success());
    fs::write(rsa_private.with_extension("pub"), &derived_rsa.stdout).unwrap();
    fs::set_permissions(&rsa_private, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(
        rsa_private.with_extension("pub"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let rsa_private_before = fs::read(&rsa_private).unwrap();
    let rsa_public_before = fs::read(rsa_private.with_extension("pub")).unwrap();
    let rsa_result = prepare_guest_ssh(&rsa_root, &key_one);
    assert!(
        !rsa_result.status.success(),
        "existing non-Ed25519 host key was accepted"
    );
    assert_eq!(fs::read(&rsa_private).unwrap(), rsa_private_before);
    assert_eq!(
        fs::read(rsa_private.with_extension("pub")).unwrap(),
        rsa_public_before
    );
}

#[test]
fn ssh_guest_initialization_accepts_only_an_empty_safe_volume_lost_found() {
    let temporary = tempfile::tempdir().unwrap();
    let key = ssh_public_key(temporary.path(), "volume-client");

    let valid_root = temporary.path().join("valid-volume");
    let valid_config = valid_root.join("home/workspace/.config");
    let valid_lost_found = valid_config.join("lost+found");
    fs::create_dir_all(&valid_lost_found).unwrap();
    fs::set_permissions(&valid_config, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(&valid_lost_found, fs::Permissions::from_mode(0o700)).unwrap();
    let valid = prepare_guest_ssh(&valid_root, &key);
    assert!(
        valid.status.success(),
        "{}",
        String::from_utf8_lossy(&valid.stderr)
    );

    for case in ["unexpected", "nonempty", "wrong-mode", "symlink"] {
        let test_root = temporary.path().join(case);
        let config_root = test_root.join("home/workspace/.config");
        let lost_found = config_root.join("lost+found");
        fs::create_dir_all(&config_root).unwrap();
        fs::set_permissions(&config_root, fs::Permissions::from_mode(0o755)).unwrap();
        match case {
            "unexpected" => fs::write(config_root.join("foreign"), "unsafe\n").unwrap(),
            "nonempty" => {
                fs::create_dir(&lost_found).unwrap();
                fs::set_permissions(&lost_found, fs::Permissions::from_mode(0o700)).unwrap();
                fs::write(lost_found.join("recovered"), "unsafe\n").unwrap();
            }
            "wrong-mode" => {
                fs::create_dir(&lost_found).unwrap();
                fs::set_permissions(&lost_found, fs::Permissions::from_mode(0o755)).unwrap();
            }
            "symlink" => {
                let outside = temporary.path().join("lost-found-outside");
                if !outside.exists() {
                    fs::create_dir(&outside).unwrap();
                }
                symlink(&outside, &lost_found).unwrap();
            }
            _ => unreachable!(),
        }
        let output = prepare_guest_ssh(&test_root, &key);
        assert!(
            !output.status.success(),
            "unsafe fresh-volume state was accepted: {case}"
        );
        assert!(!config_root.join("gascan/ssh").exists());
    }
}

#[test]
fn ssh_live_contract_blocks_workspace_replacement_but_allows_non_ssh_state() {
    let contract =
        fs::read_to_string(root().join("images/workspace/tests/ssh-contract.sh")).unwrap();
    for behavioral_check in [
        "if mv \"$managed_root\"",
        "if rm -rf \"$managed_root\"",
        "workspace-non-ssh-state",
        "sudo -n stat -c %F",
        "sudo -n grep -Fqx",
        "sudo -n ssh-keygen -y",
        "exec \"$name\" sudo -n ssh-keygen -l",
    ] {
        assert!(
            contract.contains(behavioral_check),
            "live SSH contract omits workspace filesystem behavior: {behavioral_check}"
        );
    }
}

#[test]
fn ssh_live_contract_normalizes_only_the_exact_sftp_effective_directive() {
    const SFTP_DIRECTIVE_FILTER: &str = r#"$1 == "subsystem" && $2 == "sftp" && $3 == "internal-sftp" && NF == 3 { found = 1 } END { exit !found }"#;

    let contract =
        fs::read_to_string(root().join("images/workspace/tests/ssh-contract.sh")).unwrap();
    assert!(
        contract.contains(SFTP_DIRECTIVE_FILTER),
        "live SSH contract must compare the exact normalized SFTP directive"
    );

    let temporary = tempfile::tempdir().unwrap();
    let effective = temporary.path().join("sshd-effective.txt");
    let accepts = |value: &str| {
        fs::write(&effective, value).unwrap();
        Command::new("awk")
            .arg(SFTP_DIRECTIVE_FILTER)
            .arg(&effective)
            .status()
            .unwrap()
            .success()
    };

    assert!(accepts("subsystem sftp internal-sftp \n"));
    for rejected in [
        "subsystem sftp internal-sftp unsafe\n",
        "subsystem sftp /usr/lib/openssh/sftp-server\n",
        "subsystem other internal-sftp\n",
    ] {
        assert!(
            !accepts(rejected),
            "accepted unsafe directive: {rejected:?}"
        );
    }
}

#[test]
fn dockerfile_assembles_workstation_only_from_the_verified_context() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    let installer =
        fs::read_to_string(root().join("images/workspace/bin/install-workstation-artifacts"))
            .unwrap();
    for required in [
        "COPY --chmod=0555 images/workspace/bin/install-workstation-artifacts /usr/local/bin/install-workstation-artifacts",
        "COPY workstation /tmp/workstation",
        "RUN --network=none test -x /opt/gascan/gascamp/bin/camp",
        "/usr/local/bin/install-workstation-artifacts",
        "/opt/gascan/workstation",
        "chown -R root:root /opt/gascan/workstation",
        "find /opt/gascan/workstation ! -type l \\( -perm -020 -o -perm -002 \\) -print -quit",
        "find /opt/gascan/workstation \\( ! -user root -o ! -group root \\) -print -quit",
    ] {
        assert!(
            dockerfile.contains(required),
            "missing offline workstation assembly contract: {required}"
        );
    }
    for required in [
        "\"ci\"",
        "\"--offline\"",
        "\"--ignore-scripts\"",
        "\"npm_config_logs_dir\": str(npm_root / \".home\" / \"logs\")",
        "verify_npm_inventory(npm_root, source / \"package-lock.json\", target_lock_path)",
        "checked_link(command_dir / \"fd\", Path(\"/usr/bin/fdfind\"))",
        "checked_link(command_dir / \"pico\", Path(\"/usr/bin/nano\"))",
    ] {
        assert!(
            installer.contains(required),
            "installer omits required npm argument: {required}"
        );
    }
    let workstation_assembly = dockerfile
        .split_once("COPY workstation /tmp/workstation")
        .unwrap()
        .1;
    for forbidden in [
        "curl https://",
        "wget https://",
        "npm install",
        "npm view",
        "registry.npmjs",
        "github.com/",
        "gitlab.com/",
    ] {
        assert!(
            !workstation_assembly.contains(forbidden) && !installer.contains(forbidden),
            "network-capable workstation assembly path: {forbidden}"
        );
    }
}

#[test]
fn workstation_step_diagnostics_name_only_the_failing_boundary() {
    let wrapper = root().join("images/workspace/bin/run-workstation-step");
    let output = Command::new(&wrapper)
        .args([
            "immutable-owner",
            "sh",
            "-c",
            "printf 'private-command-output\\n' >&2; exit 7",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "private-command-output\nworkstation assembly: immutable-owner failed\n"
    );

    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    for boundary in [
        "installer",
        "immutable-owner",
        "immutable-mode",
        "immutable-ownership",
        "home-directories",
        "home-configuration",
        "gascan-config-boundary",
        "home-link-owner",
        "sudoers-mode",
        "sudoers-validation",
        "chromium-mode",
        "chromium-command",
        "chromium-seal",
        "temporary-cleanup",
    ] {
        assert!(
            dockerfile.contains(&format!("run-workstation-step {boundary} ")),
            "missing named workstation boundary {boundary}"
        );
    }
    assert!(dockerfile.contains("workstation assembly: installer complete"));
}

#[test]
fn immutable_mode_check_excludes_links_but_rejects_writable_content() {
    let temporary = tempfile::tempdir().unwrap();
    let tree = temporary.path().join("immutable");
    fs::create_dir(&tree).unwrap();
    let target = tree.join("target");
    fs::write(&target, b"locked\n").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o444)).unwrap();
    std::os::unix::fs::symlink("target", tree.join("command")).unwrap();
    let check = r#"test -z "$(find "$1" ! -type l \( -perm -020 -o -perm -002 \) -print -quit)""#;
    assert!(
        Command::new("sh")
            .args(["-c", check, "sh", tree.to_str().unwrap()])
            .status()
            .unwrap()
            .success()
    );
    fs::set_permissions(&target, fs::Permissions::from_mode(0o464)).unwrap();
    assert!(
        !Command::new("sh")
            .args(["-c", check, "sh", tree.to_str().unwrap()])
            .status()
            .unwrap()
            .success()
    );

    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    assert!(dockerfile.contains(
        r#"find /opt/gascan/workstation ! -type l \( -perm -020 -o -perm -002 \) -print -quit"#
    ));
}

#[test]
fn workstation_installer_rejects_unsafe_archives_behaviorally() {
    let installer = root().join("images/workspace/bin/install-workstation-artifacts");
    let temp = tempfile::tempdir().unwrap();
    let make_archive = |name: &str, python_body: &str| {
        let archive = temp.path().join(name);
        let status = Command::new("python3")
            .args(["-c", python_body])
            .arg(&archive)
            .status()
            .unwrap();
        assert!(status.success());
        archive
    };

    let safe = make_archive(
        "safe.tar.gz",
        "import io,sys,tarfile\np=sys.argv[1]\nwith tarfile.open(p,'w:gz') as t:\n i=tarfile.TarInfo('tree/bin/tool'); i.mode=0o755; d=b'ok'; i.size=len(d); t.addfile(i,io.BytesIO(d))",
    );
    assert!(
        Command::new(&installer)
            .args(["validate-tar", safe.to_str().unwrap()])
            .status()
            .unwrap()
            .success()
    );

    for (name, body) in [
        (
            "traversal.tar.gz",
            "import io,sys,tarfile\nwith tarfile.open(sys.argv[1],'w:gz') as t:\n i=tarfile.TarInfo('../escape'); d=b'x'; i.size=1; t.addfile(i,io.BytesIO(d))",
        ),
        (
            "absolute.tar.gz",
            "import io,sys,tarfile\nwith tarfile.open(sys.argv[1],'w:gz') as t:\n i=tarfile.TarInfo('/escape'); d=b'x'; i.size=1; t.addfile(i,io.BytesIO(d))",
        ),
        (
            "device.tar.gz",
            "import sys,tarfile\nwith tarfile.open(sys.argv[1],'w:gz') as t:\n i=tarfile.TarInfo('dev'); i.type=tarfile.CHRTYPE; t.addfile(i)",
        ),
        (
            "escaping-link.tar.gz",
            "import sys,tarfile\nwith tarfile.open(sys.argv[1],'w:gz') as t:\n i=tarfile.TarInfo('tree/link'); i.type=tarfile.SYMTYPE; i.linkname='../../escape'; t.addfile(i)",
        ),
    ] {
        let archive = make_archive(name, body);
        assert!(
            !Command::new(&installer)
                .args(["validate-tar", archive.to_str().unwrap()])
                .status()
                .unwrap()
                .success(),
            "unsafe archive passed: {name}"
        );
    }
}

#[test]
fn workstation_installer_accepts_only_the_reviewed_starship_archive_behaviorally() {
    let installer = root().join("images/workspace/bin/install-workstation-artifacts");
    let temporary = tempfile::tempdir().unwrap();
    let make_archive = |name: &str, scenario: &str| {
        let archive = temporary.path().join(name);
        let status = Command::new("python3")
            .args([
                "-c",
                r#"import io,sys,tarfile
archive,scenario=sys.argv[1:]
elf=bytearray(20)
elf[:6]=b'\x7fELF\x02\x01'
elf[18:20]=(183).to_bytes(2,'little')
def add_file(t,name,data,mode=0o755):
 i=tarfile.TarInfo(name); i.mode=mode; i.size=len(data); t.addfile(i,io.BytesIO(data))
with tarfile.open(archive,'w:gz') as t:
 if scenario == 'duplicate':
  add_file(t,'starship',elf); add_file(t,'starship',elf)
 elif scenario == 'escaping-link':
  add_file(t,'starship',elf)
  i=tarfile.TarInfo('escape'); i.type=tarfile.SYMTYPE; i.linkname='../escape'; t.addfile(i)
 elif scenario == 'wrong-arch':
  elf[18:20]=(62).to_bytes(2,'little'); add_file(t,'starship',elf)
 elif scenario == 'extra-executable':
  add_file(t,'starship',elf); add_file(t,'helper',elf)
 else:
  add_file(t,'starship',elf)
"#,
            ])
            .args([archive.to_str().unwrap(), scenario])
            .status()
            .unwrap();
        assert!(status.success());
        archive
    };
    let install = |archive: &Path, destination: &Path, size: u64, digest: &str| {
        Command::new(&installer)
            .args([
                "install-starship",
                archive.to_str().unwrap(),
                destination.to_str().unwrap(),
                &size.to_string(),
                digest,
            ])
            .status()
            .unwrap()
    };

    let valid = make_archive("valid-starship.tar.gz", "valid");
    let valid_bytes = fs::read(&valid).unwrap();
    let valid_size = u64::try_from(valid_bytes.len()).unwrap();
    let valid_sha = format!("{:x}", Sha256::digest(&valid_bytes));
    assert!(
        !install(
            &valid,
            &temporary.path().join("wrong-size"),
            valid_size - 1,
            &valid_sha
        )
        .success(),
        "Starship archive size mismatch was accepted"
    );
    assert!(
        !install(
            &valid,
            &temporary.path().join("wrong-digest"),
            valid_size,
            &"0".repeat(64)
        )
        .success(),
        "Starship archive digest mismatch was accepted"
    );
    let installed = temporary.path().join("installed-starship");
    assert!(
        install(&valid, &installed, valid_size, &valid_sha).success(),
        "reviewed Starship archive was rejected"
    );
    assert_eq!(fs::read(&installed).unwrap()[..6], *b"\x7fELF\x02\x01");

    for (name, scenario) in [
        ("duplicate-starship.tar.gz", "duplicate"),
        ("escaping-starship.tar.gz", "escaping-link"),
        ("wrong-arch-starship.tar.gz", "wrong-arch"),
        ("extra-executable-starship.tar.gz", "extra-executable"),
    ] {
        let archive = make_archive(name, scenario);
        let bytes = fs::read(&archive).unwrap();
        let size = u64::try_from(bytes.len()).unwrap();
        let sha = format!("{:x}", Sha256::digest(&bytes));
        assert!(
            !install(
                &archive,
                &temporary.path().join(format!("installed-{scenario}")),
                size,
                &sha
            )
            .success(),
            "invalid Starship archive was accepted: {scenario}"
        );
    }
}

#[test]
fn workstation_installer_enforces_exact_target_npm_inventory_behaviorally() {
    let installer = root().join("images/workspace/bin/install-workstation-artifacts");
    let temporary = tempfile::tempdir_in("/tmp").unwrap();
    let npm_root = temporary.path().join("npm");
    let alpha = npm_root.join("node_modules/alpha");
    fs::create_dir_all(&alpha).unwrap();
    fs::write(
        alpha.join("package.json"),
        "{\"name\":\"alpha\",\"version\":\"1.0.0\"}\n",
    )
    .unwrap();
    let package_lock = temporary.path().join("package-lock.json");
    fs::write(
        &package_lock,
        "{\"lockfileVersion\":3,\"packages\":{\"\":{},\"node_modules/alpha\":{\"name\":\"alpha\",\"version\":\"1.0.0\"},\"node_modules/excluded\":{\"name\":\"excluded\",\"version\":\"1.0.0\"}}}\n",
    )
    .unwrap();
    let target_lock = temporary.path().join("target-lock.toml");
    fs::write(
        &target_lock,
        "schema_version = 1\nnpm_version = \"11.12.1\"\nos = \"linux\"\ncpu = \"arm64\"\nlibc = \"glibc\"\nrecord_count = 1\nexcluded_paths = [\"node_modules/excluded\"]\n",
    )
    .unwrap();
    let verify = || {
        Command::new(&installer)
            .args([
                "verify-inventory",
                npm_root.to_str().unwrap(),
                package_lock.to_str().unwrap(),
                target_lock.to_str().unwrap(),
            ])
            .status()
            .unwrap()
    };
    assert!(verify().success(), "exact target inventory was rejected");

    let extra = npm_root.join("node_modules/extra");
    fs::create_dir(&extra).unwrap();
    fs::write(
        extra.join("package.json"),
        "{\"name\":\"extra\",\"version\":\"1.0.0\"}\n",
    )
    .unwrap();
    assert!(!verify().success(), "extra package was accepted");
    fs::remove_dir_all(extra).unwrap();

    fs::write(
        alpha.join("package.json"),
        "{\"name\":\"alpha\",\"version\":\"2.0.0\"}\n",
    )
    .unwrap();
    assert!(!verify().success(), "wrong package identity was accepted");
    fs::remove_file(alpha.join("package.json")).unwrap();
    assert!(!verify().success(), "missing package manifest was accepted");

    fs::remove_dir_all(&alpha).unwrap();
    symlink(temporary.path(), &alpha).unwrap();
    assert!(!verify().success(), "symlink package root was accepted");
}

#[test]
fn workstation_installer_enforces_file_npm_version_and_mode_boundaries_behaviorally() {
    let installer = root().join("images/workspace/bin/install-workstation-artifacts");
    let temp = tempfile::tempdir().unwrap();

    let elf = temp.path().join("tool");
    let mut elf_bytes = vec![0_u8; 20];
    elf_bytes[..6].copy_from_slice(b"\x7fELF\x02\x01");
    elf_bytes[18..20].copy_from_slice(&183_u16.to_le_bytes());
    fs::write(&elf, &elf_bytes).unwrap();
    let sha = format!("{:x}", Sha256::digest(&elf_bytes));
    let validate = |size: &str, digest: &str, path: &Path| {
        Command::new(&installer)
            .args([
                "validate-file",
                path.to_str().unwrap(),
                size,
                digest,
                "arm64-elf",
            ])
            .status()
            .unwrap()
    };
    assert!(validate("20", &sha, &elf).success());
    assert!(!validate("19", &sha, &elf).success());
    assert!(!validate("20", &"0".repeat(64), &elf).success());
    fs::write(&elf, vec![0_u8; 20]).unwrap();
    let non_elf_sha = format!("{:x}", Sha256::digest(vec![0_u8; 20]));
    assert!(!validate("20", &non_elf_sha, &elf).success());

    let source = temp.path().join("npm-source");
    let destination = temp.path().join("npm-destination");
    let fake_bin = temp.path().join("fake-bin");
    fs::create_dir(&source).unwrap();
    fs::create_dir(source.join("npm-cache")).unwrap();
    fs::create_dir(&fake_bin).unwrap();
    fs::write(source.join("package.json"), "{}\n").unwrap();
    fs::write(source.join("package-lock.json"), "{}\n").unwrap();
    let bootstrap = temp.path().join("npm-cli.tgz");
    let bootstrap_status = Command::new("python3")
        .args([
            "-c",
            "import io,json,sys,tarfile\nwith tarfile.open(sys.argv[1],'w:gz') as t:\n files={'package/package.json':json.dumps({'name':'npm','version':'11.12.1','bin':{'npm':'bin/npm-cli.js','npx':'bin/npx-cli.js'}}).encode(),'package/bin/npm-cli.js':b'require(\"../lib/cli.js\");\\n','package/lib/cli.js':b''}\n for name,data in files.items():\n  i=tarfile.TarInfo(name); i.mode=0o755 if name.endswith('.js') else 0o644; i.size=len(data); t.addfile(i,io.BytesIO(data))",
        ])
        .arg(&bootstrap)
        .status()
        .unwrap();
    assert!(bootstrap_status.success());
    let args_file = temp.path().join("npm-args");
    let fake_node = fake_bin.join("node");
    fs::write(
        &fake_node,
        "#!/bin/sh\nprintf '%s\\n' CALL \"$@\" >>\"$NPM_ARGS\"\nif [ \"$1\" = --version ]; then printf '%s\\n' \"$FAKE_NODE_VERSION\"; exit 0; fi\nif [ \"$2\" = --version ]; then printf '%s\\n' \"$FAKE_NPM_VERSION\"; exit 0; fi\nmkdir -p node_modules\n",
    )
    .unwrap();
    fs::set_permissions(&fake_node, fs::Permissions::from_mode(0o755)).unwrap();
    let status = Command::new(&installer)
        .args([
            "npm-ci",
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
            bootstrap.to_str().unwrap(),
            "11.12.1",
            "24.18.0",
        ])
        .env(
            "PATH",
            format!("{}:{}", fake_bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("NPM_ARGS", &args_file)
        .env("FAKE_NPM_VERSION", "11.12.1")
        .env("FAKE_NODE_VERSION", "v24.18.0")
        .status()
        .unwrap();
    assert!(status.success());
    let calls = fs::read_to_string(&args_file).unwrap();
    assert!(calls.starts_with("CALL\n--version\nCALL\n"));
    assert!(calls.contains("/package/bin/npm-cli.js\n--version\nCALL\n"));
    assert!(
        calls.ends_with(&format!(
            "/package/bin/npm-cli.js\nci\n--offline\n--ignore-scripts\n--cache\n{}\n",
            source.join("npm-cache").display()
        )),
        "{calls}"
    );
    fs::write(&args_file, "").unwrap();
    let mismatch_destination = temp.path().join("npm-mismatch");
    let mismatch = Command::new(&installer)
        .args([
            "npm-ci",
            source.to_str().unwrap(),
            mismatch_destination.to_str().unwrap(),
            bootstrap.to_str().unwrap(),
            "11.12.1",
            "24.18.0",
        ])
        .env(
            "PATH",
            format!("{}:{}", fake_bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("NPM_ARGS", &args_file)
        .env("FAKE_NPM_VERSION", "11.12.2")
        .env("FAKE_NODE_VERSION", "v24.18.0")
        .status()
        .unwrap();
    assert!(!mismatch.success(), "accepted the wrong npm version");
    assert!(!mismatch_destination.exists());
    let calls = fs::read_to_string(&args_file).unwrap();
    assert!(calls.starts_with("CALL\n--version\nCALL\n"));
    assert!(calls.ends_with("/package/bin/npm-cli.js\n--version\n"));

    let version = temp.path().join("version-tool");
    fs::write(&version, "#!/bin/sh\nexit 99\n").unwrap();
    fs::set_permissions(&version, fs::Permissions::from_mode(0o755)).unwrap();
    let version_status = |tool: &str, expected: &str, output: &str| {
        fs::write(
            &version,
            format!(
                "#!/bin/sh\nprintf '%s' '{}'\n",
                output.replace('\'', "'\\''")
            ),
        )
        .unwrap();
        Command::new(&installer)
            .args(["verify-version", version.to_str().unwrap(), tool, expected])
            .status()
            .unwrap()
    };
    fs::write(
        &version,
        "#!/bin/sh\n\
         set -eu\n\
         test -d \"$HOME\"\n\
         test \"$HOME\" != /tmp\n\
         printf '%s\\n' 'codex-cli 0.145.0'\n",
    )
    .unwrap();
    assert!(
        Command::new(&installer)
            .args([
                "verify-version",
                version.to_str().unwrap(),
                "codex",
                "0.145.0",
            ])
            .status()
            .unwrap()
            .success(),
        "version verification did not provide a safe private home"
    );
    let oversized_starship_metadata = format!(
        "starship 1.25.1\n\
         branch:master\n\
         commit_hash:8758daa\n\
         build_time:2026-04-30 19:35:31 +00:00\n\
         build_env:{}\n",
        "x".repeat(513)
    );
    for (tool, expected, output) in [
        ("claude", "2.1.218", "2.1.218 (Claude Code)\n"),
        ("codex", "0.145.0", "codex-cli 0.145.0\n"),
        ("pi", "0.81.1", "0.81.1\n"),
        ("herdr", "0.7.5", "herdr 0.7.5\n"),
        ("glab", "1.109.0", "glab 1.109.0 (abcdef)\n"),
        (
            "nvim",
            "0.11.7",
            "NVIM v0.11.7\nBuild type: Release\nLuaJIT 2.1\n",
        ),
        ("starship", "1.25.1", "starship 1.25.1\n"),
        (
            "starship",
            "1.25.1",
            "starship 1.25.1\n\
             branch:master\n\
             commit_hash:8758daa\n\
             build_time:2026-04-30 19:35:31 +00:00\n\
             build_env:rustc 1.95.0 (59807616e 2026-04-14),\n",
        ),
    ] {
        assert!(
            version_status(tool, expected, output).success(),
            "real-shaped {tool} output was rejected"
        );
    }
    for (tool, expected, output) in [
        ("pi", "0.81.1", "pi 0.81.1\n"),
        ("pi", "0.81.1", "0.81.1beta\n"),
        ("codex", "0.145.0", "codex-cli 0.145.0-rc1\n"),
        ("claude", "2.1.218", "2.1.218+other (Claude Code)\n"),
        ("herdr", "0.7.5", "other 0.7.5\n"),
        ("glab", "1.109.0", "glab 1.109.0beta (abcdef)\n"),
        ("nvim", "0.11.7", "NVIM v0.11.70\n"),
        ("starship", "1.25.1", "starship 1.25.10\n"),
        ("starship", "1.25.1", "leading junk\nstarship 1.25.1\n"),
        ("starship", "1.25.1", "\nstarship 1.25.1\n"),
        (
            "starship",
            "1.25.1",
            "diagnostic\nstarship 1.25.1\nbranch:master\n",
        ),
        (
            "starship",
            "1.25.1",
            "starship 1.25.1\n\
             branch:master\n\
             commit_hash:8758daa\n\
             build_time:2026-04-30 19:35:31 +00:00\n\
             build_env:rustc 1.95.0 (59807616e 2026-04-14),\n\
             unexpected:metadata\n",
        ),
        ("starship", "1.25.1", oversized_starship_metadata.as_str()),
    ] {
        assert!(
            !version_status(tool, expected, output).success(),
            "spoofed/suffixed {tool} output was accepted: {output:?}"
        );
    }

    let tree = temp.path().join("tree");
    fs::create_dir(&tree).unwrap();
    fs::write(tree.join("metadata"), "locked").unwrap();
    fs::write(tree.join("executable"), "#!/bin/sh\n").unwrap();
    fs::set_permissions(tree.join("metadata"), fs::Permissions::from_mode(0o666)).unwrap();
    fs::set_permissions(tree.join("executable"), fs::Permissions::from_mode(0o777)).unwrap();
    assert!(
        Command::new(&installer)
            .args(["seal-tree", tree.to_str().unwrap()])
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        fs::metadata(&tree).unwrap().permissions().mode() & 0o777,
        0o555
    );
    assert_eq!(
        fs::metadata(tree.join("metadata"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o444
    );
    assert_eq!(
        fs::metadata(tree.join("executable"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o555
    );

    let escaping_tree = temp.path().join("escaping-tree");
    fs::create_dir(&escaping_tree).unwrap();
    symlink(temp.path(), escaping_tree.join("escape")).unwrap();
    assert!(
        !Command::new(&installer)
            .args(["seal-tree", escaping_tree.to_str().unwrap()])
            .status()
            .unwrap()
            .success(),
        "immutable tree accepted an escaping symlink"
    );
}

fn assert_exact_system_tools(package_text: &str) -> Result<(), &'static str> {
    if package_text != EXPECTED_SYSTEM_TOOLS {
        return Err("reviewed Ubuntu root package set changed");
    }
    Ok(())
}

fn assert_sole_reviewed_package_install(dockerfile: &str) -> Result<(), &'static str> {
    if dockerfile.lines().any(|line| {
        line.split_whitespace()
            .collect::<Vec<_>>()
            .windows(2)
            .any(|tokens| tokens == ["apt", "install"])
    }) {
        return Err("direct apt install bypasses the reviewed package file");
    }
    let apt_get_lines: Vec<_> = dockerfile
        .lines()
        .map(str::trim)
        .filter(|line| line.contains("apt-get"))
        .collect();
    if apt_get_lines
        != [
            "&& apt-get -o Acquire::Retries=0 update \\",
            "&& DEBIAN_FRONTEND=noninteractive xargs apt-get \\",
            "&& apt-get clean \\",
        ]
    {
        return Err("apt-get must only update, install from the reviewed file, and clean");
    }
    if !dockerfile.contains(
        "&& DEBIAN_FRONTEND=noninteractive xargs apt-get \\\n         -o Acquire::Retries=0 install --yes --no-install-recommends </tmp/system-tools.txt \\",
    ) {
        return Err("package install must consume only the reviewed file");
    }
    Ok(())
}

fn assert_correct_otp_release_term_check(script: &str) -> Result<(), &'static str> {
    let exact = r#"erlang:system_info(otp_release) =:= "29""#;
    if !script.contains(exact) {
        return Err("OTP release must use strict equality with Erlang's string/list result");
    }
    if script.contains(r#"otp_release) =:= <<"29">>"#) {
        return Err("OTP release list must not be compared with an Erlang binary");
    }
    Ok(())
}

fn effective_env_value<'a>(dockerfile: &'a str, variable: &str) -> Option<&'a str> {
    dockerfile
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("ENV "))
        .flat_map(str::split_whitespace)
        .filter_map(|assignment| assignment.split_once('='))
        .filter_map(|(name, value)| (name == variable).then_some(value))
        .next_back()
}

fn assert_env_before_first_install(
    dockerfile: &str,
    variable: &str,
    value: &str,
) -> Result<(), &'static str> {
    let first_install = dockerfile
        .find("mise install --yes")
        .ok_or("missing mise install")?;
    let declaration = format!("ENV {variable}={value}");
    let position = dockerfile
        .find(&declaration)
        .ok_or("missing build-time environment")?;
    if position >= first_install {
        return Err("build-time environment must be set before mise installs tools");
    }
    Ok(())
}

fn assert_effective_env(dockerfile: &str, variable: &str, value: &str) -> Result<(), &'static str> {
    if effective_env_value(dockerfile, variable) != Some(value) {
        return Err("effective environment differs from runtime policy");
    }
    Ok(())
}

#[test]
fn dockerfile_assembles_the_connected_workspace_base() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    for required in [
        "FROM ${BASE_IMAGE} AS workspace-base",
        "apt-get -o Acquire::Retries=0 update",
        "install --yes --no-install-recommends",
        "rm -rf /var/lib/apt/lists/*",
        "COPY --chmod=0555 .artifacts/mise-linux-arm64 /usr/local/bin/mise",
        "mise install --yes",
        "mise ls --current --installed --json",
        "cmp --silent /tmp/resolved-tool-versions.json /tmp/expected-tool-versions.json",
    ] {
        assert!(
            dockerfile.contains(required),
            "missing connected contract: {required}"
        );
    }
    for forbidden in [
        "bundles/ubuntu_packages",
        "bundles/mise_runtimes",
        "Dir::Bin::methods=/nonexistent",
        "apt-get upgrade",
        "latest",
    ] {
        assert!(
            !dockerfile.contains(forbidden),
            "deferred/unlocked path: {forbidden}"
        );
    }
}

#[test]
fn dockerfile_creates_traversable_mise_config_directory_before_copying_config() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    let directory = dockerfile
        .find("RUN install -d -o root -g root -m 0555 /etc/mise")
        .expect("missing explicit root-owned mode 0555 /etc/mise creation");
    let config = dockerfile
        .find("COPY --chmod=0444 images/workspace/etc/mise/config.toml /etc/mise/config.toml")
        .unwrap();
    assert!(
        directory < config,
        "/etc/mise must be created before config.toml is copied"
    );
}

#[test]
fn dockerfile_separates_build_time_and_runtime_rust_homes() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    assert_env_before_first_install(&dockerfile, "CARGO_HOME", "/opt/gascan/mise/cargo").unwrap();
    assert_env_before_first_install(&dockerfile, "RUSTUP_HOME", "/opt/gascan/mise/rustup").unwrap();
    assert_effective_env(
        &dockerfile,
        "CARGO_HOME",
        "/home/workspace/.local/share/cargo",
    )
    .unwrap();
    assert_effective_env(
        &dockerfile,
        "RUSTUP_HOME",
        "/home/workspace/.local/share/rustup",
    )
    .unwrap();
}

#[test]
fn rustup_home_contract_rejects_incorrect_final_overrides() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    for later_override in ["ENV CARGO_HOME=/tmp/cargo", "ENV RUSTUP_HOME=/tmp/rustup"] {
        let mutated = format!("{dockerfile}\n{later_override}\n");
        let variable = later_override
            .strip_prefix("ENV ")
            .unwrap()
            .split_once('=')
            .unwrap()
            .0;
        let expected = if variable == "CARGO_HOME" {
            "/home/workspace/.local/share/cargo"
        } else {
            "/home/workspace/.local/share/rustup"
        };
        assert!(
            assert_effective_env(&mutated, variable, expected).is_err(),
            "accepted later override: {later_override}"
        );
    }
}

#[test]
fn dockerfile_final_stage_matches_writable_runtime_policy() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    for (variable, value) in [
        ("XDG_DATA_HOME", "/home/workspace/.local/share"),
        ("XDG_CACHE_HOME", "/home/workspace/.cache"),
        ("XDG_CONFIG_HOME", "/home/workspace/.config"),
        ("MISE_DATA_DIR", "/home/workspace/.local/share/mise"),
        ("MISE_CACHE_DIR", "/home/workspace/.cache/mise"),
        (
            "MISE_GLOBAL_CONFIG_FILE",
            "/home/workspace/.config/gascan/mise.toml",
        ),
        ("MISE_SYSTEM_CONFIG_FILE", "/etc/mise/config.toml"),
        (
            "MISE_STATE_DIR",
            "/home/workspace/.config/gascan/mise-state",
        ),
        ("MISE_SYSTEM_DATA_DIR", "/opt/gascan/mise"),
        ("CARGO_HOME", "/home/workspace/.local/share/cargo"),
        ("MISE_CARGO_HOME", "/home/workspace/.local/share/cargo"),
        ("RUSTUP_HOME", "/home/workspace/.local/share/rustup"),
        ("MISE_RUSTUP_HOME", "/home/workspace/.local/share/rustup"),
        ("NPM_CONFIG_PREFIX", "/home/workspace/.local"),
        ("NPM_CONFIG_CACHE", "/home/workspace/.cache/npm"),
        ("GOPATH", "/home/workspace/.local/share/go"),
        ("GOBIN", "/home/workspace/.local/bin"),
        ("GOCACHE", "/home/workspace/.cache/go-build"),
        ("GOMODCACHE", "/home/workspace/.cache/go-mod"),
        ("PYTHONUSERBASE", "/home/workspace/.local"),
        ("GEM_HOME", "/home/workspace/.local/share/gem"),
        ("MIX_HOME", "/home/workspace/.local/share/mix"),
        ("HEX_HOME", "/home/workspace/.local/share/hex"),
        ("REBAR_CACHE_DIR", "/home/workspace/.cache/rebar3"),
    ] {
        assert_effective_env(&dockerfile, variable, value).unwrap();
    }
    assert_effective_env(
        &dockerfile,
        "PATH",
        concat!(
            "/home/workspace/.local/bin:",
            "/home/workspace/.local/share/cargo/bin:",
            "/home/workspace/.local/share/go/bin:",
            "/home/workspace/.local/share/gem/bin:",
            "/home/workspace/.local/share/mise/shims:",
            "/opt/gascan/mise/shims:",
            "/usr/local/sbin:/usr/local/bin:",
            "/opt/gascan/workstation/bin:",
            "/usr/sbin:/usr/bin:/sbin:/bin"
        ),
    )
    .unwrap();
    let volume = dockerfile
        .lines()
        .rfind(|line| line.starts_with("VOLUME "))
        .unwrap();
    assert_eq!(
        volume,
        "VOLUME [\"/home/workspace/.local\", \"/home/workspace/.cache\", \"/home/workspace/.config\"]"
    );
    assert!(dockerfile.contains(
        "COPY --chmod=0555 images/workspace/bin/initialize-rust-home /usr/local/bin/initialize-rust-home"
    ));
}

fn normalize_mise_ls(input: &str) -> std::process::Output {
    Command::new("jq")
        .args([
            "--exit-status",
            "--compact-output",
            "--sort-keys",
            MISE_LS_FILTER,
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(input.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap()
}

#[test]
fn mise_ls_schema_requires_one_active_installed_record_per_preserved_key() {
    let record =
        |version: &str| format!(r#"[{{"version":"{version}","installed":true,"active":true}}]"#);
    let valid = format!(
        r#"{{"elixir":{},"erlang":{},"go":{},"java":{},"node":{},"python":{},"ruby":{},"rust":{}}}"#,
        record("1.20.2-otp-29"),
        record("29.0.3"),
        record("1.26.5"),
        record("25.0.2"),
        record("24.18.0"),
        record("3.14.6"),
        record("3.4.10"),
        record("1.97.0")
    );
    let output = normalize_mise_ls(&valid);
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), r#"{"elixir":"1.20.2-otp-29","erlang":"29.0.3","go":"1.26.5","java":"25.0.2","node":"24.18.0","python":"3.14.6","ruby":"3.4.10","rust":"1.97.0"}"#.to_owned() + "\n");
    for invalid in [
        valid.replace(&record("29.0.3"), "[]"),
        valid.replace(
            &record("29.0.3"),
            &format!(
                "[{},{}]",
                &record("29.0.3")[1..record("29.0.3").len() - 1],
                &record("29.0.3")[1..record("29.0.3").len() - 1]
            ),
        ),
        valid.replace(r#""installed":true"#, r#""installed":false"#),
        valid.replace(r#""active":true"#, r#""active":false"#),
    ] {
        assert!(
            !normalize_mise_ls(&invalid).status.success(),
            "accepted {invalid}"
        );
    }
    let extra = valid.replacen(
        '{',
        r#"{"unexpected":[{"version":"1","installed":true,"active":true}],"#,
        1,
    );
    assert!(!normalize_mise_ls(&extra).status.success());
}

#[test]
fn dockerfile_uses_supported_mise_ls_schema_and_exact_filter() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    assert!(dockerfile.contains("mise ls --current --installed --json"));
    assert!(!dockerfile.contains("mise current --json"));
    assert!(dockerfile.contains(MISE_LS_FILTER));
}

#[test]
fn dockerfile_installs_pinned_erlang_before_elixir_and_validates_otp_29() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    let erlang = dockerfile.find("mise install --yes erlang@29.0.3").unwrap();
    let otp = dockerfile
        .find("mise exec erlang@29.0.3 -- erl -noshell -eval")
        .unwrap();
    let elixir = dockerfile
        .find("mise exec erlang@29.0.3 -- mise install --yes elixir@1.20.2-otp-29")
        .unwrap();
    let remaining = dockerfile.find("mise install --yes go@1.26.5").unwrap();
    assert!(erlang < otp && otp < elixir && elixir < remaining);
    assert!(!dockerfile.contains("&& erl -noshell"));
    assert!(dockerfile.contains("otp_release"));
    assert_correct_otp_release_term_check(&dockerfile).unwrap();
    assert!(dockerfile.contains(r#"test "$(mise current elixir)" = "1.20.2-otp-29""#));
}

#[test]
fn otp_release_contract_rejects_binary_type_and_wrong_major() {
    let valid = r#"true = (erlang:system_info(otp_release) =:= "29"), halt()."#;
    assert!(assert_correct_otp_release_term_check(valid).is_ok());
    assert!(
        assert_correct_otp_release_term_check(&valid.replace(r#""29""#, r#"<<"29">>"#)).is_err()
    );
    assert!(assert_correct_otp_release_term_check(&valid.replace("29", "28")).is_err());
}

#[test]
fn dockerfile_prints_safe_mise_version_metadata_only_when_the_lock_comparison_fails() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    assert!(dockerfile.contains("if ! cmp --silent"));
    assert!(!dockerfile.contains("mise version metadata mismatch"));
    assert!(!dockerfile.contains("actual resolved versions:"));
    assert!(!dockerfile.contains("expected resolved versions:"));
}

#[test]
fn shell_assets_are_immutable_and_wired_after_identity_migration() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    for required in [
        "RUN install -d -o root -g root -m 0555 /etc/gascan",
        "COPY --chmod=0444 images/workspace/etc/gascan/bashrc /etc/gascan/bashrc",
        "COPY --chmod=0444 images/workspace/etc/gascan/starship.toml /opt/gascan/shell/presets/starship.toml",
        "COPY --chmod=0444 images/workspace/etc/gascan/starship-nerd-font.toml /opt/gascan/shell/presets/starship-nerd-font.toml",
        "COPY --chmod=0555 images/workspace/bin/configure-shell-home /usr/local/bin/configure-shell-home",
        "ln -s ../../workstation/bin/starship /opt/gascan/shell/bin/starship",
        ". /etc/gascan/bashrc",
        "ENV SHELL=/bin/bash",
    ] {
        assert!(
            dockerfile.contains(required),
            "missing managed shell image boundary: {required}"
        );
    }
    let gascan_directory = dockerfile
        .find("RUN install -d -o root -g root -m 0555 /etc/gascan")
        .unwrap();
    let bashrc_copy = dockerfile
        .find("COPY --chmod=0444 images/workspace/etc/gascan/bashrc /etc/gascan/bashrc")
        .unwrap();
    assert!(
        gascan_directory < bashrc_copy,
        "the traversable root-owned directory must exist before its immutable hook"
    );
    let migration = dockerfile
        .find("/usr/local/bin/migrate-workspace-identity")
        .unwrap();
    let startup = dockerfile.find(". /etc/gascan/bashrc").unwrap();
    assert!(
        migration < startup,
        "workspace startup was changed before identity migration"
    );
    assert_eq!(
        dockerfile.matches(". /etc/gascan/bashrc").count(),
        1,
        "the shared hook source command must be emitted by one bounded startup step"
    );
    for unsafe_writable in [
        "chmod 0775 /opt/gascan",
        "chmod 0777 /opt/gascan",
        "chown workspace:workspace /opt/gascan",
        "chown -R workspace:workspace /opt/gascan",
    ] {
        assert!(
            !dockerfile.contains(unsafe_writable),
            "shell wiring weakens immutable root: {unsafe_writable}"
        );
    }
}

#[test]
fn workspace_home_is_normalized_without_weakening_private_directories() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    let migration = dockerfile
        .find("/usr/local/bin/migrate-workspace-identity")
        .unwrap();
    let home_owner = dockerfile
        .find("chown workspace:workspace /home/workspace")
        .unwrap();
    let home_mode = dockerfile.find("chmod 0755 /home/workspace").unwrap();
    let private = dockerfile
        .find("install -d -o workspace -g workspace -m 0700")
        .unwrap();
    let config = dockerfile
        .find("install -d -o root -g workspace -m 1770 /home/workspace/.config")
        .unwrap();
    assert!(
        migration < home_owner && home_owner < home_mode && home_mode < private && private < config,
        "workspace HOME normalization is not post-migration or weakens private children"
    );
}

#[test]
fn mise_comparison_is_quiet_on_match_and_emits_only_both_json_documents_on_mismatch() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    let block = dockerfile
        .split("if ! cmp --silent")
        .nth(1)
        .unwrap()
        .split("       fi \\")
        .next()
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let actual = temp.path().join("actual.json");
    let expected = temp.path().join("expected.json");
    let script = format!(
        "if ! cmp --silent{} fi",
        block
            .replace("/tmp/resolved-tool-versions.json", actual.to_str().unwrap())
            .replace(
                "/tmp/expected-tool-versions.json",
                expected.to_str().unwrap()
            )
            .replace("\\\n", "\n")
    );
    fs::write(&actual, "{\"node\":\"20\"}\n").unwrap();
    fs::write(&expected, "{\"node\":\"20\"}\n").unwrap();
    let equal = Command::new("bash").args(["-c", &script]).output().unwrap();
    assert!(equal.status.success());
    assert!(equal.stdout.is_empty());
    assert!(equal.stderr.is_empty());
    fs::write(&expected, "{\"node\":\"22\"}\n").unwrap();
    let mismatch = Command::new("bash").args(["-c", &script]).output().unwrap();
    assert!(!mismatch.status.success());
    assert_eq!(mismatch.stdout, b"{\"node\":\"20\"}\n{\"node\":\"22\"}\n");
    assert!(mismatch.stderr.is_empty());
}

#[test]
fn dockerfile_installs_exactly_the_sorted_unique_reviewed_package_list() {
    let package_text = fs::read_to_string(root().join("tests/image/system-tools.txt")).unwrap();
    assert_exact_system_tools(&package_text).unwrap();
    assert!(package_text.ends_with('\n'));
    assert!(package_text.lines().all(|line| !line.is_empty()));
    let packages: Vec<_> = package_text.lines().collect();
    let sorted_unique: BTreeSet<_> = packages.iter().copied().collect();
    assert_eq!(packages, sorted_unique.into_iter().collect::<Vec<_>>());

    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    for required in [
        "COPY --chmod=0444 tests/image/system-tools.txt /tmp/system-tools.txt",
        "xargs apt-get \\",
        "--no-install-recommends </tmp/system-tools.txt",
        "done </tmp/system-tools.txt",
        "rm -rf /var/lib/apt/lists/* /tmp/system-tools.txt",
    ] {
        assert!(
            dockerfile.contains(required),
            "missing package contract: {required}"
        );
    }
    assert_sole_reviewed_package_install(&dockerfile).unwrap();
}

#[test]
fn workstation_package_inputs_include_bash_completion_exactly_once() {
    let package_text = fs::read_to_string(root().join("tests/image/system-tools.txt")).unwrap();
    assert_eq!(
        package_text
            .lines()
            .filter(|package| *package == "bash-completion")
            .count(),
        1
    );
    assert_exact_system_tools(&package_text).unwrap();
}

#[test]
fn exact_system_tool_contract_rejects_addition_removal_and_substitution() {
    let exact = fs::read_to_string(root().join("tests/image/system-tools.txt")).unwrap();
    assert_exact_system_tools(&(exact.clone() + "unreviewed-extra\n")).unwrap_err();
    assert_exact_system_tools(&exact.replacen("bind9-dnsutils\n", "", 1)).unwrap_err();
    assert_exact_system_tools(&exact.replacen("nano\n", "nano-tiny\n", 1)).unwrap_err();
}

#[test]
fn package_contract_rejects_an_inline_unreviewed_install() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    let mutated = format!("{dockerfile}\nRUN apt-get install arbitrary-package\n");
    assert!(assert_sole_reviewed_package_install(&mutated).is_err());
}

#[test]
fn package_contract_rejects_an_inline_unreviewed_apt_install() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    let mutated = format!("{dockerfile}\nRUN apt install arbitrary-package\n");
    assert!(assert_sole_reviewed_package_install(&mutated).is_err());
}
