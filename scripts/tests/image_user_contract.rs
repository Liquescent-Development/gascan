use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::Path,
    process::Command,
};
#[cfg(target_os = "macos")]
use std::ffi::OsString;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

fn rust_seed_command(
    script: &Path,
    source: &Path,
    destination: &Path,
    test_root: &Path,
) -> Command {
    let mut command = Command::new(script);
    command.arg(source).arg(destination);
    #[cfg(target_os = "macos")]
    {
        let bin = test_root.join("gnu-bin");
        fs::create_dir_all(&bin).unwrap();
        let mv = bin.join("mv");
        if !mv.exists() {
            let gmv = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .map(|directory| directory.join("gmv"))
                .find(|candidate| candidate.is_file())
                .expect("GNU mv is required to exercise Linux publication semantics on macOS");
            symlink(gmv, &mv).unwrap();
        }
        let mut path = vec![bin];
        path.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        command.env("PATH", std::env::join_paths(path).unwrap_or(OsString::new()));
    }
    command
}

fn write_test_toolchain(source_root: &Path, name: &str, cargo: &str) {
    let bin = source_root.join("toolchains").join(name).join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("cargo"), cargo).unwrap();
    fs::write(bin.join("rustc"), format!("rustc for {name}\n")).unwrap();
    for command in ["cargo", "rustc"] {
        fs::set_permissions(bin.join(command), fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::create_dir_all(source_root.join("update-hashes")).unwrap();
    fs::write(
        source_root.join("update-hashes").join(name),
        format!("hash for {name}\n"),
    )
    .unwrap();
}

fn rust_staging_residue(destination_root: &Path) -> Vec<String> {
    fs::read_dir(destination_root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.starts_with(".gascan-rust-"))
        .collect()
}

#[test]
fn writable_rust_bootstrap_is_idempotent_fail_closed_and_never_mutates_source() {
    let script = root().join("images/workspace/bin/initialize-rust-home");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    let bundled = "1.97.0-aarch64-unknown-linux-gnu";
    write_test_toolchain(&source, bundled, "bundled cargo\n");

    let source_cargo = source.join("toolchains").join(bundled).join("bin/cargo");
    let source_inode = fs::metadata(&source_cargo).unwrap().ino();
    let source_contents = fs::read(&source_cargo).unwrap();
    let user_toolchain = destination.join("toolchains/user-installed");
    fs::create_dir_all(&user_toolchain).unwrap();
    fs::write(user_toolchain.join("sentinel"), "keep me\n").unwrap();

    let run = || rust_seed_command(&script, &source, &destination, temp.path()).status();
    assert!(run().unwrap().success());
    let published = destination
        .join("toolchains")
        .join(bundled)
        .join("bin/cargo");
    let first_inode = fs::metadata(&published).unwrap().ino();
    let first_contents = fs::read(&published).unwrap();
    assert_eq!(first_contents, b"bundled cargo\n");
    assert_eq!(
        fs::metadata(&published).unwrap().uid(),
        fs::metadata(&destination).unwrap().uid(),
        "published files must belong to the invoking user"
    );
    assert_eq!(
        fs::read_to_string(destination.join("update-hashes").join(bundled)).unwrap(),
        "hash for 1.97.0-aarch64-unknown-linux-gnu\n"
    );

    assert!(run().unwrap().success());
    assert_eq!(fs::metadata(&published).unwrap().ino(), first_inode);
    assert_eq!(fs::read(&published).unwrap(), first_contents);
    assert_eq!(
        fs::read_to_string(user_toolchain.join("sentinel")).unwrap(),
        "keep me\n"
    );

    let additional = "1.98.0-aarch64-unknown-linux-gnu";
    write_test_toolchain(&source, additional, "new cargo\n");
    assert!(run().unwrap().success());
    assert_eq!(fs::metadata(&published).unwrap().ino(), first_inode);
    assert_eq!(fs::read(&published).unwrap(), first_contents);
    assert_eq!(
        fs::read_to_string(
            destination
                .join("toolchains")
                .join(additional)
                .join("bin/cargo")
        )
        .unwrap(),
        "new cargo\n"
    );
    let marker = destination.join(".gascan-bundled-toolchains-v1");
    assert_eq!(
        fs::read_to_string(&marker).unwrap(),
        "1.97.0-aarch64-unknown-linux-gnu\n1.98.0-aarch64-unknown-linux-gnu\n"
    );
    assert_eq!(fs::metadata(&marker).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(fs::metadata(&source_cargo).unwrap().ino(), source_inode);
    assert_eq!(fs::read(&source_cargo).unwrap(), source_contents);
}

#[test]
fn writable_rust_bootstrap_cleans_staging_and_rejects_unsafe_paths() {
    let script = root().join("images/workspace/bin/initialize-rust-home");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let incomplete = "1.97.0-aarch64-unknown-linux-gnu";
    write_test_toolchain(&source, incomplete, "cargo\n");
    fs::remove_file(
        source
            .join("toolchains")
            .join(incomplete)
            .join("bin/rustc"),
    )
    .unwrap();
    let retry_destination = temp.path().join("retry-destination");
    let failed = rust_seed_command(&script, &source, &retry_destination, temp.path())
        .status()
        .unwrap();
    assert!(!failed.success());
    assert!(rust_staging_residue(&retry_destination).is_empty());
    fs::write(
        source
            .join("toolchains")
            .join(incomplete)
            .join("bin/rustc"),
        "rustc\n",
    )
    .unwrap();
    fs::set_permissions(
        source
            .join("toolchains")
            .join(incomplete)
            .join("bin/rustc"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert!(
        rust_seed_command(&script, &source, &retry_destination, temp.path())
            .status()
            .unwrap()
            .success()
    );
    assert!(rust_staging_residue(&retry_destination).is_empty());

    for collision in ["symlink", "file"] {
        let destination = temp.path().join(format!("{collision}-destination"));
        fs::create_dir_all(destination.join("toolchains")).unwrap();
        let final_path = destination.join("toolchains").join(incomplete);
        if collision == "symlink" {
            symlink(temp.path(), &final_path).unwrap();
        } else {
            fs::write(&final_path, "unmanaged").unwrap();
        }
        let rejected = rust_seed_command(&script, &source, &destination, temp.path())
            .status()
            .unwrap();
        assert!(
            !rejected.success(),
            "accepted {collision} destination collision"
        );
        assert!(rust_staging_residue(&destination).is_empty());
    }

    for source_collision in ["symlink", "file"] {
        let unsafe_source = temp.path().join(format!("{source_collision}-source"));
        fs::create_dir_all(unsafe_source.join("toolchains")).unwrap();
        fs::create_dir_all(unsafe_source.join("update-hashes")).unwrap();
        let toolchain = unsafe_source.join("toolchains/unsafe");
        if source_collision == "symlink" {
            symlink(source.join("toolchains").join(incomplete), &toolchain).unwrap();
        } else {
            fs::write(&toolchain, "not a directory").unwrap();
        }
        let destination = temp
            .path()
            .join(format!("{source_collision}-source-destination"));
        let rejected = rust_seed_command(&script, &unsafe_source, &destination, temp.path())
            .status()
            .unwrap();
        assert!(
            !rejected.success(),
            "accepted {source_collision} source collision"
        );
    }
}

#[test]
fn workstation_contract_uses_exact_locked_command_versions() {
    let contract =
        fs::read_to_string(root().join("images/workspace/tests/workstation-contract.sh")).unwrap();
    assert!(
        !contract.contains("expect_pattern"),
        "guaranteed commands must not pass through broad version regexes"
    );
    assert!(
        contract.contains("first_line ip -Version | cut -d, -f1,2"),
        "ip normalization must retain the utility and iproute2 version fields"
    );

    let lock: toml::Value =
        toml::from_str(&fs::read_to_string(root().join("images/workspace/versions.lock")).unwrap())
            .unwrap();
    let commands = lock["workstation_commands"].as_table().unwrap();
    let expected = [
        ("cargo", "cargo 1.97.0"),
        ("rustc", "rustc 1.97.0"),
        ("vim", "VIM - Vi IMproved 9.1"),
        ("emacs", "GNU Emacs 29.3"),
        ("pico", "GNU nano, version 7.2"),
        ("gh", "gh version 2.45.0"),
        ("git", "git version 2.43.0"),
        ("ip", "ip utility, iproute2-6.1.0"),
        ("ss", "ss utility, iproute2-6.1.0"),
        ("ping", "ping from iputils 20240117"),
        ("ifconfig", "net-tools 2.10"),
        ("netstat", "net-tools 2.10"),
        ("dig", "DiG 9.18.39-0ubuntu0.24.04.5-Ubuntu"),
        ("traceroute", "Modern traceroute for Linux, version 2.1.5"),
        ("nc", "OpenBSD netcat (Debian patchlevel 1.226-1ubuntu2)"),
        ("rg", "ripgrep 14.1.0"),
        ("fd", "fdfind 9.0.0"),
        ("fzf", "0.44.1 (debian)"),
        ("tmux", "tmux 3.4"),
    ];
    assert_eq!(commands.len(), expected.len());
    for (name, version) in expected {
        assert_eq!(commands[name].as_str(), Some(version), "{name}");
        assert!(
            contract.contains(&format!("locked_version {name}")),
            "{name} is not checked against locked workstation evidence"
        );
    }
}

#[test]
fn workstation_home_configuration_is_idempotent_and_refuses_unmanaged_paths() {
    let script = root().join("images/workspace/bin/configure-workstation-home");
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let run = || Command::new(&script).env("HOME", &home).status().unwrap();
    assert!(run().success());
    assert!(run().success());
    for agent in ["claude", "codex", "pi"] {
        let link = home.join(format!(".{agent}"));
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(
            fs::read_link(link).unwrap(),
            Path::new(".config/gascan/agents").join(agent)
        );
        assert!(home.join(".config/gascan/agents").join(agent).is_dir());
        assert_eq!(
            fs::read_to_string(
                home.join(".config/gascan/agents")
                    .join(agent)
                    .join(".gascan-managed")
            )
            .unwrap(),
            "gascan-workstation-home-v1\n"
        );
    }
    for tool in ["mise", "claude", "codex", "pi", "herdr", "gh", "glab"] {
        assert!(home.join(".cache").join(tool).is_dir());
    }
    let herdr = home.join(".config/herdr");
    assert!(herdr.symlink_metadata().unwrap().file_type().is_symlink());
    assert_eq!(fs::read_link(herdr).unwrap(), Path::new("../.cache/herdr"));

    let blocked_home = temp.path().join("blocked");
    fs::create_dir(&blocked_home).unwrap();
    fs::write(blocked_home.join(".claude"), "user data").unwrap();
    let blocked = Command::new(&script)
        .env("HOME", &blocked_home)
        .status()
        .unwrap();
    assert!(!blocked.success());
    assert_eq!(
        fs::read_to_string(blocked_home.join(".claude")).unwrap(),
        "user data"
    );
    assert!(
        !blocked_home.join(".config/gascan/agents/claude").exists(),
        "refusal must occur before managed targets are created"
    );

    let blocked_herdr_home = temp.path().join("blocked-herdr");
    fs::create_dir_all(blocked_herdr_home.join(".config/herdr")).unwrap();
    let blocked_herdr = Command::new(&script)
        .env("HOME", &blocked_herdr_home)
        .status()
        .unwrap();
    assert!(!blocked_herdr.success());
    assert!(
        !blocked_herdr_home.join(".config/gascan/agents").exists(),
        "Herdr refusal must occur before managed targets are created"
    );

    for case in [
        "later-agent",
        "config",
        "cache",
        "cache-mise",
        "cache-file",
        "cache-link",
    ] {
        let adversarial_home = temp.path().join(case);
        fs::create_dir(&adversarial_home).unwrap();
        match case {
            "later-agent" => {
                let agents = adversarial_home.join(".config/gascan/agents");
                fs::create_dir_all(agents.join("codex")).unwrap();
                fs::write(
                    agents.join(".gascan-managed"),
                    "gascan-workstation-home-v1\n",
                )
                .unwrap();
            }
            "config" => fs::create_dir_all(adversarial_home.join(".config/gascan/glab")).unwrap(),
            "cache" => fs::create_dir_all(adversarial_home.join(".cache/glab")).unwrap(),
            "cache-mise" => fs::create_dir_all(adversarial_home.join(".cache/mise")).unwrap(),
            "cache-file" => {
                fs::create_dir_all(adversarial_home.join(".cache")).unwrap();
                fs::write(adversarial_home.join(".cache/pi"), "unmanaged").unwrap();
            }
            "cache-link" => {
                fs::create_dir_all(adversarial_home.join(".cache")).unwrap();
                symlink(temp.path(), adversarial_home.join(".cache/gh")).unwrap();
            }
            _ => unreachable!(),
        }
        let rejected = Command::new(&script)
            .env("HOME", &adversarial_home)
            .status()
            .unwrap();
        assert!(!rejected.success(), "accepted unmanaged {case} destination");
        assert!(
            !adversarial_home.join(".claude").exists()
                && !adversarial_home
                    .join(".config/gascan/agents/claude")
                    .exists(),
            "preflight rejection for {case} left partial Claude state"
        );
    }
}

#[test]
fn workstation_home_configuration_contains_no_credentials() {
    let script =
        fs::read_to_string(root().join("images/workspace/bin/configure-workstation-home")).unwrap();
    for forbidden in ["token=", "TOKEN=", "api_key", "API_KEY", "credential"] {
        assert!(
            !script.contains(forbidden),
            "home setup must not materialize credentials: {forbidden}"
        );
    }
}

#[test]
fn reviewed_workstation_packages_require_no_extra_privileges() {
    let system_tools = fs::read_to_string(root().join("tests/image/system-tools.txt")).unwrap();
    let packages = system_tools
        .lines()
        .collect::<std::collections::BTreeSet<_>>();
    for forbidden_package in [
        "libcap2-bin",
        "nmap",
        "tcpdump",
        "tshark",
        "wireshark",
        "wireshark-common",
    ] {
        assert!(
            !packages.contains(forbidden_package),
            "unreviewed capability or packet-capture package: {forbidden_package}"
        );
    }

    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    for forbidden in [
        "CAP_NET_ADMIN",
        "CAP_NET_RAW",
        "--cap-add",
        "--device",
        "--privileged",
        "/dev/net/tun",
    ] {
        assert!(
            !dockerfile.contains(forbidden),
            "Dockerfile adds forbidden privilege or device access: {forbidden}"
        );
    }
}

#[test]
fn dockerfile_declares_workspace_user_init_and_persistent_layout() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    let system_tools = fs::read_to_string(root().join("tests/image/system-tools.txt")).unwrap();
    for required in ["sudo", "tini"] {
        assert!(
            system_tools.lines().any(|package| package == required),
            "missing image package: {required}"
        );
    }
    for required in [
        "COPY --chmod=0440 images/workspace/etc/sudoers.d/workspace /etc/sudoers.d/workspace",
        "COPY --chmod=0555 images/workspace/bin/migrate-workspace-identity /usr/local/bin/migrate-workspace-identity",
        "COPY --chmod=0555 images/workspace/bin/initialize-rust-home /usr/local/bin/initialize-rust-home",
        "COPY --chmod=0555 images/workspace/libexec/migrate-workspace-identity-core /usr/local/libexec/gascan/migrate-workspace-identity-core",
        "/usr/local/bin/migrate-workspace-identity",
        "chown workspace:workspace /opt/gascan/mise",
        "/opt/gascan/mise",
        "/home/workspace/.cache",
        "/home/workspace/.local/state",
        "/home/workspace/.config/gascan",
        "visudo -cf /etc/sudoers.d/workspace",
        "USER workspace:workspace",
        "WORKDIR /workspace",
        "ENTRYPOINT [\"/usr/local/bin/gascan-entrypoint\"]",
        "VOLUME [\"/home/workspace/.local/share/mise\", \"/home/workspace/.cache\", \"/home/workspace/.config/gascan\"]",
    ] {
        assert!(
            dockerfile.contains(required),
            "missing image contract: {required}"
        );
    }
    assert!(
        !dockerfile.contains(
            "ENTRYPOINT [\"/usr/bin/tini\", \"--\", \"/usr/local/bin/gascan-entrypoint\"]"
        ),
        "Apple --init must be the sole init boundary"
    );
}

#[test]
fn identity_migration_is_exact_and_fail_closed() {
    let wrapper =
        fs::read_to_string(root().join("images/workspace/bin/migrate-workspace-identity")).unwrap();
    let migration =
        fs::read_to_string(root().join("images/workspace/libexec/migrate-workspace-identity-core"))
            .unwrap();

    assert!(wrapper.contains("/etc/passwd /etc/group /home"));
    assert!(wrapper.contains("/usr/sbin/usermod /usr/sbin/groupmod /usr/bin/stat"));

    for required in [
        "ubuntu:x:1000:1000:Ubuntu:$old_home:/bin/bash",
        "ubuntu:x:1000:",
        "--login workspace --home \"$new_home\" --move-home ubuntu",
        "--new-name workspace ubuntu",
        "workspace:x:1000:1000:Ubuntu:$new_home:/bin/bash",
        "workspace:x:1000:",
        "test ! -e \"$old_home\"",
    ] {
        assert!(
            migration.contains(required),
            "missing exact identity contract: {required}"
        );
    }
    for forbidden in ["--non-unique", "userdel", "groupdel", "useradd", "groupadd"] {
        assert!(
            !migration.contains(forbidden),
            "unsafe identity migration: {forbidden}"
        );
    }
}

#[test]
fn identity_migration_executes_exact_transition_and_rejects_before_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let bin = temp.path().join("bin");
    fs::create_dir_all(home.join("ubuntu")).unwrap();
    fs::create_dir(&bin).unwrap();
    let passwd = temp.path().join("passwd");
    let group = temp.path().join("group");
    fs::write(
        &passwd,
        format!(
            "ubuntu:x:1000:1000:Ubuntu:{}/ubuntu:/bin/bash\n",
            home.display()
        ),
    )
    .unwrap();
    fs::write(&group, "ubuntu:x:1000:\n").unwrap();
    let calls = temp.path().join("calls");
    let fake = |name: &str, body: &str| {
        let path = bin.join(name);
        fs::write(&path, body).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    };
    fake("stat", "#!/bin/sh\nprintf 'directory:1000:1000\n'\n");
    fake(
        "usermod",
        "#!/bin/sh\nprintf 'usermod\n' >>\"$CALLS\"\ntest \"${BAD_POST:-0}\" = 0 || exit 0\nsed 's/^ubuntu:/workspace:/; s#/ubuntu:/bin/bash#/workspace:/bin/bash#' \"$PASSWD\" >\"$PASSWD.new\"\nmv \"$PASSWD.new\" \"$PASSWD\"\nmv \"$HOME_ROOT/ubuntu\" \"$HOME_ROOT/workspace\"\n",
    );
    fake(
        "groupmod",
        "#!/bin/sh\nprintf 'groupmod\n' >>\"$CALLS\"\ntest \"${BAD_POST:-0}\" = 0 || exit 0\nsed 's/^ubuntu:/workspace:/' \"$GROUP\" >\"$GROUP.new\"\nmv \"$GROUP.new\" \"$GROUP\"\n",
    );

    let run = || {
        Command::new("bash")
            .arg(root().join("images/workspace/libexec/migrate-workspace-identity-core"))
            .args([
                &passwd,
                &group,
                &home,
                &bin.join("usermod"),
                &bin.join("groupmod"),
                &bin.join("stat"),
            ])
            .env("CALLS", &calls)
            .env("PASSWD", &passwd)
            .env("GROUP", &group)
            .env("HOME_ROOT", &home)
            .status()
            .unwrap()
    };
    let bad_post = Command::new("bash")
        .arg(root().join("images/workspace/libexec/migrate-workspace-identity-core"))
        .args([
            &passwd,
            &group,
            &home,
            &bin.join("usermod"),
            &bin.join("groupmod"),
            &bin.join("stat"),
        ])
        .env("CALLS", &calls)
        .env("PASSWD", &passwd)
        .env("GROUP", &group)
        .env("HOME_ROOT", &home)
        .env("BAD_POST", "1")
        .status()
        .unwrap();
    assert!(!bad_post.success(), "invalid post-mutation state passed");
    assert_eq!(fs::read_to_string(&calls).unwrap(), "usermod\ngroupmod\n");
    fs::remove_file(&calls).unwrap();

    assert!(run().success());
    assert_eq!(fs::read_to_string(&calls).unwrap(), "usermod\ngroupmod\n");

    fs::remove_file(&calls).unwrap();
    fs::write(
        &passwd,
        format!(
            "ubuntu:x:1001:1000:Ubuntu:{}/ubuntu:/bin/bash\n",
            home.display()
        ),
    )
    .unwrap();
    assert!(!run().success());
    assert!(!calls.exists(), "prevalidation failure invoked mutation");
}

#[test]
fn identity_migration_prevalidation_rejects_unsafe_fixtures_without_mutation() {
    use std::os::unix::fs::symlink;

    for case in [
        "passwd-fields",
        "group-fields",
        "duplicate-uid",
        "duplicate-gid",
        "workspace-user",
        "workspace-group",
        "missing-home",
        "symlink-home",
        "file-home",
        "wrong-owner",
        "destination-exists",
        "destination-link",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let bin = temp.path().join("bin");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir(&bin).unwrap();
        let passwd = temp.path().join("passwd");
        let group = temp.path().join("group");
        let mut passwd_text = format!(
            "ubuntu:x:1000:1000:Ubuntu:{}/ubuntu:/bin/bash\n",
            home.display()
        );
        let mut group_text = "ubuntu:x:1000:\n".to_string();
        match case {
            "passwd-fields" => passwd_text = passwd_text.replace("Ubuntu:", "Wrong:"),
            "group-fields" => group_text = "ubuntu:x:1000:member\n".into(),
            "duplicate-uid" => passwd_text.push_str("alias:x:1000:2000::/tmp:/bin/false\n"),
            "duplicate-gid" => group_text.push_str("alias:x:1000:\n"),
            "workspace-user" => passwd_text.push_str("workspace:x:2000:2000::/tmp:/bin/false\n"),
            "workspace-group" => group_text.push_str("workspace:x:2000:\n"),
            _ => {}
        }
        fs::write(&passwd, passwd_text).unwrap();
        fs::write(&group, group_text).unwrap();
        match case {
            "missing-home" => {}
            "symlink-home" => symlink(temp.path(), home.join("ubuntu")).unwrap(),
            "file-home" => fs::write(home.join("ubuntu"), "not a directory").unwrap(),
            _ => fs::create_dir(home.join("ubuntu")).unwrap(),
        }
        match case {
            "destination-exists" => fs::create_dir(home.join("workspace")).unwrap(),
            "destination-link" => symlink(temp.path(), home.join("workspace")).unwrap(),
            _ => {}
        }
        let calls = temp.path().join("calls");
        for command in ["usermod", "groupmod"] {
            let path = bin.join(command);
            fs::write(&path, "#!/bin/sh\ntouch \"$CALLS\"\nexit 99\n").unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let stat = bin.join("stat");
        let value = if case == "wrong-owner" {
            "directory:501:20"
        } else {
            "directory:1000:1000"
        };
        fs::write(&stat, format!("#!/bin/sh\nprintf '%s\\n' '{value}'\n")).unwrap();
        fs::set_permissions(&stat, fs::Permissions::from_mode(0o755)).unwrap();
        let status = Command::new("bash")
            .arg(root().join("images/workspace/libexec/migrate-workspace-identity-core"))
            .args([
                &passwd,
                &group,
                &home,
                &bin.join("usermod"),
                &bin.join("groupmod"),
                &stat,
            ])
            .env("CALLS", &calls)
            .status()
            .unwrap();
        assert!(!status.success(), "unsafe fixture passed: {case}");
        assert!(!calls.exists(), "unsafe fixture mutated identity: {case}");
    }
}

#[test]
fn sudoers_and_entrypoint_are_exact_and_non_bootstrapping() {
    let sudoers = root().join("images/workspace/etc/sudoers.d/workspace");
    assert_eq!(
        fs::read_to_string(&sudoers).unwrap(),
        "workspace ALL=(ALL:ALL) NOPASSWD: ALL\n"
    );

    let entrypoint =
        fs::read_to_string(root().join("images/workspace/bin/gascan-entrypoint")).unwrap();
    assert!(entrypoint.contains("exec \"$@\""));
    assert!(entrypoint.contains("exec sleep infinity"));
    for forbidden in [
        "curl",
        "wget",
        "http://",
        "https://",
        "mise install",
        "git clone",
    ] {
        assert!(
            !entrypoint.contains(forbidden),
            "entrypoint contains bootstrap behavior: {forbidden}"
        );
    }
}

#[test]
fn ssh_entrypoint_preserves_commands_and_has_fail_closed_default_dispatch() {
    let entrypoint_path = root().join("images/workspace/bin/gascan-entrypoint");
    let temporary = tempfile::tempdir().unwrap();
    let marker = temporary.path().join("marker");
    let command = temporary.path().join("explicit-command");
    fs::write(
        &command,
        "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$*\" >\"$MARKER\"\n",
    )
    .unwrap();
    fs::set_permissions(&command, fs::Permissions::from_mode(0o755)).unwrap();
    let status = Command::new(&entrypoint_path)
        .arg(&command)
        .args(["one", "two words"])
        .env("MARKER", &marker)
        .env("GASCAN_SSH_ENABLED", "1")
        .env("GASCAN_SSH_AUTHORIZED_KEY", "not-a-key")
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(fs::read_to_string(&marker).unwrap(), "one two words\n");

    let sleep = temporary.path().join("sleep");
    fs::write(
        &sleep,
        "#!/bin/sh\nset -eu\nprintf 'sleep:%s\\n' \"$*\" >\"$MARKER\"\n",
    )
    .unwrap();
    fs::set_permissions(&sleep, fs::Permissions::from_mode(0o755)).unwrap();
    let disabled = Command::new(&entrypoint_path)
        .env("MARKER", &marker)
        .env("PATH", temporary.path())
        .env("GASCAN_SSH_ENABLED", "0")
        .status()
        .unwrap();
    assert!(disabled.success());
    assert_eq!(fs::read_to_string(&marker).unwrap(), "sleep:infinity\n");

    let invalid = Command::new(&entrypoint_path)
        .env("GASCAN_SSH_ENABLED", "yes")
        .status()
        .unwrap();
    assert!(!invalid.success());

    let entrypoint = fs::read_to_string(&entrypoint_path).unwrap();
    assert!(
        entrypoint.find("exec \"$@\"").unwrap() < entrypoint.find("GASCAN_SSH_ENABLED").unwrap(),
        "explicit image commands must dispatch before managed SSH startup"
    );
    for required in [
        "GASCAN_SSH_ENABLED:-0",
        "0)",
        "1)",
        "--preserve-env=GASCAN_SSH_AUTHORIZED_KEY",
        "/usr/local/bin/start-gascan-sshd",
        "exec sleep infinity",
    ] {
        assert!(
            entrypoint.contains(required),
            "missing SSH dispatch: {required}"
        );
    }
}

#[test]
fn smoke_fixture_uses_built_ref_and_checks_signal_and_zombies() {
    let smoke = fs::read_to_string(root().join("tests/image/user-and-volumes.sh")).unwrap();
    for required in [
        ".artifacts/workspace-image-ref",
        "\"$container_bin\" create",
        "--init",
        "--label dev.gascan.test=true",
        "dev.gascan.test.owner=$owner_token",
        "--mount \"type=bind,source=$root,target=/workspace\"",
        "--bin validate-owned-container",
        "\"$container_bin\" start",
        "\"$container_bin\" exec",
        "/proc/[0-9]*/status",
        "bounded_container stop --time 5",
        "test \"$elapsed\" -le 5",
    ] {
        assert!(
            smoke.contains(required),
            "missing live smoke contract: {required}"
        );
    }
    assert_eq!(smoke.matches("--mount ").count(), 1);
    assert!(!smoke.contains("container run"));
}

#[test]
fn smoke_fixture_restarts_and_rechecks_the_process_contract_after_stop() {
    let smoke = fs::read_to_string(root().join("tests/image/user-and-volumes.sh")).unwrap();
    assert_eq!(
        smoke.matches("\"$container_bin\" start \"$name\"").count(),
        2,
        "live smoke must exercise initial start and restart"
    );
    assert_eq!(
        smoke
            .matches(
                "\"$container_bin\" exec \"$name\" bash /workspace/tests/image/user-and-volumes.sh --inside",
            )
            .count(),
        2,
        "live smoke must recheck identity, signal, and zombie contracts after restart"
    );
}

#[test]
fn every_live_image_smoke_models_the_runtime_init_boundary() {
    for fixture in [
        "user-and-volumes.sh",
        "polyglot-smoke.sh",
        "gascamp-smoke.sh",
        "workstation-smoke.sh",
    ] {
        let smoke = fs::read_to_string(root().join("tests/image").join(fixture)).unwrap();
        assert!(
            smoke.contains("--init"),
            "{fixture} does not model the production Apple runtime init boundary"
        );
    }
}

#[test]
fn gascamp_smoke_fails_closed_without_a_built_image_reference() {
    let missing = root().join(".artifacts/definitely-missing-gascamp-image-ref");
    let output = Command::new("bash")
        .arg(root().join("tests/image/gascamp-smoke.sh"))
        .env("GASCAN_IMAGE_REF_FILE", &missing)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        format!("missing Gascamp image reference: {}\n", missing.display())
    );
}
