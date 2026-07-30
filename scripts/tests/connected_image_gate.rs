use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
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

fn file_digest(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

fn configure_validator_dispatcher(command: &mut Command, fixture_manifest: &Path) {
    let fixture_manifest = fs::canonicalize(fixture_manifest.parent().unwrap())
        .unwrap()
        .join(fixture_manifest.file_name().unwrap());
    command
        .env(
            "GASCAN_GATE_TEST_CARGO_MANIFEST",
            repository_root().join("scripts/Cargo.toml"),
        )
        .env("GASCAN_GATE_TEST_FIXTURE_CARGO_MANIFEST", fixture_manifest)
        .env(
            "GASCAN_GATE_TEST_VALIDATE_CONNECTED_BUILD",
            env!("CARGO_BIN_EXE_validate-connected-build"),
        )
        .env(
            "GASCAN_GATE_TEST_VALIDATE_CONTAINER_INVENTORY",
            env!("CARGO_BIN_EXE_validate-container-inventory"),
        )
        .env(
            "GASCAN_GATE_TEST_VALIDATE_OWNED_CONTAINER",
            env!("CARGO_BIN_EXE_validate-owned-container"),
        )
        .env(
            "GASCAN_GATE_TEST_VALIDATE_OWNED_VOLUME",
            env!("CARGO_BIN_EXE_validate-owned-volume"),
        )
        .env(
            "GASCAN_GATE_TEST_VALIDATE_RUNTIME_CONTRACT",
            env!("CARGO_BIN_EXE_validate-runtime-contract"),
        );
}

struct Fixture {
    temp: tempfile::TempDir,
    root: PathBuf,
    calls: PathBuf,
    path: std::ffi::OsString,
    command: Command,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    for directory in [
        "scripts",
        "tests/image",
        "images/workspace",
        "images/workspace/bin",
        "crates/gascand/src",
        "docs/evidence",
        ".artifacts/connected-workspace-context",
    ] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }
    fs::write(
        root.join("images/workspace/versions.lock"),
        "workspace_build_mode = \"connected\"\nworkspace_tag = \"gascan-workspace:d4964500a3295a33\"\n",
    )
    .unwrap();
    fs::write(
        root.join(".artifacts/connected-workspace-context/context-manifest.tsv"),
        "context\n",
    )
    .unwrap();
    let lock_digest = file_digest(&root.join("images/workspace/versions.lock"));
    let context_digest =
        file_digest(&root.join(".artifacts/connected-workspace-context/context-manifest.tsv"));
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
    fs::copy(
        repository_root().join("images/workspace/runtime-contract.toml"),
        root.join("images/workspace/runtime-contract.toml"),
    )
    .unwrap();
    for helper in [
        "configure-shell-home",
        "configure-workstation-home",
        "initialize-rust-home",
        "select-gascamp",
    ] {
        fs::copy(
            repository_root().join("images/workspace/bin").join(helper),
            root.join("images/workspace/bin").join(helper),
        )
        .unwrap();
    }
    fs::write(
        root.join("crates/gascand/src/service.rs"),
        r#"
            const CONFIGURE_SHELL_HOME: &str = "/usr/local/bin/configure-shell-home";
            const INITIALIZE_RUST_HOME: &str = "/usr/local/bin/initialize-rust-home";
            const CONFIGURE_WORKSTATION_HOME: &str = "/usr/local/bin/configure-workstation-home";
            const SELECT_GASCAMP: &str = "/usr/local/bin/select-gascamp";
            const MISE: &str = "/usr/local/bin/mise";
        "#,
    )
    .unwrap();
    let calls = temp.path().join("calls");
    executable(
        &temp.path().join("cargo"),
        r#"#!/bin/sh
set -eu
test "$#" -ge 9 || exit 64
test "$1" = run || exit 64
test "$2" = --quiet || exit 64
test "$3" = --locked || exit 64
test "$4" = --offline || exit 64
test "$5" = --manifest-path || exit 64
case "$6" in
  "$GASCAN_GATE_TEST_CARGO_MANIFEST"|"$GASCAN_GATE_TEST_FIXTURE_CARGO_MANIFEST") ;;
  *) exit 64 ;;
esac
test "$7" = --bin || exit 64
bin=$8
test "$9" = -- || exit 64
shift 9
case "$bin" in
  validate-connected-build) executable=$GASCAN_GATE_TEST_VALIDATE_CONNECTED_BUILD ;;
  validate-container-inventory) executable=$GASCAN_GATE_TEST_VALIDATE_CONTAINER_INVENTORY ;;
  validate-owned-container) executable=$GASCAN_GATE_TEST_VALIDATE_OWNED_CONTAINER ;;
  validate-owned-volume) executable=$GASCAN_GATE_TEST_VALIDATE_OWNED_VOLUME ;;
  validate-runtime-contract) executable=$GASCAN_GATE_TEST_VALIDATE_RUNTIME_CONTRACT ;;
  *) exit 64 ;;
esac
exec "$executable" "$@"
"#,
    );
    executable(
        &root.join("scripts/prefetch-connected-workspace-image.sh"),
        "#!/bin/sh\nset -eu\nprintf 'prefetch\\n' >>\"$CALLS\"\n",
    );
    executable(
        &root.join("scripts/build-connected-workspace-image.sh"),
        &format!(
            "#!/bin/sh\nset -eu\nprintf 'build\\n' >>\"$CALLS\"\n[ \"${{GASCAN_GATE_TEST_BUILD_FAILURE:-}}\" != 1 ]\nmkdir -p \"$GASCAN_GATE_ARTIFACTS\"\nref='gascan-workspace:d4964500a3295a33@sha256:{DIGEST}'\n[ \"${{REFERENCE_KIND:-}}\" != mutable ] || ref=gascan-workspace:d4964500a3295a33\nprintf '%s\\n' \"$ref\" >\"$GASCAN_GATE_ARTIFACTS/workspace-image-ref\"\nprintf '{{\"reference\":\"%s\",\"tag\":\"gascan-workspace:d4964500a3295a33\",\"platform\":\"linux/arm64\",\"lock_digest\":\"{lock_digest}\",\"context_digest\":\"{context_digest}\",\"source_digest\":\"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee\",\"image_digest\":\"sha256:{DIGEST}\",\"status\":\"succeeded\"}}\\n' \"$ref\" >\"$GASCAN_GATE_ARTIFACTS/workspace-image-build.json\"\ncase \"${{RECEIPT_KIND:-}}\" in missing) rm -f \"$GASCAN_GATE_ARTIFACTS/workspace-image-build.json\" ;; malformed) printf '{{bad\\n' >\"$GASCAN_GATE_ARTIFACTS/workspace-image-build.json\" ;; mismatched) printf '{{\"reference\":\"wrong\"}}\\n' >\"$GASCAN_GATE_ARTIFACTS/workspace-image-build.json\" ;; esac\nprintf '%s\\n' \"$ref\"\n"
        ),
    );
    fs::copy(
        repository_root().join("scripts/validate-connected-image-receipt.sh"),
        root.join("scripts/validate-connected-image-receipt-real.sh"),
    )
    .unwrap();
    executable(
        &root.join("scripts/validate-connected-image-receipt.sh"),
        "#!/bin/sh\nset -eu\n[ \"${GASCAN_GATE_TEST_RECEIPT_FAILURE:-}\" != 1 ]\nexec \"$(dirname \"$0\")/validate-connected-image-receipt-real.sh\" \"$@\"\n",
    );
    for smoke in [
        "user-and-volumes.sh",
        "polyglot-smoke.sh",
        "gascamp-smoke.sh",
        "workstation-smoke.sh",
        "container-cli.sh",
    ] {
        fs::copy(
            repository_root().join("tests/image").join(smoke),
            root.join("tests/image").join(smoke),
        )
        .unwrap();
    }
    fs::create_dir_all(root.join("images/workspace/tests")).unwrap();
    fs::copy(
        repository_root().join("images/workspace/tests/ssh-contract.sh"),
        root.join("images/workspace/tests/ssh-contract.sh"),
    )
    .unwrap();
    let raw_container = temp.path().join("container-raw");
    executable(
        &raw_container,
        &format!(
            "#!/bin/sh\nset -eu\nprintf 'container:%s\\n' \"$*\" >>\"$CALLS\"\nif [ \"$1 ${{2:-}}\" = 'image inspect' ]; then [ $# -eq 3 ] || exit 93; expected=${{INSPECT_REFERENCE:-gascan-workspace:d4964500a3295a33}}; [ \"$3\" = \"$expected\" ] || {{ printf 'image not found\\n' >&2; exit 94; }}; [ \"${{IMAGE_AVAILABLE:-1}}\" = 1 ] || exit 94; platform=${{IMAGE_PLATFORM:-arm64}}; image_digest=${{IMAGE_DIGEST:-sha256:{DIGEST}}}; image_id=${{image_digest#sha256:}}; printf '[{{\"id\":\"%s\",\"configuration\":{{\"name\":\"%s\",\"descriptor\":{{\"digest\":\"%s\"}}}},\"variants\":[{{\"platform\":{{\"os\":\"linux\",\"architecture\":\"%s\"}},\"digest\":\"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\"}}]}}]\\n' \"$image_id\" \"$expected\" \"$image_digest\" \"$platform\"; exit 0; fi\nif [ \"$1\" = volume ]; then action=$2; shift 2; case \"$action\" in list) first=true; printf '['; for name in gascan-image-workstation-tools-$OWNER gascan-image-workstation-cache-$OWNER gascan-image-workstation-config-$OWNER gascan-image-polyglot-tools-$OWNER gascan-image-ssh-config-$OWNER; do if [ -f \"$STATE/.volume-$name\" ]; then $first || printf ','; first=false; printf '{{\"id\":\"%s\"}}' \"$name\"; fi; done; printf ']\\n' ;; create) name=; for argument in \"$@\"; do name=$argument; done; touch \"$STATE/.volume-$name\" ;; inspect) name=$1; [ -f \"$STATE/.volume-$name\" ] || exit 1; count_file=\"$STATE/.volume-inspect-$name\"; count=0; [ ! -f \"$count_file\" ] || count=$(cat \"$count_file\"); count=$((count+1)); printf '%s' \"$count\" >\"$count_file\"; owner=$OWNER; [ \"${{FOREIGN_VOLUME:-}}\" = \"$name\" ] && owner=ffffffffffffffffffffffffffffffff; [ \"${{FAIL_VOLUME_ATTESTATION_TWICE:-}}\" = \"$name\" ] && [ \"$count\" -le 2 ] && owner=ffffffffffffffffffffffffffffffff; [ \"${{REPLACE_VOLUME_ON_SECOND_INSPECT:-}}\" = \"$name\" ] && [ \"$count\" -ge 2 ] && owner=ffffffffffffffffffffffffffffffff; printf '[{{\"id\":\"%s\",\"configuration\":{{\"name\":\"%s\",\"labels\":{{\"dev.gascan.test\":\"true\",\"dev.gascan.test.owner\":\"%s\"}}}}}}]\\n' \"$name\" \"$name\" \"$owner\" ;; delete) name=$1; if [ \"${{FAIL_VOLUME_DELETE_ONCE:-}}\" = \"$name\" ] && [ ! -f \"$STATE/.volume-delete-failed-$name\" ]; then touch \"$STATE/.volume-delete-failed-$name\"; exit 1; fi; [ \"${{FAIL_VOLUME_DELETE:-}}\" != \"$name\" ] || exit 1; rm -f \"$STATE/.volume-$name\" ;; esac; exit 0; fi\ncase \"$1\" in create) name=; image=; shift; while [ $# -gt 0 ]; do if [ \"$1\" = --name ]; then name=$2; shift 2; continue; fi; image=$1; shift; done; touch \"$STATE/$name\"; printf '%s' \"$image\" >\"$STATE/.image-$name\" ;; inspect) name=$2; [ \"${{RESIDUE:-}}\" = \"$name\" ] || [ -f \"$STATE/$name\" ] || exit 1; count_file=\"$STATE/.inspect-$name\"; count=0; [ ! -f \"$count_file\" ] || count=$(cat \"$count_file\"); count=$((count+1)); printf '%s' \"$count\" >\"$count_file\"; owner=$OWNER; [ \"${{FOREIGN:-}}\" = \"$name\" ] && owner=ffffffffffffffffffffffffffffffff; [ \"${{REPLACE_ON_SECOND_INSPECT:-}}\" = \"$name\" ] && [ \"$count\" -ge 2 ] && owner=ffffffffffffffffffffffffffffffff; image=${{CONTAINER_IMAGE_REFERENCE:-$(cat \"$STATE/.image-$name\" 2>/dev/null || printf 'gascan-workspace:d4964500a3295a33')}}; digest=${{CONTAINER_IMAGE_DIGEST:-sha256:{DIGEST}}}; printf '[{{\"id\":\"%s\",\"configuration\":{{\"id\":\"%s\",\"labels\":{{\"dev.gascan.test\":\"true\",\"dev.gascan.test.owner\":\"%s\"}},\"image\":{{\"descriptor\":{{\"digest\":\"%s\"}},\"reference\":\"%s\"}}}}}}]\\n' \"$name\" \"$name\" \"$owner\" \"$digest\" \"$image\" ;; exec) if [ \"${{FAIL_WORKSTATION_EXEC:-0}}\" = 1 ]; then case \"$*\" in *workstation-contract.sh*) exit 1 ;; esac; fi ;; stop) : ;; delete) name=${{@:$#}}; [ \"${{FAIL_DELETE:-}}\" != \"$name\" ] || exit 1; rm -f \"$STATE/$name\" \"$STATE/.image-$name\" ;; esac\n"
        ),
    );
    let container = temp.path().join("container");
    executable(
        &container,
        "#!/bin/sh\nset -eu\nif [ \"$1\" = list ]; then first=true; printf '['; for name in gascan-image-user-test-$OWNER gascan-image-polyglot-test-$OWNER gascan-image-gascamp-test-$OWNER gascan-image-workstation-test-$OWNER gascan-image-ws-network-test-$OWNER gascan-image-ssh-test-$OWNER; do if [ \"${RESIDUE:-}\" = \"$name\" ] || [ -f \"$STATE/$name\" ]; then $first || printf ','; first=false; printf '{\"id\":\"%s\",\"configuration\":{\"id\":\"%s\",\"labels\":{}}}' \"$name\" \"$name\"; fi; done; printf ']\\n'; exit 0; fi\nexec \"$RAW_CONTAINER\" \"$@\"\n",
    );
    let state = temp.path().join("state");
    fs::create_dir(&state).unwrap();
    fs::write(state.join("unrelated-resource"), "foreign").unwrap();
    let mut command = Command::new("bash");
    let path = std::env::join_paths(std::iter::once(temp.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();
    command
        .arg(repository_root().join("scripts/run-connected-image-gate.sh"))
        .env("PATH", &path)
        .env("GASCAN_GATE_TEST_ROOT", &root)
        .env("GASCAN_GATE_ARTIFACTS", root.join(".artifacts"))
        .env("GASCAN_TEST_OWNER_TOKEN", TOKEN)
        .env("CONTAINER_BIN", &container)
        .env("CALLS", &calls)
        .env("STATE", &state)
        .env("OWNER", TOKEN)
        .env("RAW_CONTAINER", &raw_container);
    configure_validator_dispatcher(&mut command, &root.join("scripts/Cargo.toml"));
    Fixture {
        temp,
        root,
        calls,
        path,
        command,
    }
}

#[test]
fn fixture_cargo_dispatcher_is_strict_and_preserves_validator_io() {
    let f = fixture();
    let cargo = f.temp.path().join("cargo");
    let manifest = repository_root().join("scripts/Cargo.toml");
    let name = "gascan-image-user-test-owner";
    let token = TOKEN;
    let inventory = format!(
        r#"[{{"id":"{name}","configuration":{{"id":"{name}","labels":{{"dev.gascan.test":"true","dev.gascan.test.owner":"{token}"}}}}}}]"#
    );
    let mut valid = Command::new(&cargo);
    valid
        .args(["run", "--quiet", "--locked", "--offline", "--manifest-path"])
        .arg(&manifest)
        .args(["--bin", "validate-owned-container", "--", name, token])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_validator_dispatcher(&mut valid, &f.root.join("scripts/Cargo.toml"));
    let mut child = valid.spawn().unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(inventory.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());

    for invalid in [
        &["check", "--quiet"][..],
        &["run", "--verbose"][..],
        &["run", "--quiet", "--locked"][..],
    ] {
        let mut command = Command::new(&cargo);
        command.args(invalid);
        configure_validator_dispatcher(&mut command, &f.root.join("scripts/Cargo.toml"));
        assert_eq!(command.status().unwrap().code(), Some(64));
    }
    let mut unknown_bin = Command::new(&cargo);
    unknown_bin
        .args(["run", "--quiet", "--locked", "--offline", "--manifest-path"])
        .arg(manifest)
        .args(["--bin", "unknown-validator", "--"]);
    configure_validator_dispatcher(&mut unknown_bin, &f.root.join("scripts/Cargo.toml"));
    assert_eq!(unknown_bin.status().unwrap().code(), Some(64));

    let mut unknown_manifest = Command::new(&cargo);
    unknown_manifest.args([
        "run",
        "--quiet",
        "--locked",
        "--offline",
        "--manifest-path",
        "/tmp/foreign/Cargo.toml",
        "--bin",
        "validate-owned-container",
        "--",
    ]);
    configure_validator_dispatcher(&mut unknown_manifest, &f.root.join("scripts/Cargo.toml"));
    assert_eq!(unknown_manifest.status().unwrap().code(), Some(64));
}

fn seed_valid_receipt(f: &Fixture) {
    let artifacts = f.root.join(".artifacts");
    fs::create_dir_all(&artifacts).unwrap();
    let reference = format!("gascan-workspace:d4964500a3295a33@sha256:{DIGEST}");
    let lock_digest = file_digest(&f.root.join("images/workspace/versions.lock"));
    let context_digest =
        file_digest(&artifacts.join("connected-workspace-context/context-manifest.tsv"));
    fs::write(
        artifacts.join("workspace-image-ref"),
        format!("{reference}\n"),
    )
    .unwrap();
    fs::write(
        artifacts.join("workspace-image-build.json"),
        format!(
            "{{\"reference\":\"{reference}\",\"tag\":\"gascan-workspace:d4964500a3295a33\",\"platform\":\"linux/arm64\",\"lock_digest\":\"{lock_digest}\",\"context_digest\":\"{context_digest}\",\"source_digest\":\"{}\",\"image_digest\":\"sha256:{DIGEST}\",\"status\":\"succeeded\"}}\n",
            "e".repeat(64)
        ),
    )
    .unwrap();
}

#[test]
fn fixture_cargo_dispatcher_accepts_the_canonical_fixture_manifest() {
    let f = fixture();
    seed_valid_receipt(&f);
    let artifacts = f.root.join(".artifacts");
    let mut command = Command::new(f.root.join("scripts/validate-connected-image-receipt.sh"));
    command
        .args([
            artifacts.join("workspace-image-ref"),
            artifacts.join("workspace-image-build.json"),
        ])
        .env("PATH", &f.path)
        .env("GASCAN_IMAGE_ARTIFACTS", &artifacts);
    configure_validator_dispatcher(&mut command, &f.root.join("scripts/Cargo.toml"));
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn seed_valid_ghcr_receipt(f: &Fixture) -> String {
    let artifacts = f.root.join(".artifacts");
    fs::create_dir_all(&artifacts).unwrap();
    let tag = "ghcr.io/liquescent-development/gascan/workspace:d4964500a3295a33";
    let reference = format!("{tag}@sha256:{DIGEST}");
    let lock_digest = file_digest(&f.root.join("images/workspace/versions.lock"));
    let context_digest =
        file_digest(&artifacts.join("connected-workspace-context/context-manifest.tsv"));
    fs::write(
        artifacts.join("workspace-image-ref"),
        format!("{reference}\n"),
    )
    .unwrap();
    fs::write(
        artifacts.join("workspace-image-build.json"),
        format!(
            "{{\"reference\":\"{reference}\",\"tag\":\"{tag}\",\"platform\":\"linux/arm64\",\"lock_digest\":\"{lock_digest}\",\"context_digest\":\"{context_digest}\",\"source_digest\":\"{}\",\"image_digest\":\"sha256:{DIGEST}\",\"status\":\"succeeded\"}}\n",
            "e".repeat(64)
        ),
    )
    .unwrap();
    reference
}

fn assert_no_publications(f: &Fixture) {
    assert!(
        !f.root
            .join("docs/evidence/connected-workspace-image.md")
            .exists()
    );
    assert!(!f.root.join("images/workspace/approved-image.txt").exists());
}

#[test]
fn connected_gate_has_no_privileged_snapshot_or_sudo_precondition() {
    let gate =
        fs::read_to_string(repository_root().join("scripts/run-connected-image-gate.sh")).unwrap();
    for obsolete in [
        "snapshot-helper-identity",
        "/Library/PrivilegedHelperTools/dev.gascan.snapshot-workspace-context",
        "GASCAN_GATE_TEST_SNAPSHOT_HELPER",
        "GASCAN_GATE_TEST_HELPER_IDENTITY_BIN",
        "sudo -n",
    ] {
        assert!(
            !gate.contains(obsolete),
            "connected gate retained obsolete privileged precondition: {obsolete}"
        );
    }
}

#[test]
fn repository_receipt_validator_is_executable() {
    let permissions =
        fs::metadata(repository_root().join("scripts/validate-connected-image-receipt.sh"))
            .unwrap()
            .permissions();
    assert_eq!(
        permissions.mode() & 0o100,
        0o100,
        "connected receipt validator must set the owner-execute bit in a checkout"
    );
}

#[test]
fn workstation_smoke_initializes_the_mounted_home_before_contract_checks() {
    let smoke =
        fs::read_to_string(repository_root().join("tests/image/workstation-smoke.sh")).unwrap();
    let initialize_rust = smoke
        .find("/usr/local/bin/initialize-rust-home")
        .expect("workstation smoke must seed the mounted Rust home");
    let initialize_workstation = smoke
        .find("/usr/local/bin/configure-workstation-home")
        .expect("workstation smoke must initialize image-owned home defaults");
    let contract = smoke
        .find("/opt/gascan/tests/workstation-contract.sh")
        .expect("workstation smoke must run the image contract");
    assert!(
        initialize_rust < initialize_workstation && initialize_workstation < contract,
        "mounted workstation home must be initialized before its contract is checked"
    );
    for rust_command in ["cargo run --manifest-path", "cargo install --path"] {
        assert!(
            initialize_rust < smoke.find(rust_command).expect("missing Rust write smoke"),
            "Rust command ran before the writable Rust home was seeded: {rust_command}"
        );
    }
}

#[test]
fn connected_gate_runs_the_managed_ssh_contract_without_publishing_a_port() {
    let gate =
        fs::read_to_string(repository_root().join("scripts/run-connected-image-gate.sh")).unwrap();
    let contract =
        fs::read_to_string(repository_root().join("images/workspace/tests/ssh-contract.sh"))
            .unwrap();
    assert!(
        gate.contains("\"$root/images/workspace/tests/ssh-contract.sh\""),
        "connected gate does not run the managed SSH contract"
    );
    assert!(
        gate.contains("GASCAN_IMAGE_REF_FILE=\"$reference_file\""),
        "SSH contract does not consume the candidate receipt"
    );
    assert!(
        !gate.contains("--publish"),
        "connected gate publishes a runtime port"
    );
    for required in [
        "--network none",
        "GASCAN_SSH_ENABLED=1",
        "GASCAN_SSH_AUTHORIZED_KEY",
        "127.0.0.1:22",
        "ssh-keygen",
        "PasswordAuthentication",
        "PermitRootLogin",
        "AllowTcpForwarding",
        "AllowAgentForwarding",
        "fingerprint",
        "sftp",
    ] {
        assert!(
            contract.contains(required),
            "SSH live contract omits: {required}"
        );
    }
}

#[test]
fn successful_candidate_validation_preserves_tracked_approval_and_stages_only_candidate() {
    let mut f = fixture();
    let existing_approval = "ghcr.io/liquescent-development/gascan/workspace:approved@sha256:\
         bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let existing_evidence = "# Connected workspace image evidence\n\n- status: `PASS`\n\
                             - image: `existing-approved-image`\n";
    fs::write(
        f.root.join("images/workspace/approved-image.txt"),
        existing_approval,
    )
    .unwrap();
    fs::write(
        f.root.join("docs/evidence/connected-workspace-image.md"),
        existing_evidence,
    )
    .unwrap();

    let output = f.command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(f.root.join("images/workspace/approved-image.txt")).unwrap(),
        existing_approval
    );
    assert_eq!(
        fs::read_to_string(f.root.join("docs/evidence/connected-workspace-image.md")).unwrap(),
        existing_evidence
    );
    assert_eq!(
        fs::read_to_string(
            f.root
                .join(".artifacts/connected-workspace-image-candidate.txt")
        )
        .unwrap(),
        format!("gascan-workspace:d4964500a3295a33@sha256:{DIGEST}\n")
    );
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
    for prefix in ["user", "polyglot", "gascamp", "workstation"] {
        assert!(calls.contains(&format!("inspect gascan-image-{prefix}-test-{TOKEN}")));
    }
    for prefix in ["user", "polyglot", "gascamp", "workstation"] {
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
    assert_eq!(
        fs::read_to_string(
            f.root
                .join(".artifacts/connected-workspace-image-candidate.txt")
        )
        .unwrap(),
        format!("gascan-workspace:d4964500a3295a33@sha256:{DIGEST}\n")
    );
    assert_no_publications(&f);
    assert_eq!(
        fs::read(f.temp.path().join("state/unrelated-resource")).unwrap(),
        b"foreign"
    );
}

#[test]
fn successful_prebuilt_gate_skips_build_work_and_stages_exact_candidate_reference() {
    let mut f = fixture();
    seed_valid_receipt(&f);
    let output = f.command.arg("--prebuilt").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = fs::read_to_string(&f.calls).unwrap();
    assert!(
        !calls
            .lines()
            .any(|line| line == "prefetch" || line == "build")
    );
    assert!(calls.contains("container:image inspect gascan-workspace:d4964500a3295a33"));
    for prefix in ["user", "polyglot", "gascamp", "workstation"] {
        let name = format!("gascan-image-{prefix}-test-{TOKEN}");
        assert!(calls.contains(&format!("create --name {name} ")));
        assert!(calls.contains(&format!("delete {name}")));
        assert!(!f.temp.path().join("state").join(name).exists());
    }
    for kind in ["tools", "cache", "config"] {
        let name = format!("gascan-image-workstation-{kind}-{TOKEN}");
        assert!(calls.contains("container:volume create "));
        assert!(calls.contains(&format!(" {name}\n")));
        assert!(calls.contains(&format!("container:volume delete {name}")));
        assert!(
            !f.temp
                .path()
                .join("state")
                .join(format!(".volume-{name}"))
                .exists()
        );
    }
    let reference = format!("gascan-workspace:d4964500a3295a33@sha256:{DIGEST}");
    assert_eq!(
        fs::read_to_string(
            f.root
                .join(".artifacts/connected-workspace-image-candidate.txt")
        )
        .unwrap()
        .trim(),
        reference
    );
    assert_no_publications(&f);
}

#[test]
fn workstation_contract_is_wired_into_the_release_blocking_gate() {
    let gate =
        fs::read_to_string(repository_root().join("scripts/run-connected-image-gate.sh")).unwrap();
    assert!(
        gate.contains("workstation-smoke.sh"),
        "connected image gate must execute the workstation smoke"
    );
    let smoke =
        fs::read_to_string(repository_root().join("tests/image/workstation-smoke.sh")).unwrap();
    let contract = fs::read_to_string(
        repository_root().join("images/workspace/tests/workstation-contract.sh"),
    )
    .unwrap();
    assert!(
        smoke.contains("/opt/gascan/tests/workstation-contract.sh"),
        "host smoke must invoke the immutable guest contract"
    );
    for required in [
        "--volume \"$tools_volume:/home/workspace/.local\"",
        "--volume \"$cache_volume:/home/workspace/.cache\"",
        "--volume \"$config_volume:/home/workspace/.config\"",
        "--env CARGO_HOME=/home/workspace/.local/share/cargo",
        "--env RUSTUP_HOME=/home/workspace/.local/share/rustup",
        "--env GOBIN=/home/workspace/.local/bin",
        "--env MISE_DATA_DIR=/home/workspace/.local/share/mise",
        "--env MISE_SYSTEM_DATA_DIR=/opt/gascan/mise",
        "--env MISE_CACHE_DIR=/home/workspace/.cache/mise",
        "--env MISE_GLOBAL_CONFIG_FILE=/home/workspace/.config/gascan/mise.toml",
        "--bin validate-owned-volume",
        "--bin validate-container-inventory",
        "bounded_container volume inspect \"$volume\" |\n    cargo run --quiet --locked --offline",
        "bounded_container volume delete \"$volume\"",
        "offline_verified=false",
        "network_verified=false",
        "container_inventory_proves_absent",
        "refusing cleanup of unattested container",
    ] {
        assert!(
            smoke.contains(required),
            "workstation smoke omitted production topology or owner-scoped cleanup: {required}"
        );
    }
    for command in [
        "cargo run --manifest-path \"$fixture/rust-app/Cargo.toml\"",
        "cargo install --path \"$fixture/rust-bin\"",
        "npm pack \"$fixture/npm-bin\" --pack-destination \"$fixture\"",
        "npm install --global \"$fixture/gascan-npm-local-1.0.0.tgz\"",
        "\"$fixture/go.mod\"",
        "cd \"$fixture\" && go install ./go-bin",
        "python -m zipfile -c ../gascan_python_local-0.1.0-py3-none-any.whl",
        "\"$fixture/gascan_python_local-0.1.0-py3-none-any.whl\"",
        "gem build gascan-ruby-local.gemspec --output ../ruby-bin.gem",
        "gem install --local \"$fixture/ruby-bin.gem\"",
        "cfg-if = \\\"=1.0.4\\\"",
        "rustup component add rust-src",
        "rustup component list --installed",
    ] {
        assert!(
            smoke.contains(command),
            "workstation smoke omitted writable package-manager proof: {command}"
        );
    }
    assert!(
        smoke.contains("network_name=\"gascan-image-ws-network-test-$owner_token\""),
        "workstation network name must remain within Apple container's 64-character limit"
    );
    assert!(
        !smoke.contains("type=bind"),
        "credential-free workstation smoke must not mount the host checkout"
    );
    for forbidden in [
        "inspect=$(bounded_container volume inspect",
        "\"$fixture/go-bin/go.mod\"",
    ] {
        assert!(
            !smoke.contains(forbidden),
            "workstation smoke retained unsafe or invalid fixture wiring: {forbidden}"
        );
    }
    for command in [
        "vim --version",
        "nvim --version",
        "emacs --version",
        "pico --version",
        "claude --version",
        "codex --version",
        "pi --version",
        "herdr --version",
        "go version",
        "rustc --version",
        "cargo --version",
        "gh --version",
        "glab --version",
        "git --version",
        "ip -Version",
        "ss --version",
        "ping -V",
        "ifconfig --version",
        "netstat --version",
        "dig -v",
        "traceroute --version",
        "nc -h",
        "rg --version",
        "fd --version",
        "fzf --version",
        "tmux -V",
    ] {
        assert!(
            contract.contains(command),
            "guest contract omitted guaranteed command: {command}"
        );
    }
    for diagnostic in [
        "nslookup -version",
        "curl --fail --silent file:///etc/os-release",
        "wget --version",
        "rsync --version",
        "lsof -v",
        "file /bin/sh",
        "jq -e",
        "ps -o comm=",
        "top -b -n 1",
        "pstree -p",
        "tree --version",
        "less -F -X",
    ] {
        assert!(
            smoke.contains(diagnostic),
            "host-side workstation smoke omitted advertised diagnostic: {diagnostic}"
        );
    }
    for boundary in [
        "--network none",
        "/home/workspace/.local",
        "/home/workspace/.config",
        "/home/workspace/.cache",
        "/var/run/docker.sock",
        "/Library/Keychains",
        "CapEff:",
    ] {
        assert!(
            smoke.contains(boundary) || contract.contains(boundary),
            "workstation gate omitted security boundary: {boundary}"
        );
    }
}

#[test]
fn workstation_contract_matches_the_reviewed_ubuntu_fzf_version_format_exactly() {
    let contract = fs::read_to_string(
        repository_root().join("images/workspace/tests/workstation-contract.sh"),
    )
    .unwrap();
    let lock: toml::Value = toml::from_str(
        &fs::read_to_string(repository_root().join("images/workspace/versions.lock")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        lock["workstation_commands"]["fzf"].as_str(),
        Some("0.44.1 (debian)"),
        "reviewed Ubuntu fzf output must remain exact in the workstation lock"
    );
    assert!(
        contract.contains("expect_exact \"$(locked_version fzf)\" fzf --version"),
        "fzf must be checked against the exact locked Ubuntu output"
    );
    assert!(
        !contract.contains("expect_pattern '^fzf "),
        "fzf provenance must not be weakened to a broad version pattern"
    );
}

#[test]
fn ghcr_prebuilt_gate_inspects_canonical_name_and_smokes_original_reference() {
    let mut f = fixture();
    let reference = seed_valid_ghcr_receipt(&f);
    let canonical = format!("ghcr.io/liquescent-development/gascan/workspace@sha256:{DIGEST}");
    f.command.env("INSPECT_REFERENCE", &canonical);
    let output = f.command.arg("--prebuilt").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = fs::read_to_string(&f.calls).unwrap();
    assert!(calls.contains(&format!("container:image inspect {canonical}")));
    for prefix in ["user", "polyglot", "gascamp"] {
        assert!(calls.lines().any(|line| {
            line.contains(&format!(
                "create --name gascan-image-{prefix}-test-{TOKEN} "
            )) && line.ends_with(&reference)
        }));
    }
    assert_eq!(
        fs::read_to_string(
            f.root
                .join(".artifacts/connected-workspace-image-candidate.txt")
        )
        .unwrap(),
        format!("{reference}\n")
    );
    assert_no_publications(&f);
}

#[test]
fn invalid_arguments_skip_work_and_preserve_existing_approval() {
    for arguments in [["--unknown"].as_slice(), ["--prebuilt", "extra"].as_slice()] {
        let mut f = fixture();
        fs::write(
            f.root.join("docs/evidence/connected-workspace-image.md"),
            "status: `PASS`\n",
        )
        .unwrap();
        fs::write(f.root.join("images/workspace/approved-image.txt"), "stale").unwrap();
        let output = f.command.args(arguments).output().unwrap();
        assert!(!output.status.success(), "arguments={arguments:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("usage:"));
        assert!(!f.calls.exists(), "arguments={arguments:?} reached work");
        assert_eq!(
            fs::read_to_string(f.root.join("docs/evidence/connected-workspace-image.md")).unwrap(),
            "status: `PASS`\n"
        );
        assert_eq!(
            fs::read_to_string(f.root.join("images/workspace/approved-image.txt")).unwrap(),
            "stale"
        );
    }
}

#[test]
fn invalid_prebuilt_receipt_or_inspection_never_rebuilds_smokes_or_publishes() {
    for failure in [
        "missing-reference",
        "missing-receipt",
        "malformed",
        "mismatched",
        "mutable",
        "wrong-platform",
        "unavailable-image",
        "digest-mismatch",
    ] {
        let mut f = fixture();
        seed_valid_receipt(&f);
        fs::write(
            f.root.join("docs/evidence/connected-workspace-image.md"),
            "status: `PASS`\n",
        )
        .unwrap();
        fs::write(f.root.join("images/workspace/approved-image.txt"), "stale").unwrap();
        let artifacts = f.root.join(".artifacts");
        match failure {
            "missing-reference" => {
                fs::remove_file(artifacts.join("workspace-image-ref")).unwrap()
            }
            "missing-receipt" => {
                fs::remove_file(artifacts.join("workspace-image-build.json")).unwrap()
            }
            "malformed" => {
                fs::write(artifacts.join("workspace-image-build.json"), "{bad\n").unwrap()
            }
            "mismatched" => fs::write(
                artifacts.join("workspace-image-build.json"),
                "{\"reference\":\"gascan-workspace:other@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"}\n",
            )
            .unwrap(),
            "mutable" => {
                fs::write(
                    artifacts.join("workspace-image-ref"),
                    "gascan-workspace:d4964500a3295a33\n",
                )
                .unwrap()
            }
            "wrong-platform" => {
                f.command.env("IMAGE_PLATFORM", "amd64");
            }
            "unavailable-image" => {
                f.command.env("IMAGE_AVAILABLE", "0");
            }
            "digest-mismatch" => {
                f.command.env(
                    "IMAGE_DIGEST",
                    "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                );
            }
            _ => unreachable!(),
        }
        let output = f.command.arg("--prebuilt").output().unwrap();
        assert!(!output.status.success(), "failure={failure}");
        let calls = fs::read_to_string(&f.calls).unwrap_or_default();
        assert!(
            !calls
                .lines()
                .any(|line| line == "prefetch" || line == "build")
        );
        assert!(
            !calls.contains("container:create"),
            "failure={failure} reached smoke"
        );
        assert_eq!(
            fs::read_to_string(f.root.join("docs/evidence/connected-workspace-image.md")).unwrap(),
            "status: `PASS`\n"
        );
        assert_eq!(
            fs::read_to_string(f.root.join("images/workspace/approved-image.txt")).unwrap(),
            "stale"
        );
        assert!(
            !f.root
                .join(".artifacts/connected-workspace-image-candidate.txt")
                .exists()
        );
    }
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
fn every_failure_prevents_candidate_publication() {
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
        assert!(
            !f.root
                .join("docs/evidence/connected-workspace-image.md")
                .exists()
        );
        assert!(!f.root.join("images/workspace/approved-image.txt").exists());
        assert!(
            !f.root
                .join(".artifacts/connected-workspace-image-candidate.txt")
                .exists()
        );
    }
}

#[test]
fn existing_approval_is_preserved_on_failure_and_owner_token_is_never_candidate_evidence() {
    let mut f = fixture();
    fs::write(
        f.root.join("docs/evidence/connected-workspace-image.md"),
        "status: `PASS`\n",
    )
    .unwrap();
    fs::write(f.root.join("images/workspace/approved-image.txt"), "stale").unwrap();
    f.command.env("GASCAN_GATE_TEST_BUILD_FAILURE", "1");
    assert!(!f.command.status().unwrap().success());
    assert_eq!(
        fs::read_to_string(f.root.join("docs/evidence/connected-workspace-image.md")).unwrap(),
        "status: `PASS`\n"
    );
    assert_eq!(
        fs::read_to_string(f.root.join("images/workspace/approved-image.txt")).unwrap(),
        "stale"
    );

    let mut f = fixture();
    assert!(f.command.status().unwrap().success());
    let candidate = fs::read_to_string(
        f.root
            .join(".artifacts/connected-workspace-image-candidate.txt"),
    )
    .unwrap();
    assert!(!candidate.contains(TOKEN));
}

#[test]
fn existing_approval_is_preserved_when_obsolete_credential_input_is_rejected() {
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
    assert_eq!(
        fs::read_to_string(f.root.join("docs/evidence/connected-workspace-image.md")).unwrap(),
        "status: `PASS`\n"
    );
    assert_eq!(
        fs::read_to_string(f.root.join("images/workspace/approved-image.txt")).unwrap(),
        "stale"
    );
    assert!(!f.calls.exists());
}

#[test]
fn every_candidate_staging_boundary_removes_temporary_evidence() {
    for action in ["FAIL", "INT", "TERM"] {
        let mut f = fixture();
        f.command
            .env("GASCAN_GATE_TEST_CANDIDATE_BOUNDARY", "after-stage")
            .env("GASCAN_GATE_TEST_CANDIDATE_ACTION", action);
        let status = f.command.status().unwrap();
        assert!(!status.success(), "{action}");
        if action == "INT" {
            assert_eq!(status.code(), Some(130));
        }
        if action == "TERM" {
            assert_eq!(status.code(), Some(143));
        }
        assert_no_publications(&f);
        assert!(
            !f.root
                .join(".artifacts/connected-workspace-image-candidate.txt")
                .exists()
        );
        assert!(
            !fs::read_dir(f.root.join(".artifacts"))
                .unwrap()
                .any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".connected-workspace-image-candidate."))
        );
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
        assert!(
            !f.root
                .join("docs/evidence/connected-workspace-image.md")
                .exists()
        );
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
fn foreign_volume_replacement_between_checks_is_never_deleted() {
    let mut f = fixture();
    seed_valid_receipt(&f);
    let name = format!("gascan-image-workstation-tools-{TOKEN}");
    f.command
        .env("REPLACE_VOLUME_ON_SECOND_INSPECT", &name)
        .env("FAIL_WORKSTATION_EXEC", "1");
    assert!(!f.command.arg("--prebuilt").status().unwrap().success());
    let calls = fs::read_to_string(&f.calls).unwrap();
    assert!(
        calls
            .matches(&format!("container:volume inspect {name}"))
            .count()
            >= 2
    );
    assert!(!calls.contains(&format!("container:volume delete {name}")));
    assert!(
        f.temp
            .path()
            .join("state")
            .join(format!(".volume-{name}"))
            .exists()
    );
}

#[test]
fn workstation_volume_delete_failure_is_detected_by_exact_inventory_and_never_publishes() {
    let mut f = fixture();
    seed_valid_receipt(&f);
    let name = format!("gascan-image-workstation-cache-{TOKEN}");
    f.command.env("FAIL_VOLUME_DELETE", &name);
    assert!(!f.command.arg("--prebuilt").status().unwrap().success());
    let calls = fs::read_to_string(&f.calls).unwrap();
    assert!(calls.contains(&format!("container:volume delete {name}")));
    assert!(calls.contains("container:volume list --format json"));
    assert_no_publications(&f);
}

#[test]
fn polyglot_volume_is_recovered_after_local_delete_failure() {
    let mut f = fixture();
    seed_valid_receipt(&f);
    let name = format!("gascan-image-polyglot-tools-{TOKEN}");
    let output = f
        .command
        .env("FAIL_VOLUME_DELETE_ONCE", &name)
        .arg("--prebuilt")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let calls = fs::read_to_string(&f.calls).unwrap();
    assert!(
        calls
            .matches(&format!("container:volume delete {name}"))
            .count()
            >= 2,
        "outer cleanup did not retry the locally stranded polyglot volume"
    );
    assert!(
        !f.temp
            .path()
            .join("state")
            .join(format!(".volume-{name}"))
            .exists()
    );
    assert_no_publications(&f);
}

#[test]
fn polyglot_volume_is_recovered_after_local_attestation_failure() {
    let mut f = fixture();
    seed_valid_receipt(&f);
    let name = format!("gascan-image-polyglot-tools-{TOKEN}");
    let output = f
        .command
        .env("FAIL_VOLUME_ATTESTATION_TWICE", &name)
        .arg("--prebuilt")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let calls = fs::read_to_string(&f.calls).unwrap();
    assert!(calls.contains(&format!("container:volume delete {name}")));
    assert!(
        !f.temp
            .path()
            .join("state")
            .join(format!(".volume-{name}"))
            .exists()
    );
    assert_no_publications(&f);
}

#[test]
fn ssh_volume_is_recovered_after_smoke_failure() {
    let mut f = fixture();
    seed_valid_receipt(&f);
    let name = format!("gascan-image-ssh-config-{TOKEN}");
    fs::write(
        f.temp.path().join("state").join(format!(".volume-{name}")),
        "",
    )
    .unwrap();
    let output = f
        .command
        .env("FAIL_SMOKE", "ssh-contract.sh")
        .arg("--prebuilt")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let calls = fs::read_to_string(&f.calls).unwrap();
    assert!(calls.contains(&format!("container:volume delete {name}")));
    assert!(
        !f.temp
            .path()
            .join("state")
            .join(format!(".volume-{name}"))
            .exists()
    );
    assert_no_publications(&f);
}

#[test]
fn network_image_attestation_failure_is_recovered_without_silent_residue() {
    let mut f = fixture();
    seed_valid_receipt(&f);
    let name = format!("gascan-image-ws-network-test-{TOKEN}");
    let real_raw = f.temp.path().join("container-raw");
    let mismatch = f.temp.path().join("container-network-image-mismatch");
    executable(
        &mismatch,
        "#!/bin/sh\nset -eu\nif [ \"$1\" = inspect ] && [ \"$2\" = \"gascan-image-ws-network-test-$OWNER\" ]; then\n  \"$REAL_RAW_CONTAINER\" \"$@\" | sed 's/sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc/g'\n  exit 0\nfi\nexec \"$REAL_RAW_CONTAINER\" \"$@\"\n",
    );
    let output = f
        .command
        .env("RAW_CONTAINER", &mismatch)
        .env("REAL_RAW_CONTAINER", &real_raw)
        .arg("--prebuilt")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let calls = fs::read_to_string(&f.calls).unwrap();
    assert!(calls.contains(&format!("create --name {name} ")));
    assert!(calls.contains(&format!("stop --time 5 {name}")));
    assert!(calls.contains(&format!("delete {name}")));
    assert!(!f.temp.path().join("state").join(&name).exists());
    assert_no_publications(&f);
}

#[test]
fn foreign_network_attestation_failure_is_visible_and_never_deleted() {
    let mut f = fixture();
    seed_valid_receipt(&f);
    let name = format!("gascan-image-ws-network-test-{TOKEN}");
    assert!(
        !f.command
            .env("FOREIGN", &name)
            .arg("--prebuilt")
            .status()
            .unwrap()
            .success()
    );
    let calls = fs::read_to_string(&f.calls).unwrap();
    assert!(calls.contains(&format!("create --name {name} ")));
    assert!(calls.contains(&format!("inspect {name}")));
    assert!(!calls.contains(&format!("stop --time 5 {name}")));
    assert!(!calls.contains(&format!("delete {name}")));
    assert!(f.temp.path().join("state").join(&name).exists());
    assert_no_publications(&f);
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
        assert!(
            !f.root
                .join("docs/evidence/connected-workspace-image.md")
                .exists()
        );
        assert!(!f.root.join("images/workspace/approved-image.txt").exists());
        assert!(
            !f.temp
                .path()
                .join("state")
                .join(format!("gascan-image-user-test-{TOKEN}"))
                .exists()
        );
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
    assert!(
        !f.root
            .join("docs/evidence/connected-workspace-image.md")
            .exists()
    );
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
        assert!(
            !f.root
                .join("docs/evidence/connected-workspace-image.md")
                .exists()
        );
        assert!(!f.root.join("images/workspace/approved-image.txt").exists());
        assert!(
            !fs::read_dir(f.root.join("docs/evidence"))
                .unwrap()
                .any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".connected-workspace-image."))
        );
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
        assert!(
            !Command::new("kill")
                .args(["-0", pid])
                .status()
                .unwrap()
                .success()
        );
    }
    assert!(
        !f.root
            .join("docs/evidence/connected-workspace-image.md")
            .exists()
    );
    assert!(!f.root.join("images/workspace/approved-image.txt").exists());
}

#[test]
fn inspect_failure_never_proves_absence_without_authoritative_inventory() {
    for inventory in ["present", "error", "malformed", "timeout"] {
        let mut f = fixture();
        let wrapper = f.temp.path().join("inventory-container");
        executable(
            &wrapper,
            "#!/bin/sh\nset -eu\nif [ \"$1\" = inspect ] && [ ! -f \"$STATE/$2\" ]; then exit 77; fi\nif [ \"$1\" = list ]; then case \"$INVENTORY_MODE\" in present) name=gascan-image-user-test-00112233445566778899aabbccddeeff; printf '[{\"id\":\"%s\",\"configuration\":{\"id\":\"%s\",\"labels\":{}}}]\\n' \"$name\" \"$name\" ;; error) exit 78 ;; malformed) printf '{bad\\n' ;; timeout) printf '%s\\n' $$ >>\"$BLOCKED_PIDS\"; trap '' INT TERM; while :; do sleep 1; done ;; esac; exit 0; fi\nexec \"$REAL_CONTAINER\" \"$@\"\n",
        );
        let pids = f.temp.path().join("blocked-pids");
        f.command
            .env("CONTAINER_BIN", wrapper)
            .env("REAL_CONTAINER", f.temp.path().join("container"))
            .env("INVENTORY_MODE", inventory)
            .env("BLOCKED_PIDS", &pids)
            .env("GASCAN_GATE_CLI_TIMEOUT_SECONDS", "1");
        let output = f.command.output().unwrap();
        assert!(!output.status.success(), "inventory={inventory}");
        if inventory == "present" {
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains("exact container remains in inventory"),
                "native presence failed for the wrong reason: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert!(
            !f.root
                .join("docs/evidence/connected-workspace-image.md")
                .exists()
        );
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
