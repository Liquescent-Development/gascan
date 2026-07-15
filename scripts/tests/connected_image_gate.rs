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
    executable(&root.join("scripts/build-connected-workspace-image.sh"), &format!("#!/bin/sh\nset -eu\nprintf 'build:%s\\n' \"$GASCAMP_READ_TOKEN_FILE\" >>\"$CALLS\"\n[ \"${{GASCAN_GATE_TEST_BUILD_FAILURE:-}}\" != 1 ]\nmkdir -p \"$GASCAN_GATE_ARTIFACTS\"\nref='gascan-workspace:test@sha256:{DIGEST}'\nprintf '%s\\n' \"$ref\" >\"$GASCAN_GATE_ARTIFACTS/workspace-image-ref\"\nprintf '{{\"reference\":\"%s\",\"tag\":\"gascan-workspace:test\",\"platform\":\"linux/arm64\",\"image_digest\":\"sha256:{DIGEST}\",\"status\":\"succeeded\"}}\\n' \"$ref\" >\"$GASCAN_GATE_ARTIFACTS/workspace-image-build.json\"\nprintf '%s\\n' \"$ref\"\n"));
    executable(&root.join("scripts/validate-connected-image-receipt.sh"), "#!/bin/sh\nset -eu\n[ \"${GASCAN_GATE_TEST_RECEIPT_FAILURE:-}\" != 1 ]\nref=$(cat \"$1\")\n[ -f \"${2:-$(dirname \"$1\")/workspace-image-build.json}\" ]\ncase \"$ref\" in gascan-workspace:test@sha256:????????????????????????????????????????????????????????????????) ;; *) exit 1;; esac\nprintf '%s\\n' \"$ref\"\n");
    for smoke in ["user-and-volumes.sh", "polyglot-smoke.sh", "gascamp-smoke.sh"] {
        executable(&root.join("tests/image").join(smoke), &format!("#!/bin/sh\nset -eu\nprintf 'smoke:{smoke}:%s:%s\\n' \"$GASCAN_IMAGE_REF_FILE\" \"$GASCAN_TEST_OWNER_TOKEN\" >>\"$CALLS\"\n[ \"${{FAIL_SMOKE:-}}\" != '{smoke}' ]\n"));
    }
    let container = temp.path().join("container");
    executable(&container, &format!("#!/bin/sh\nset -eu\nprintf 'container:%s\\n' \"$*\" >>\"$CALLS\"\nif [ \"$1 ${{2:-}}\" = 'image inspect' ]; then printf '[{{\"id\":\"sha256:{DIGEST}\",\"configuration\":{{\"name\":\"gascan-workspace:test\",\"descriptor\":{{\"digest\":\"sha256:{DIGEST}\"}}}},\"variants\":[{{\"platform\":{{\"os\":\"linux\",\"architecture\":\"arm64\"}}}}]}}]\\n'; exit 0; fi\ncase \"$1\" in create) while [ $# -gt 0 ]; do [ \"$1\" = --name ] && {{ touch \"$STATE/$2\"; break; }}; shift; done ;; inspect) name=$2; [ \"${{RESIDUE:-}}\" = \"$name\" ] || [ -f \"$STATE/$name\" ] || exit 1; owner=$OWNER; [ \"${{FOREIGN:-}}\" = \"$name\" ] && owner=ffffffffffffffffffffffffffffffff; printf '[{{\"configuration\":{{\"id\":\"%s\",\"name\":\"%s\",\"labels\":{{\"dev.gascan.test\":\"true\",\"dev.gascan.test.owner\":\"%s\"}}}}}}]\\n' \"$name\" \"$name\" \"$owner\" ;; stop) : ;; delete) rm -f \"$STATE/${{@:$#}}\" ;; esac\n"));
    let token_file = temp.path().join("gascamp-token");
    fs::write(&token_file, "synthetic\n").unwrap(); fs::set_permissions(&token_file, fs::Permissions::from_mode(0o600)).unwrap();
    let state = temp.path().join("state"); fs::create_dir(&state).unwrap();
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
    let evidence = fs::read_to_string(f.root.join("docs/evidence/connected-workspace-image.md")).unwrap();
    assert!(evidence.contains(&format!("gascan-workspace:test@sha256:{DIGEST}"))); assert!(evidence.contains("platform: `linux/arm64`"));
    assert_eq!(fs::read(f.root.join("images/workspace/approved-image.txt")).unwrap(), format!("gascan-workspace:test@sha256:{DIGEST}").as_bytes());
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
fn cleanup_validates_ownership_before_mutation_and_leaves_foreign_resource() {
    let mut f = fixture(); let name = format!("gascan-image-gascamp-test-{TOKEN}"); fs::write(f.temp.path().join("state").join(&name), "").unwrap();
    f.command.env("FOREIGN", &name).env("FAIL_SMOKE", "user-and-volumes.sh"); assert!(!f.command.status().unwrap().success());
    let calls = fs::read_to_string(&f.calls).unwrap(); assert!(calls.contains(&format!("inspect {name}"))); assert!(!calls.contains(&format!("stop --time 5 {name}"))); assert!(!calls.contains(&format!("delete {name}")));
}

#[test]
fn int_and_term_exit_nonzero_after_bounded_cleanup() {
    for signal in ["INT", "TERM"] { let mut f = fixture(); f.command.env("GASCAN_GATE_TEST_SIGNAL", signal); assert!(!f.command.status().unwrap().success()); let calls = fs::read_to_string(&f.calls).unwrap(); assert!(calls.contains("stop --time 5")); assert!(calls.contains("delete gascan-image-user-test-")); }
}
