use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

const TOKEN: &str = "00112233445566778899aabbccddeeff";
const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}
fn executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

struct Fixture {
    temp: tempfile::TempDir,
    root: PathBuf,
    calls: PathBuf,
    command: Command,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    for directory in [
        "scripts",
        "tests/image",
        "images/workspace",
        "docs/evidence",
        ".artifacts/connected-workspace-context",
    ] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }
    fs::write(
        root.join("images/workspace/versions.lock"),
        "workspace_build_mode = \"connected\"\nworkspace_tag = \"gascan-workspace:test\"\n",
    )
    .unwrap();
    fs::write(
        root.join(".artifacts/connected-workspace-context/context-manifest.tsv"),
        "context\n",
    )
    .unwrap();
    for cargo_file in ["Cargo.toml", "Cargo.lock"] {
        std::os::unix::fs::symlink(
            repository_root().join("scripts").join(cargo_file),
            root.join("scripts").join(cargo_file),
        )
        .unwrap();
    }
    std::os::unix::fs::symlink(
        repository_root().join("scripts/src"),
        root.join("scripts/src"),
    )
    .unwrap();
    fs::copy(
        repository_root().join("images/workspace/Dockerfile"),
        root.join("images/workspace/Dockerfile"),
    )
    .unwrap();
    fs::create_dir_all(root.join("images/workspace/bin")).unwrap();
    fs::copy(
        repository_root().join("images/workspace/bin/select-gascamp"),
        root.join("images/workspace/bin/select-gascamp"),
    )
    .unwrap();
    let calls = temp.path().join("calls");
    executable(
        &root.join("scripts/prefetch-connected-workspace-image.sh"),
        "#!/bin/sh\nset -eu\nprintf 'prefetch\\n' >>\"$CALLS\"\n",
    );
    let helper = temp.path().join("snapshot-helper");
    executable(&helper, "#!/bin/sh\nexit 0\n");
    let helper_identity = temp.path().join("snapshot-helper-identity");
    executable(
        &helper_identity,
        "#!/bin/sh\nset -eu\nprintf 'helper-identity\\n' >>\"$CALLS\"\n[ \"${HELPER_IDENTITY_UNSAFE:-}\" != 1 ]\nprintf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\\t1\\t2\\n'\n",
    );
    executable(
        &root.join("scripts/build-connected-workspace-image.sh"),
        &format!(
            "#!/bin/sh\nset -eu\nprintf 'build\\n' >>\"$CALLS\"\n[ \"${{GASCAN_GATE_TEST_BUILD_FAILURE:-}}\" != 1 ]\nmkdir -p \"$GASCAN_GATE_ARTIFACTS\"\nref='gascan-workspace:test@sha256:{DIGEST}'\n[ \"${{REFERENCE_KIND:-}}\" != mutable ] || ref=gascan-workspace:test\nprintf '%s\\n' \"$ref\" >\"$GASCAN_GATE_ARTIFACTS/workspace-image-ref\"\nprintf '{{\"reference\":\"%s\",\"tag\":\"gascan-workspace:test\",\"platform\":\"linux/arm64\",\"image_digest\":\"sha256:{DIGEST}\",\"status\":\"succeeded\"}}\\n' \"$ref\" >\"$GASCAN_GATE_ARTIFACTS/workspace-image-build.json\"\ncase \"${{RECEIPT_KIND:-}}\" in missing) rm -f \"$GASCAN_GATE_ARTIFACTS/workspace-image-build.json\" ;; malformed) printf '{{bad\\n' >\"$GASCAN_GATE_ARTIFACTS/workspace-image-build.json\" ;; mismatched) printf '{{\"reference\":\"wrong\"}}\\n' >\"$GASCAN_GATE_ARTIFACTS/workspace-image-build.json\" ;; esac\nprintf '%s\\n' \"$ref\"\n"
        ),
    );
    executable(
        &root.join("scripts/validate-connected-image-receipt.sh"),
        "#!/bin/sh\nset -eu\n[ \"${GASCAN_GATE_TEST_RECEIPT_FAILURE:-}\" != 1 ]\nref=$(cat \"$1\")\nreceipt=${2:-$(dirname \"$1\")/workspace-image-build.json}\n[ -f \"$receipt\" ]\ncase \"$ref\" in gascan-workspace:test@sha256:????????????????????????????????????????????????????????????????) ;; *) exit 1;; esac\ngrep -Fq \"\\\"reference\\\":\\\"$ref\\\"\" \"$receipt\"\nprintf '%s\\n' \"$ref\"\n",
    );
    for smoke in [
        "user-and-volumes.sh",
        "polyglot-smoke.sh",
        "gascamp-smoke.sh",
        "container-cli.sh",
    ] {
        fs::copy(
            repository_root().join("tests/image").join(smoke),
            root.join("tests/image").join(smoke),
        )
        .unwrap();
    }
    let raw_container = temp.path().join("container-raw");
    executable(
        &raw_container,
        &format!(
            "#!/bin/sh\nset -eu\nprintf 'container:%s\\n' \"$*\" >>\"$CALLS\"\nif [ \"$1 ${{2:-}}\" = 'image inspect' ]; then [ $# -eq 3 ] || exit 93; platform=${{IMAGE_PLATFORM:-arm64}}; printf '[{{\"id\":\"{DIGEST}\",\"configuration\":{{\"name\":\"gascan-workspace:test\",\"descriptor\":{{\"digest\":\"sha256:{DIGEST}\"}}}},\"variants\":[{{\"platform\":{{\"os\":\"linux\",\"architecture\":\"%s\"}},\"digest\":\"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\"}}]}}]\\n' \"$platform\"; exit 0; fi\ncase \"$1\" in create) while [ $# -gt 0 ]; do [ \"$1\" = --name ] && {{ touch \"$STATE/$2\"; break; }}; shift; done ;; inspect) name=$2; [ \"${{RESIDUE:-}}\" = \"$name\" ] || [ -f \"$STATE/$name\" ] || exit 1; count_file=\"$STATE/.inspect-$name\"; count=0; [ ! -f \"$count_file\" ] || count=$(cat \"$count_file\"); count=$((count+1)); printf '%s' \"$count\" >\"$count_file\"; owner=$OWNER; [ \"${{FOREIGN:-}}\" = \"$name\" ] && owner=ffffffffffffffffffffffffffffffff; [ \"${{REPLACE_ON_SECOND_INSPECT:-}}\" = \"$name\" ] && [ \"$count\" -ge 2 ] && owner=ffffffffffffffffffffffffffffffff; printf '[{{\"id\":\"%s\",\"configuration\":{{\"id\":\"%s\",\"labels\":{{\"dev.gascan.test\":\"true\",\"dev.gascan.test.owner\":\"%s\"}}}}}}]\\n' \"$name\" \"$name\" \"$owner\" ;; stop) : ;; delete) name=${{@:$#}}; [ \"${{FAIL_DELETE:-}}\" != \"$name\" ] || exit 1; rm -f \"$STATE/$name\" ;; esac\n"
        ),
    );
    let container = temp.path().join("container");
    executable(
        &container,
        "#!/bin/sh\nset -eu\nif [ \"$1\" = list ]; then first=true; printf '['; for name in gascan-image-user-test-$OWNER gascan-image-polyglot-test-$OWNER gascan-image-gascamp-test-$OWNER; do if [ \"${RESIDUE:-}\" = \"$name\" ] || [ -f \"$STATE/$name\" ]; then $first || printf ','; first=false; printf '{\"id\":\"%s\",\"configuration\":{\"id\":\"%s\",\"labels\":{}}}' \"$name\" \"$name\"; fi; done; printf ']\\n'; exit 0; fi\nexec \"$RAW_CONTAINER\" \"$@\"\n",
    );
    let state = temp.path().join("state");
    fs::create_dir(&state).unwrap();
    fs::write(state.join("unrelated-resource"), "foreign").unwrap();
    let mut command = Command::new("bash");
    command
        .arg(repository_root().join("scripts/run-connected-image-gate.sh"))
        .env("GASCAN_GATE_TEST_ROOT", &root)
        .env("GASCAN_GATE_ARTIFACTS", root.join(".artifacts"))
        .env("GASCAN_TEST_OWNER_TOKEN", TOKEN)
        .env("CONTAINER_BIN", &container)
        .env("CALLS", &calls)
        .env("STATE", &state)
        .env("OWNER", TOKEN)
        .env("RAW_CONTAINER", &raw_container)
        .env("GASCAN_GATE_TEST_SNAPSHOT_HELPER", &helper)
        .env("GASCAN_GATE_TEST_HELPER_IDENTITY_BIN", &helper_identity)
        .env("CARGO_TARGET_DIR", repository_root().join("scripts/target"));
    Fixture {
        temp,
        root,
        calls,
        command,
    }
}

#[test]
fn missing_or_unsafe_snapshot_helper_fails_before_prefetch_or_container_activity() {
    for mode in ["missing", "unsafe"] {
        let mut f = fixture();
        if mode == "missing" {
            fs::remove_file(f.temp.path().join("snapshot-helper")).unwrap();
        } else {
            f.command.env("HELPER_IDENTITY_UNSAFE", "1");
        }
        assert!(!f.command.status().unwrap().success(), "{mode}");
        let calls = fs::read_to_string(&f.calls).unwrap_or_default();
        if mode == "missing" {
            assert!(calls.is_empty());
        } else {
            assert_eq!(calls, "helper-identity\n");
        }
        assert!(!f
            .root
            .join("docs/evidence/connected-workspace-image.md")
            .exists());
        assert!(!f.root.join("images/workspace/approved-image.txt").exists());
    }
}

#[test]
fn successful_gate_uses_one_reference_and_token_then_publishes_atomically() {
    let mut f = fixture();
    let output = f.command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = fs::read_to_string(&f.calls).unwrap();
    assert!(calls.find("prefetch").unwrap() < calls.find("build").unwrap());
    for prefix in ["user", "polyglot", "gascamp"] {
        assert!(calls.contains(&format!("inspect gascan-image-{prefix}-test-{TOKEN}")));
    }
    for prefix in ["user", "polyglot", "gascamp"] {
        let name = format!("gascan-image-{prefix}-test-{TOKEN}");
        let create = calls.find(&format!("create --name {name} ")).unwrap();
        let stop = calls.find(&format!("stop --time 5 {name}")).unwrap();
        let delete = calls.find(&format!("delete {name}")).unwrap();
        assert!(create < stop && stop < delete);
        assert!(calls.contains(&format!(
            "container:inspect {name}\ncontainer:stop --time 5 {name}\n"
        )));
        assert!(calls.contains(&format!(
            "container:inspect {name}\ncontainer:delete {name}\n"
        )));
    }
    let lines: Vec<_> = calls.lines().collect();
    for (index, line) in lines.iter().enumerate().filter(|(_, line)| {
        line.starts_with("container:stop ") || line.starts_with("container:delete ")
    }) {
        let name = line.split_whitespace().last().unwrap();
        assert!(
            index > 0 && lines[index - 1] == format!("container:inspect {name}"),
            "mutation lacked immediately preceding structural inspect: {line}"
        );
    }
    let evidence =
        fs::read_to_string(f.root.join("docs/evidence/connected-workspace-image.md")).unwrap();
    assert!(evidence.contains(&format!("gascan-workspace:test@sha256:{DIGEST}")));
    assert!(evidence.contains("platform: `linux/arm64`"));
    assert_eq!(
        fs::read(f.root.join("images/workspace/approved-image.txt")).unwrap(),
        format!("gascan-workspace:test@sha256:{DIGEST}").as_bytes()
    );
    assert_eq!(
        fs::read(f.temp.path().join("state/unrelated-resource")).unwrap(),
        b"foreign"
    );
}

#[test]
fn injected_random_source_proves_fresh_live_tokens_across_runs() {
    for (index, token) in [
        "11111111111111111111111111111111",
        "22222222222222222222222222222222",
    ]
    .into_iter()
    .enumerate()
    {
        let mut f = fixture();
        let random = f.temp.path().join(format!("random-{index}"));
        executable(&random, &format!("#!/bin/sh\nprintf '%s\\n' '{token}'\n"));
        f.command
            .env_remove("GASCAN_TEST_OWNER_TOKEN")
            .env("GASCAN_GATE_RANDOM_BIN", &random)
            .env("OWNER", token);
        assert!(f.command.status().unwrap().success());
        let calls = fs::read_to_string(&f.calls).unwrap();
        assert!(calls.contains(&format!("gascan-image-user-test-{token}")));
    }
}

#[test]
fn every_failure_prevents_both_publications() {
    for failure in ["build", "receipt", "smoke", "residue"] {
        let mut f = fixture();
        match failure {
            "build" => {
                f.command.env("GASCAN_GATE_TEST_BUILD_FAILURE", "1");
            }
            "receipt" => {
                f.command.env("GASCAN_GATE_TEST_RECEIPT_FAILURE", "1");
            }
            "smoke" => {
                let wrapper = f.temp.path().join("failing-container");
                executable(
                    &wrapper,
                    "#!/bin/sh\ncase \"$*\" in *polyglot-smoke.sh*) exit 1 ;; esac\nexec \"$REAL_CONTAINER\" \"$@\"\n",
                );
                f.command
                    .env("CONTAINER_BIN", wrapper)
                    .env("REAL_CONTAINER", f.temp.path().join("container"));
            }
            "residue" => {
                f.command
                    .env("RESIDUE", format!("gascan-image-user-test-{TOKEN}"));
            }
            _ => unreachable!(),
        };
        assert!(!f.command.status().unwrap().success(), "{failure}");
        assert!(!f
            .root
            .join("docs/evidence/connected-workspace-image.md")
            .exists());
        assert!(!f.root.join("images/workspace/approved-image.txt").exists());
    }
}

#[test]
fn stale_pass_pair_is_retired_before_work_and_owner_token_is_never_evidence() {
    let mut f = fixture();
    fs::write(
        f.root.join("docs/evidence/connected-workspace-image.md"),
        "status: `PASS`\n",
    )
    .unwrap();
    fs::write(f.root.join("images/workspace/approved-image.txt"), "stale").unwrap();
    f.command.env("GASCAN_GATE_TEST_BUILD_FAILURE", "1");
    assert!(!f.command.status().unwrap().success());
    assert!(!f
        .root
        .join("docs/evidence/connected-workspace-image.md")
        .exists());
    assert!(!f.root.join("images/workspace/approved-image.txt").exists());

    let mut f = fixture();
    assert!(f.command.status().unwrap().success());
    let evidence =
        fs::read_to_string(f.root.join("docs/evidence/connected-workspace-image.md")).unwrap();
    assert!(!evidence.contains(TOKEN));
    assert!(!evidence.to_ascii_lowercase().contains("owner token"));
}

#[test]
fn stale_pass_pair_is_retired_when_obsolete_credential_input_is_rejected() {
    let mut f = fixture();
    fs::write(
        f.root.join("docs/evidence/connected-workspace-image.md"),
        "status: `PASS`\n",
    )
    .unwrap();
    fs::write(f.root.join("images/workspace/approved-image.txt"), "stale").unwrap();
    f.command
        .env("GASCAMP_READ_TOKEN_FILE", "/tmp/obsolete-token");
    assert!(!f.command.status().unwrap().success());
    assert!(!f
        .root
        .join("docs/evidence/connected-workspace-image.md")
        .exists());
    assert!(!f.root.join("images/workspace/approved-image.txt").exists());
    assert!(!f.calls.exists());
}

#[test]
fn every_publication_boundary_rolls_back_the_pair() {
    for boundary in ["after-stage", "after-evidence"] {
        for action in ["FAIL", "INT", "TERM"] {
            let mut f = fixture();
            f.command
                .env("GASCAN_GATE_TEST_PUBLICATION_BOUNDARY", boundary)
                .env("GASCAN_GATE_TEST_PUBLICATION_ACTION", action);
            let status = f.command.status().unwrap();
            assert!(!status.success(), "{boundary}/{action}");
            if action == "INT" {
                assert_eq!(status.code(), Some(130));
            }
            if action == "TERM" {
                assert_eq!(status.code(), Some(143));
            }
            assert!(!f
                .root
                .join("docs/evidence/connected-workspace-image.md")
                .exists());
            assert!(!f.root.join("images/workspace/approved-image.txt").exists());
            assert_eq!(
                fs::read_dir(f.root.join("docs/evidence")).unwrap().count(),
                0
            );
            assert!(!fs::read_dir(f.root.join("images/workspace"))
                .unwrap()
                .any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".approved-image.")));
        }
    }
}

#[test]
fn gate_rejects_obsolete_credential_input_before_work() {
    for (name, value) in [
        ("GASCAMP_READ_TOKEN_FILE", "/tmp/obsolete-token"),
        ("GITHUB_TOKEN", "obsolete-token"),
        ("DOCKER_AUTH_CONFIG", "{}"),
        ("CUSTOM_BUILD_CREDENTIAL", "obsolete-credential"),
    ] {
        let mut f = fixture();
        f.command.env(name, value);
        assert!(!f.command.status().unwrap().success(), "{name}");
        assert!(!f.calls.exists(), "{name} reached connected work");
    }
}

#[test]
fn malformed_missing_mismatched_mutable_and_wrong_platform_are_fail_closed() {
    for (variable, value) in [
        ("RECEIPT_KIND", "missing"),
        ("RECEIPT_KIND", "malformed"),
        ("RECEIPT_KIND", "mismatched"),
        ("REFERENCE_KIND", "mutable"),
        ("IMAGE_PLATFORM", "amd64"),
    ] {
        let mut f = fixture();
        f.command.env(variable, value);
        assert!(!f.command.status().unwrap().success(), "{variable}={value}");
        assert!(!f
            .root
            .join("docs/evidence/connected-workspace-image.md")
            .exists());
        assert!(!f.root.join("images/workspace/approved-image.txt").exists());
    }
}

#[test]
fn foreign_replacement_between_checks_is_never_mutated() {
    let mut f = fixture();
    let name = format!("gascan-image-gascamp-test-{TOKEN}");
    fs::write(f.temp.path().join("state").join(&name), "").unwrap();
    f.command
        .env("REPLACE_ON_SECOND_INSPECT", &name)
        .env("FAIL_SMOKE", "user-and-volumes.sh");
    assert!(!f.command.status().unwrap().success());
    let calls = fs::read_to_string(&f.calls).unwrap();
    assert!(calls.matches(&format!("container:inspect {name}")).count() >= 2);
    assert!(!calls.contains(&format!("container:stop --time 5 {name}")));
    assert!(!calls.contains(&format!("container:delete {name}")));
}

#[test]
fn cleanup_validates_ownership_before_mutation_and_leaves_foreign_resource() {
    let mut f = fixture();
    let name = format!("gascan-image-gascamp-test-{TOKEN}");
    fs::write(f.temp.path().join("state").join(&name), "").unwrap();
    f.command
        .env("FOREIGN", &name)
        .env("FAIL_SMOKE", "user-and-volumes.sh");
    assert!(!f.command.status().unwrap().success());
    let calls = fs::read_to_string(&f.calls).unwrap();
    assert!(calls.contains(&format!("inspect {name}")));
    assert!(!calls.contains(&format!("stop --time 5 {name}")));
    assert!(!calls.contains(&format!("delete {name}")));
}

#[test]
fn int_and_term_exit_nonzero_after_bounded_cleanup() {
    for (signal, code) in [("INT", 130), ("TERM", 143)] {
        let mut f = fixture();
        f.command.env("GASCAN_GATE_TEST_SIGNAL", signal);
        let status = f.command.status().unwrap();
        assert_eq!(status.code(), Some(code));
        let calls = fs::read_to_string(&f.calls).unwrap();
        assert!(calls.contains("stop --time 5"));
        assert!(calls.contains("delete gascan-image-user-test-"));
        assert!(!f
            .root
            .join("docs/evidence/connected-workspace-image.md")
            .exists());
        assert!(!f.root.join("images/workspace/approved-image.txt").exists());
        assert!(!f
            .temp
            .path()
            .join("state")
            .join(format!("gascan-image-user-test-{TOKEN}"))
            .exists());
    }
}

#[test]
fn cleanup_failure_is_nonzero_and_never_publishes() {
    let mut f = fixture();
    f.command
        .env("GASCAN_GATE_TEST_SIGNAL", "TERM")
        .env("FAIL_DELETE", format!("gascan-image-user-test-{TOKEN}"));
    let status = f.command.status().unwrap();
    assert_eq!(status.code(), Some(1));
    assert!(!f
        .root
        .join("docs/evidence/connected-workspace-image.md")
        .exists());
    assert!(!f.root.join("images/workspace/approved-image.txt").exists());
}

#[test]
fn every_blocking_cleanup_cli_is_killed_reaped_and_fail_closed() {
    for blocked in ["inspect", "stop", "delete", "final"] {
        let mut f = fixture();
        let pids = f.temp.path().join("blocked-pids");
        let wrapper = f.temp.path().join("blocking-container");
        executable(
            &wrapper,
            "#!/bin/sh\nset -eu\nhang=false\ncase \"$HANG_COMMAND:$1\" in inspect:inspect|stop:stop|delete:delete|final:list) hang=true ;; esac\nif $hang; then printf '%s\\n' $$ >>\"$BLOCKED_PIDS\"; trap '' INT TERM; while :; do sleep 1; done; fi\nexec \"$REAL_CONTAINER\" \"$@\"\n",
        );
        f.command
            .env("CONTAINER_BIN", &wrapper)
            .env("REAL_CONTAINER", f.temp.path().join("container"))
            .env("GASCAN_GATE_TEST_SIGNAL", "TERM")
            .env("GASCAN_GATE_CLI_TIMEOUT_SECONDS", "1")
            .env("HANG_COMMAND", blocked)
            .env("BLOCKED_PIDS", &pids);
        let started = Instant::now();
        let mut child = f.command.spawn().unwrap();
        let deadline = started + Duration::from_secs(30);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                for pid in fs::read_to_string(&pids).unwrap_or_default().lines() {
                    let _ = Command::new("kill").args(["-KILL", pid]).status();
                }
                panic!("unbounded cleanup controller call: {blocked}");
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(!status.success());
        assert!(started.elapsed() < Duration::from_secs(30));
        let blocked_pids = fs::read_to_string(&pids).unwrap_or_default();
        assert!(
            !blocked_pids.is_empty(),
            "blocking path was not exercised: {blocked}"
        );
        for pid in blocked_pids
            .lines()
            .map(|line| line.parse::<i32>().unwrap())
        {
            assert!(
                !Command::new("kill")
                    .args(["-0", &pid.to_string()])
                    .status()
                    .unwrap()
                    .success(),
                "blocked child survived: {pid}"
            );
        }
        assert!(!f
            .root
            .join("docs/evidence/connected-workspace-image.md")
            .exists());
        assert!(!f.root.join("images/workspace/approved-image.txt").exists());
        assert!(!fs::read_dir(f.root.join("docs/evidence"))
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".connected-workspace-image.")));
    }
}

#[test]
fn real_smoke_cleanup_controller_hang_is_bounded_and_reaped() {
    let mut f = fixture();
    let pids = f.temp.path().join("blocked-pids");
    let wrapper = f.temp.path().join("blocking-container");
    executable(
        &wrapper,
        "#!/bin/sh\nset -eu\nif [ \"$1\" = stop ]; then printf '%s\\n' $$ >>\"$BLOCKED_PIDS\"; trap '' INT TERM; while :; do sleep 1; done; fi\nexec \"$REAL_CONTAINER\" \"$@\"\n",
    );
    f.command
        .env("CONTAINER_BIN", &wrapper)
        .env("REAL_CONTAINER", f.temp.path().join("container"))
        .env("GASCAN_IMAGE_CLI_TIMEOUT_SECONDS", "1")
        .env("GASCAN_GATE_CLI_TIMEOUT_SECONDS", "1")
        .env("BLOCKED_PIDS", &pids);
    let started = Instant::now();
    let mut child = f.command.spawn().unwrap();
    let deadline = started + Duration::from_secs(25);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            for pid in fs::read_to_string(&pids).unwrap_or_default().lines() {
                let _ = Command::new("kill").args(["-KILL", pid]).status();
            }
            panic!("real smoke cleanup was unbounded");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(!status.success());
    for pid in fs::read_to_string(&pids).unwrap().lines() {
        assert!(!Command::new("kill")
            .args(["-0", pid])
            .status()
            .unwrap()
            .success());
    }
    assert!(!f
        .root
        .join("docs/evidence/connected-workspace-image.md")
        .exists());
    assert!(!f.root.join("images/workspace/approved-image.txt").exists());
}

#[test]
fn inspect_failure_never_proves_absence_without_authoritative_inventory() {
    for inventory in ["present", "error", "malformed", "timeout"] {
        let mut f = fixture();
        let wrapper = f.temp.path().join("inventory-container");
        executable(
            &wrapper,
            "#!/bin/sh\nset -eu\nif [ \"$1\" = inspect ] && [ ! -f \"$STATE/$2\" ]; then exit 77; fi\nif [ \"$1\" = list ]; then case \"$INVENTORY_MODE\" in present) printf '[{\"configuration\":{\"name\":\"gascan-image-user-test-00112233445566778899aabbccddeeff\"}}]\\n' ;; error) exit 78 ;; malformed) printf '{bad\\n' ;; timeout) printf '%s\\n' $$ >>\"$BLOCKED_PIDS\"; trap '' INT TERM; while :; do sleep 1; done ;; esac; exit 0; fi\nexec \"$REAL_CONTAINER\" \"$@\"\n",
        );
        let pids = f.temp.path().join("blocked-pids");
        f.command
            .env("CONTAINER_BIN", wrapper)
            .env("REAL_CONTAINER", f.temp.path().join("container"))
            .env("INVENTORY_MODE", inventory)
            .env("BLOCKED_PIDS", &pids)
            .env("GASCAN_GATE_CLI_TIMEOUT_SECONDS", "1");
        let status = f.command.status().unwrap();
        assert!(!status.success(), "inventory={inventory}");
        assert!(!f
            .root
            .join("docs/evidence/connected-workspace-image.md")
            .exists());
        assert!(!f.root.join("images/workspace/approved-image.txt").exists());
    }
}

#[test]
fn inspect_failure_plus_parsed_inventory_absence_is_authoritative() {
    let mut f = fixture();
    let wrapper = f.temp.path().join("inventory-container");
    executable(
        &wrapper,
        "#!/bin/sh\nset -eu\nif [ \"$1\" = inspect ] && [ ! -f \"$STATE/$2\" ]; then exit 77; fi\nif [ \"$1\" = list ]; then printf '[]\\n'; exit 0; fi\nexec \"$REAL_CONTAINER\" \"$@\"\n",
    );
    f.command
        .env("CONTAINER_BIN", wrapper)
        .env("REAL_CONTAINER", f.temp.path().join("container"));
    assert!(f.command.status().unwrap().success());
}
