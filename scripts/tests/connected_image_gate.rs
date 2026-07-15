use std::{fs, os::unix::fs::PermissionsExt, path::{Path, PathBuf}, process::Command};

const TOKEN: &str = "00112233445566778899aabbccddeeff";
const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn repository_root() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf() }
fn executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

struct Fixture { temp: tempfile::TempDir, root: PathBuf, calls: PathBuf, command: Command }

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    for directory in ["scripts", "tests/image", "images/workspace", "docs/evidence", ".artifacts/connected-workspace-context"] { fs::create_dir_all(root.join(directory)).unwrap(); }
    fs::write(root.join("images/workspace/versions.lock"), "workspace_build_mode = \"connected\"\nworkspace_tag = \"gascan-workspace:test\"\n").unwrap();
    fs::write(root.join(".artifacts/connected-workspace-context/context-manifest.tsv"), "context\n").unwrap();
    let calls = temp.path().join("calls");
    executable(&root.join("scripts/prefetch-connected-workspace-image.sh"), "#!/bin/sh\nset -eu\nprintf 'prefetch\\n' >>\"$CALLS\"\n");
    executable(&root.join("scripts/build-connected-workspace-image.sh"), &format!("#!/bin/sh\nset -eu\nprintf 'build:%s\\n' \"$GASCAMP_READ_TOKEN_FILE\" >>\"$CALLS\"\n[ \"${{GASCAN_GATE_TEST_BUILD_FAILURE:-}}\" != 1 ]\nmkdir -p \"$GASCAN_GATE_ARTIFACTS\"\nref='gascan-workspace:test@sha256:{DIGEST}'\n[ \"${{REFERENCE_KIND:-}}\" != mutable ] || ref=gascan-workspace:test\nprintf '%s\\n' \"$ref\" >\"$GASCAN_GATE_ARTIFACTS/workspace-image-ref\"\nprintf '{{\"reference\":\"%s\",\"tag\":\"gascan-workspace:test\",\"platform\":\"linux/arm64\",\"image_digest\":\"sha256:{DIGEST}\",\"status\":\"succeeded\"}}\\n' \"$ref\" >\"$GASCAN_GATE_ARTIFACTS/workspace-image-build.json\"\ncase \"${{RECEIPT_KIND:-}}\" in missing) rm -f \"$GASCAN_GATE_ARTIFACTS/workspace-image-build.json\" ;; malformed) printf '{{bad\\n' >\"$GASCAN_GATE_ARTIFACTS/workspace-image-build.json\" ;; mismatched) printf '{{\"reference\":\"wrong\"}}\\n' >\"$GASCAN_GATE_ARTIFACTS/workspace-image-build.json\" ;; esac\nprintf '%s\\n' \"$ref\"\n"));
    executable(&root.join("scripts/validate-connected-image-receipt.sh"), "#!/bin/sh\nset -eu\n[ \"${GASCAN_GATE_TEST_RECEIPT_FAILURE:-}\" != 1 ]\nref=$(cat \"$1\")\nreceipt=${2:-$(dirname \"$1\")/workspace-image-build.json}\n[ -f \"$receipt\" ]\ncase \"$ref\" in gascan-workspace:test@sha256:????????????????????????????????????????????????????????????????) ;; *) exit 1;; esac\ngrep -Fq \"\\\"reference\\\":\\\"$ref\\\"\" \"$receipt\"\nprintf '%s\\n' \"$ref\"\n");
    for smoke in ["user-and-volumes.sh", "polyglot-smoke.sh", "gascamp-smoke.sh"] {
        let prefix = match smoke { "user-and-volumes.sh" => "user", "polyglot-smoke.sh" => "polyglot", _ => "gascamp" };
        executable(&root.join("tests/image").join(smoke), &format!("#!/bin/sh\nset -eu\nprintf 'smoke:{smoke}:%s:%s\\n' \"$GASCAN_IMAGE_REF_FILE\" \"$GASCAN_TEST_OWNER_TOKEN\" >>\"$CALLS\"\nname=gascan-image-{prefix}-test-$GASCAN_TEST_OWNER_TOKEN\n\"$CONTAINER_BIN\" create --name \"$name\" --label dev.gascan.test=true --label \"dev.gascan.test.owner=$GASCAN_TEST_OWNER_TOKEN\" image >/dev/null\n\"$CONTAINER_BIN\" inspect \"$name\" >/dev/null\n\"$CONTAINER_BIN\" stop --time 5 \"$name\" >/dev/null\n\"$CONTAINER_BIN\" inspect \"$name\" >/dev/null\n\"$CONTAINER_BIN\" delete \"$name\" >/dev/null\n[ \"${{FAIL_SMOKE:-}}\" != '{smoke}' ]\n"));
    }
    let container = temp.path().join("container");
    executable(&container, &format!("#!/bin/sh\nset -eu\nprintf 'container:%s\\n' \"$*\" >>\"$CALLS\"\nif [ \"$1 ${{2:-}}\" = 'image inspect' ]; then platform=${{IMAGE_PLATFORM:-arm64}}; printf '[{{\"id\":\"sha256:{DIGEST}\",\"configuration\":{{\"name\":\"gascan-workspace:test\",\"descriptor\":{{\"digest\":\"sha256:{DIGEST}\"}}}},\"variants\":[{{\"platform\":{{\"os\":\"linux\",\"architecture\":\"%s\"}}}}]}}]\\n' \"$platform\"; exit 0; fi\ncase \"$1\" in create) while [ $# -gt 0 ]; do [ \"$1\" = --name ] && {{ touch \"$STATE/$2\"; break; }}; shift; done ;; inspect) name=$2; [ \"${{RESIDUE:-}}\" = \"$name\" ] || [ -f \"$STATE/$name\" ] || exit 1; count_file=\"$STATE/.inspect-$name\"; count=0; [ ! -f \"$count_file\" ] || count=$(cat \"$count_file\"); count=$((count+1)); printf '%s' \"$count\" >\"$count_file\"; owner=$OWNER; [ \"${{FOREIGN:-}}\" = \"$name\" ] && owner=ffffffffffffffffffffffffffffffff; [ \"${{REPLACE_ON_SECOND_INSPECT:-}}\" = \"$name\" ] && [ \"$count\" -ge 2 ] && owner=ffffffffffffffffffffffffffffffff; printf '[{{\"configuration\":{{\"id\":\"%s\",\"name\":\"%s\",\"labels\":{{\"dev.gascan.test\":\"true\",\"dev.gascan.test.owner\":\"%s\"}}}}}}]\\n' \"$name\" \"$name\" \"$owner\" ;; stop) : ;; delete) name=${{@:$#}}; [ \"${{FAIL_DELETE:-}}\" != \"$name\" ] || exit 1; rm -f \"$STATE/$name\" ;; esac\n"));
    let token_file = temp.path().join("gascamp-token");
    fs::write(&token_file, "synthetic\n").unwrap(); fs::set_permissions(&token_file, fs::Permissions::from_mode(0o600)).unwrap();
    let state = temp.path().join("state"); fs::create_dir(&state).unwrap(); fs::write(state.join("unrelated-resource"), "foreign").unwrap();
    let mut command = Command::new("bash");
    command.arg(repository_root().join("scripts/run-connected-image-gate.sh")).env("GASCAN_GATE_TEST_ROOT", &root).env("GASCAN_GATE_ARTIFACTS", root.join(".artifacts")).env("GASCAMP_READ_TOKEN_FILE", &token_file).env("GASCAN_TEST_OWNER_TOKEN", TOKEN).env("CONTAINER_BIN", &container).env("CALLS", &calls).env("STATE", &state).env("OWNER", TOKEN);
    Fixture { temp, root, calls, command }
}

#[test]
fn successful_gate_uses_one_reference_and_token_then_publishes_atomically() {
    let mut f = fixture(); let output = f.command.output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let calls = fs::read_to_string(&f.calls).unwrap();
    assert!(calls.find("prefetch").unwrap() < calls.find("build:").unwrap()); assert_eq!(calls.matches("smoke:").count(), 3);
    for smoke in ["user-and-volumes.sh", "polyglot-smoke.sh", "gascamp-smoke.sh"] { assert!(calls.contains(&format!("smoke:{smoke}:{}:{TOKEN}", f.root.join(".artifacts/workspace-image-ref").display()))); }
    for prefix in ["user", "polyglot", "gascamp"] { assert!(calls.contains(&format!("inspect gascan-image-{prefix}-test-{TOKEN}"))); }
    for prefix in ["user", "polyglot", "gascamp"] {
        let name = format!("gascan-image-{prefix}-test-{TOKEN}");
        let create = calls.find(&format!("create --name {name} ")).unwrap();
        let stop = calls.find(&format!("stop --time 5 {name}")).unwrap();
        let delete = calls.find(&format!("delete {name}")).unwrap();
        assert!(create < stop && stop < delete);
        assert!(calls.contains(&format!("container:inspect {name}\ncontainer:stop --time 5 {name}\n")));
        assert!(calls.contains(&format!("container:inspect {name}\ncontainer:delete {name}\n")));
    }
    let evidence = fs::read_to_string(f.root.join("docs/evidence/connected-workspace-image.md")).unwrap();
    assert!(evidence.contains(&format!("gascan-workspace:test@sha256:{DIGEST}"))); assert!(evidence.contains("platform: `linux/arm64`"));
    assert_eq!(fs::read(f.root.join("images/workspace/approved-image.txt")).unwrap(), format!("gascan-workspace:test@sha256:{DIGEST}").as_bytes());
    assert_eq!(fs::read(f.temp.path().join("state/unrelated-resource")).unwrap(), b"foreign");
}

#[test]
fn injected_random_source_proves_fresh_live_tokens_across_runs() {
    for (index, token) in ["11111111111111111111111111111111", "22222222222222222222222222222222"].into_iter().enumerate() {
        let mut f = fixture();
        let random = f.temp.path().join(format!("random-{index}"));
        executable(&random, &format!("#!/bin/sh\nprintf '%s\\n' '{token}'\n"));
        f.command.env_remove("GASCAN_TEST_OWNER_TOKEN").env("GASCAN_GATE_RANDOM_BIN", &random);
        assert!(f.command.status().unwrap().success());
        let calls = fs::read_to_string(&f.calls).unwrap();
        assert!(calls.contains(&format!("gascan-image-user-test-{token}")));
    }
}

#[test]
fn every_failure_prevents_both_publications() {
    for failure in ["build", "receipt", "smoke", "residue"] {
        let mut f = fixture();
        match failure { "build" => { f.command.env("GASCAN_GATE_TEST_BUILD_FAILURE", "1"); }, "receipt" => { f.command.env("GASCAN_GATE_TEST_RECEIPT_FAILURE", "1"); }, "smoke" => { f.command.env("FAIL_SMOKE", "polyglot-smoke.sh"); }, "residue" => { f.command.env("RESIDUE", format!("gascan-image-user-test-{TOKEN}")); }, _ => unreachable!() };
        assert!(!f.command.status().unwrap().success(), "{failure}");
        assert!(!f.root.join("docs/evidence/connected-workspace-image.md").exists()); assert!(!f.root.join("images/workspace/approved-image.txt").exists());
    }
}

#[test]
fn stale_pass_pair_is_retired_before_work_and_owner_token_is_never_evidence() {
    let mut f = fixture();
    fs::write(f.root.join("docs/evidence/connected-workspace-image.md"), "status: `PASS`\n").unwrap();
    fs::write(f.root.join("images/workspace/approved-image.txt"), "stale").unwrap();
    f.command.env("GASCAN_GATE_TEST_BUILD_FAILURE", "1");
    assert!(!f.command.status().unwrap().success());
    assert!(!f.root.join("docs/evidence/connected-workspace-image.md").exists());
    assert!(!f.root.join("images/workspace/approved-image.txt").exists());

    let mut f = fixture();
    assert!(f.command.status().unwrap().success());
    let evidence = fs::read_to_string(f.root.join("docs/evidence/connected-workspace-image.md")).unwrap();
    assert!(!evidence.contains(TOKEN));
    assert!(!evidence.to_ascii_lowercase().contains("owner token"));
}

#[test]
fn stale_pass_pair_is_retired_even_when_secret_precondition_fails() {
    let mut f = fixture();
    fs::write(f.root.join("docs/evidence/connected-workspace-image.md"), "status: `PASS`\n").unwrap();
    fs::write(f.root.join("images/workspace/approved-image.txt"), "stale").unwrap();
    f.command.env("GASCAMP_READ_TOKEN_FILE", "relative");
    assert!(!f.command.status().unwrap().success());
    assert!(!f.root.join("docs/evidence/connected-workspace-image.md").exists());
    assert!(!f.root.join("images/workspace/approved-image.txt").exists());
    assert!(!f.calls.exists());
}

#[test]
fn every_publication_boundary_rolls_back_the_pair() {
    for boundary in ["after-stage", "after-evidence"] {
        for action in ["FAIL", "INT", "TERM"] {
            let mut f = fixture();
            f.command.env("GASCAN_GATE_TEST_PUBLICATION_BOUNDARY", boundary).env("GASCAN_GATE_TEST_PUBLICATION_ACTION", action);
            let status = f.command.status().unwrap();
            assert!(!status.success(), "{boundary}/{action}");
            if action == "INT" { assert_eq!(status.code(), Some(130)); }
            if action == "TERM" { assert_eq!(status.code(), Some(143)); }
            assert!(!f.root.join("docs/evidence/connected-workspace-image.md").exists());
            assert!(!f.root.join("images/workspace/approved-image.txt").exists());
            assert_eq!(fs::read_dir(f.root.join("docs/evidence")).unwrap().count(), 0);
            assert_eq!(fs::read_dir(f.root.join("images/workspace")).unwrap().filter(|entry| entry.as_ref().unwrap().file_name() != "versions.lock").count(), 0);
        }
    }
}

#[test]
fn canonical_repository_descendant_secret_is_rejected_before_work() {
    let mut f = fixture();
    let outside_parent = f.temp.path().join("outside-parent");
    std::os::unix::fs::symlink(&f.root, &outside_parent).unwrap();
    let secret = outside_parent.join("images/workspace/versions.lock");
    fs::set_permissions(f.root.join("images/workspace/versions.lock"), fs::Permissions::from_mode(0o600)).unwrap();
    f.command.env("GASCAMP_READ_TOKEN_FILE", secret);
    assert!(!f.command.status().unwrap().success());
    assert!(!f.calls.exists());
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
        let mut f = fixture(); f.command.env(variable, value);
        assert!(!f.command.status().unwrap().success(), "{variable}={value}");
        assert!(!f.root.join("docs/evidence/connected-workspace-image.md").exists());
        assert!(!f.root.join("images/workspace/approved-image.txt").exists());
    }
}

#[test]
fn foreign_replacement_between_checks_is_never_mutated() {
    let mut f = fixture();
    let name = format!("gascan-image-gascamp-test-{TOKEN}");
    fs::write(f.temp.path().join("state").join(&name), "").unwrap();
    f.command.env("REPLACE_ON_SECOND_INSPECT", &name).env("FAIL_SMOKE", "user-and-volumes.sh");
    assert!(!f.command.status().unwrap().success());
    let calls = fs::read_to_string(&f.calls).unwrap();
    assert!(calls.matches(&format!("container:inspect {name}")).count() >= 2);
    assert!(!calls.contains(&format!("container:stop --time 5 {name}")));
    assert!(!calls.contains(&format!("container:delete {name}")));
}

#[test]
fn cleanup_validates_ownership_before_mutation_and_leaves_foreign_resource() {
    let mut f = fixture(); let name = format!("gascan-image-gascamp-test-{TOKEN}"); fs::write(f.temp.path().join("state").join(&name), "").unwrap();
    f.command.env("FOREIGN", &name).env("FAIL_SMOKE", "user-and-volumes.sh"); assert!(!f.command.status().unwrap().success());
    let calls = fs::read_to_string(&f.calls).unwrap(); assert!(calls.contains(&format!("inspect {name}"))); assert!(!calls.contains(&format!("stop --time 5 {name}"))); assert!(!calls.contains(&format!("delete {name}")));
}

#[test]
fn int_and_term_exit_nonzero_after_bounded_cleanup() {
    for (signal, code) in [("INT", 130), ("TERM", 143)] { let mut f = fixture(); f.command.env("GASCAN_GATE_TEST_SIGNAL", signal); let status = f.command.status().unwrap(); assert_eq!(status.code(), Some(code)); let calls = fs::read_to_string(&f.calls).unwrap(); assert!(calls.contains("stop --time 5")); assert!(calls.contains("delete gascan-image-user-test-")); assert!(!f.root.join("docs/evidence/connected-workspace-image.md").exists()); assert!(!f.root.join("images/workspace/approved-image.txt").exists()); assert!(!f.temp.path().join("state").join(format!("gascan-image-user-test-{TOKEN}")).exists()); }
}

#[test]
fn cleanup_failure_is_nonzero_and_never_publishes() {
    let mut f = fixture();
    f.command.env("GASCAN_GATE_TEST_SIGNAL", "TERM").env("FAIL_DELETE", format!("gascan-image-user-test-{TOKEN}"));
    let status = f.command.status().unwrap();
    assert_eq!(status.code(), Some(1));
    assert!(!f.root.join("docs/evidence/connected-workspace-image.md").exists());
    assert!(!f.root.join("images/workspace/approved-image.txt").exists());
}
