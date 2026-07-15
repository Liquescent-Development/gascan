use camino::Utf8Path;
use gascan_apple::AppleCommandBuilder;
use gascan_core::manifest::Manifest;
use gascan_core::policy::PolicyCompiler;
use gascan_core::runtime::{NetworkIsolation, RuntimeCapabilities, RuntimeVersion};
use gascan_core::sandbox::{SandboxId, SandboxSpec};

fn capabilities() -> RuntimeCapabilities {
    RuntimeCapabilities {
        version: RuntimeVersion::new(1, 1, 0),
        bind_mounts: true,
        named_volumes: true,
        tty: true,
        signals: true,
        loopback_publish: true,
        resource_limits: true,
        offline: NetworkIsolation::Proven,
    }
}

fn request(name: &str, manifest: &str) -> (tempfile::TempDir, gascan_core::runtime::CreateRequest) {
    let temp = tempfile::tempdir().expect("temporary translation root");
    let root = Utf8Path::from_path(temp.path()).expect("UTF-8 temporary path");
    std::fs::write(root.join("gascan.toml"), manifest).expect("write translation manifest");
    let manifest = Manifest::load(root).expect("load translation manifest");
    let spec = SandboxSpec::from_root(name, root, manifest).expect("build sealed sandbox spec");
    let request = PolicyCompiler::compile(spec, &capabilities()).expect("compile policy");
    (temp, request)
}

#[test]
fn pull_and_inspect_use_literal_argument_vectors() {
    let image = "ghcr.io/gascan/workspace@sha256:7c45e19c71c72fdacf28ef794c6f4eaf3d14fc5216e82c5a7230030996b8d59b";
    assert_eq!(
        AppleCommandBuilder::pull(image).expect("immutable image"),
        gascan_apple::CommandSpec::new("container", ["image", "pull", image])
    );
    let id = SandboxId::test("inspect");
    assert_eq!(
        AppleCommandBuilder::inspect(&id),
        gascan_apple::CommandSpec::new("container", ["inspect", id.as_str()])
    );
}

#[test]
fn create_uses_one_workspace_mount_offline_mode_and_owned_volumes() {
    let (_root, request) = request("code", "version = 1\nnetwork = 'offline'\n");
    let source = &request.bind_mounts()[0].source;
    let id = request.id().as_str();
    let image = request.image();
    let expected: Vec<String> =
        serde_json::from_str(include_str!("fixtures/translate-create-offline.json"))
            .expect("valid literal argv fixture");
    let expected = expected
        .into_iter()
        .map(|arg| {
            arg.replace("$ID", id)
                .replace("$ROOT", source.as_str())
                .replace("$IMAGE", image)
        })
        .collect::<Vec<_>>();
    let spec = AppleCommandBuilder::create(&request).expect("translate approved request");
    assert_eq!(spec.program, "container");
    assert_eq!(spec.args, expected);
    assert!(!spec.args.join(" ").contains("/Users/tester"));
}

#[test]
fn networked_create_publishes_only_ipv4_loopback() {
    let (_root, request) = request(
        "web",
        "version = 1\nnetwork = 'networked'\n[ports]\nweb = 3000\n",
    );
    let spec = AppleCommandBuilder::create(&request).expect("translate networked request");
    assert!(
        spec.args
            .windows(2)
            .any(|pair| pair == ["--publish", "127.0.0.1:3000:3000"])
    );
    assert!(!spec.args.iter().any(|arg| arg == "--network"));
}

#[test]
fn mutable_image_references_are_rejected_with_a_typed_error() {
    let error = AppleCommandBuilder::pull("ghcr.io/gascan/workspace:latest")
        .expect_err("mutable image must fail closed");
    assert_eq!(error.code(), "missing_image_digest");
}
