use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
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
        "COPY --chmod=0555 images/workspace/libexec/migrate-workspace-identity-core /usr/local/libexec/gascan/migrate-workspace-identity-core",
        "/usr/local/bin/migrate-workspace-identity",
        "chown workspace:workspace /opt/gascan/mise",
        "/opt/gascan/mise",
        "/home/workspace/.cache",
        "/home/workspace/.config/gascan",
        "visudo -cf /etc/sudoers.d/workspace",
        "USER workspace:workspace",
        "WORKDIR /workspace",
        "ENTRYPOINT [\"/usr/bin/tini\", \"--\", \"/usr/local/bin/gascan-entrypoint\"]",
        "VOLUME [\"/opt/gascan/mise\", \"/home/workspace/.cache\", \"/home/workspace/.config/gascan\"]",
    ] {
        assert!(
            dockerfile.contains(required),
            "missing image contract: {required}"
        );
    }
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
    fake("usermod", "#!/bin/sh\nprintf 'usermod\n' >>\"$CALLS\"\ntest \"${BAD_POST:-0}\" = 0 || exit 0\nsed 's/^ubuntu:/workspace:/; s#/ubuntu:/bin/bash#/workspace:/bin/bash#' \"$PASSWD\" >\"$PASSWD.new\"\nmv \"$PASSWD.new\" \"$PASSWD\"\nmv \"$HOME_ROOT/ubuntu\" \"$HOME_ROOT/workspace\"\n");
    fake("groupmod", "#!/bin/sh\nprintf 'groupmod\n' >>\"$CALLS\"\ntest \"${BAD_POST:-0}\" = 0 || exit 0\nsed 's/^ubuntu:/workspace:/' \"$GROUP\" >\"$GROUP.new\"\nmv \"$GROUP.new\" \"$GROUP\"\n");

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
fn smoke_fixture_uses_built_ref_and_checks_signal_and_zombies() {
    let smoke = fs::read_to_string(root().join("tests/image/user-and-volumes.sh")).unwrap();
    for required in [
        ".artifacts/workspace-image-ref",
        "\"$container_bin\" create",
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
