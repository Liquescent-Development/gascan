use camino::Utf8Path;
use gascan_apple::AppleCommandBuilder;
use gascan_core::manifest::Manifest;
use gascan_core::policy::{ControlPlanePolicy, PolicyCompiler};
use gascan_core::runtime::{
    NetworkIsolation, RecreateRequest, ResourceIdentity, ResourceKind, ResourceOwnership,
    RetainedResources, RuntimeCapabilities, RuntimeResource, RuntimeVersion,
};
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

fn ssh_request() -> (tempfile::TempDir, gascan_core::runtime::CreateRequest) {
    let temp = tempfile::tempdir().expect("temporary SSH translation root");
    let root = Utf8Path::from_path(temp.path()).expect("UTF-8 temporary path");
    std::fs::write(
        root.join("gascan.toml"),
        "version = 1\nnetwork = 'networked'\n",
    )
    .expect("write SSH translation manifest");
    let manifest = Manifest::load(root).expect("load SSH translation manifest");
    let spec =
        SandboxSpec::from_root("ssh", root, manifest).expect("build sealed SSH sandbox spec");
    let request = PolicyCompiler::compile_with_control_plane(
        spec,
        &capabilities(),
        ControlPlanePolicy {
            ssh_authorized_key: Some(
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEB",
            ),
            ssh_host_port: Some(22222),
        },
    )
    .expect("compile SSH policy");
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
fn networked_create_uses_the_managed_network_and_loopback_publish() {
    let (_root, request) = request(
        "web",
        "version = 1\nnetwork = 'networked'\n[ports]\nweb = 3000\n",
    );
    let expected_network = request.network().managed_name().unwrap();
    let spec = AppleCommandBuilder::create(&request).unwrap();

    assert!(
        spec.args
            .windows(2)
            .any(|pair| pair == ["--publish", "127.0.0.1:3000:3000"])
    );
    assert!(
        spec.args
            .windows(2)
            .any(|pair| pair[0] == "--network" && pair[1] == expected_network)
    );
    assert!(
        !spec
            .args
            .windows(2)
            .any(|pair| pair == ["--network", "default"])
    );
}

#[test]
fn networked_ssh_create_publishes_guest_port_22_only_on_ipv4_loopback() {
    let (_root, request) = ssh_request();
    let spec = AppleCommandBuilder::create(&request).expect("translate approved SSH request");

    assert!(
        spec.args
            .windows(2)
            .any(|pair| pair == ["--publish", "127.0.0.1:22222:22"])
    );
    assert!(!spec.args.iter().any(|argument| {
        argument.contains("0.0.0.0:22222:22") || argument.contains("[::1]:22222:22")
    }));
}

#[test]
fn retained_create_uses_the_validated_topology_without_resource_create_commands() {
    let (_root, create) = request("retained-create", "version = 1\nnetwork = 'networked'\n");
    let mut resources = create
        .volumes()
        .iter()
        .map(|volume| {
            RuntimeResource::discovered(
                ResourceIdentity::new(ResourceKind::Volume, volume.name.clone()).unwrap(),
                Some(create.id().clone()),
                ResourceOwnership::GasCanOwned,
            )
        })
        .collect::<Vec<_>>();
    resources.push(RuntimeResource::discovered(
        ResourceIdentity::new(
            ResourceKind::Network,
            create.network().managed_name().unwrap(),
        )
        .unwrap(),
        Some(create.id().clone()),
        ResourceOwnership::GasCanOwned,
    ));
    let retained = RetainedResources::new(&create, resources).unwrap();
    let recreate = RecreateRequest::new(create.clone(), retained).unwrap();

    assert_eq!(
        AppleCommandBuilder::create_with_retained(&recreate).unwrap(),
        AppleCommandBuilder::create(&create).unwrap()
    );
}

#[test]
fn mutable_image_references_are_rejected_with_a_typed_error() {
    let error = AppleCommandBuilder::pull("ghcr.io/gascan/workspace:latest")
        .expect_err("mutable image must fail closed");
    assert_eq!(error.code(), "missing_image_digest");
}
