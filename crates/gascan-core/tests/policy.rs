use camino::{Utf8Path, Utf8PathBuf};
use gascan_core::manifest::Manifest;
use gascan_core::policy::{
    DEFAULT_CPUS, DEFAULT_DISK_BYTES, DEFAULT_MEMORY_BYTES, DEFAULT_PROCESS_COUNT, MAX_CPUS,
    MAX_DISK_BYTES, MAX_MEMORY_BYTES, PolicyCompiler, filtered_host_environment,
};
use gascan_core::runtime::{
    NetworkIsolation, ResourceKind, RuntimeCapabilities, RuntimeNetwork, RuntimeUser,
    RuntimeVersion,
};
use gascan_core::sandbox::{SandboxSpec, WORKSPACE_TARGET};
use serde_json::Value;
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};

fn capabilities() -> RuntimeCapabilities {
    RuntimeCapabilities {
        version: RuntimeVersion::new(1, 0, 0),
        bind_mounts: true,
        named_volumes: true,
        tty: true,
        signals: true,
        loopback_publish: true,
        resource_limits: true,
        offline: NetworkIsolation::Proven,
    }
}

fn spec(source: &str) -> (tempfile::TempDir, SandboxSpec) {
    let temp = tempfile::tempdir().expect("temporary policy root");
    let root = Utf8Path::from_path(temp.path()).expect("UTF-8 temporary path");
    std::fs::write(root.join("gascan.toml"), source).expect("write policy manifest");
    let manifest = Manifest::load(root).expect("load policy manifest");
    let spec = SandboxSpec::from_root("policy", root, manifest).expect("build sandbox spec");
    (temp, spec)
}

#[test]
fn offline_requires_proven_isolation_before_compilation() {
    let (_temp, spec) = spec("version = 1\nnetwork = 'offline'\n");
    for offline in [NetworkIsolation::Unsupported, NetworkIsolation::Unverified] {
        let mut capabilities = capabilities();
        capabilities.offline = offline;
        let error = PolicyCompiler::compile(spec.clone(), &capabilities)
            .expect_err("offline must fail closed");
        assert_eq!(error.code(), "offline_unavailable");
    }
}

#[test]
fn every_mandatory_request_capability_fails_closed() {
    let (_temp, offline) = spec("version = 1\n");
    let (_temp_networked, networked) = spec("version = 1\nnetwork = 'networked'\n");
    let (_temp_port, with_port) = spec("version = 1\nnetwork = 'networked'\n[ports]\nweb = 3000\n");

    let mut missing_mounts = capabilities();
    missing_mounts.bind_mounts = false;
    assert_eq!(
        PolicyCompiler::compile(offline.clone(), &missing_mounts)
            .expect_err("mount capability is mandatory")
            .code(),
        "bind_mounts_unavailable"
    );

    let mut missing_volumes = capabilities();
    missing_volumes.named_volumes = false;
    assert_eq!(
        PolicyCompiler::compile(networked.clone(), &missing_volumes)
            .expect_err("volume capability is mandatory")
            .code(),
        "named_volumes_unavailable"
    );

    let mut missing_resources = capabilities();
    missing_resources.resource_limits = false;
    assert_eq!(
        PolicyCompiler::compile(networked, &missing_resources)
            .expect_err("resource controls are mandatory")
            .code(),
        "resource_limits_unavailable"
    );

    let mut missing_loopback = capabilities();
    missing_loopback.loopback_publish = false;
    assert_eq!(
        PolicyCompiler::compile(with_port, &missing_loopback)
            .expect_err("declared ports require loopback publishing")
            .code(),
        "loopback_publish_unavailable"
    );
}

#[test]
fn host_environment_has_a_fixed_allowlist() {
    let environment = filtered_host_environment([
        ("TERM", "xterm-256color"),
        ("COLORTERM", "truecolor"),
        ("LANG", "en_US.UTF-8"),
        ("LC_ALL", "C"),
        ("LC_", "invalid"),
        ("AWS_SECRET_ACCESS_KEY", "secret"),
        ("SSH_AUTH_SOCK", "/private/socket"),
        ("HOME", "/Users/person"),
        ("PATH", "/host/bin"),
    ]);

    assert_eq!(
        environment.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["COLORTERM", "LANG", "LC_ALL", "TERM"]
    );
    assert!(!environment.values().any(|value| value.contains("secret")));
}

#[test]
fn canonical_request_has_one_root_mount_owned_volumes_loopback_ports_and_init() {
    let (_temp, spec) = spec(
        "version = 1\nnetwork = 'networked'\nuser = 'root'\n[ports]\napi = 8080\nweb = 3000\n",
    );
    let root = spec.canonical_root().to_owned();
    let id = spec.id().clone();

    let request = PolicyCompiler::compile(spec, &capabilities()).expect("compile valid policy");
    assert_eq!(request.id(), &id);
    assert_eq!(request.bind_mounts().len(), 1);
    assert_eq!(request.bind_mounts()[0].source, root);
    assert_eq!(
        request.bind_mounts()[0].target,
        Utf8PathBuf::from(WORKSPACE_TARGET)
    );
    assert!(request.bind_mounts()[0].writable);
    assert_eq!(request.network(), RuntimeNetwork::Networked);
    assert_eq!(request.user(), RuntimeUser::Root);
    assert!(request.init());
    assert_eq!(request.ownership().managed_by, "gascan");
    assert_eq!(request.ownership().sandbox_id, id);
    assert_eq!(request.volumes().len(), 3);
    assert!(request.volumes().iter().all(|volume| {
        volume.writable
            && volume.name.starts_with("gascan-")
            && &volume.ownership == request.ownership()
            && volume.target.starts_with("/home/workspace")
    }));
    assert_eq!(request.ports().len(), 2);
    assert!(request.ports().iter().all(|port| {
        port.host_address == IpAddr::V4(Ipv4Addr::LOCALHOST) && port.host_port == port.guest_port
    }));
}

#[test]
fn expected_resource_identities_are_derived_from_the_sealed_sandbox_id() {
    let id = gascan_core::sandbox::SandboxId::test("expected-resources");

    let identities = PolicyCompiler::expected_resource_identities(&id).unwrap();

    assert_eq!(identities.len(), 4);
    assert_eq!(identities[0].kind(), ResourceKind::Container);
    assert_eq!(identities[0].name(), id.as_str());
    assert_eq!(
        identities
            .iter()
            .skip(1)
            .map(|identity| (identity.kind(), identity.name()))
            .collect::<Vec<_>>(),
        [
            (ResourceKind::Volume, format!("gascan-mise-{id}")),
            (ResourceKind::Volume, format!("gascan-cache-{id}")),
            (ResourceKind::Volume, format!("gascan-config-{id}")),
        ]
        .iter()
        .map(|(kind, name)| (*kind, name.as_str()))
        .collect::<Vec<_>>()
    );
}

#[test]
fn image_reference_is_an_immutable_digest() {
    let (_temp, spec) = spec("version = 1\nnetwork = 'networked'\n");
    let request = PolicyCompiler::compile(spec, &capabilities()).expect("compile valid policy");
    let (_, digest) = request
        .image()
        .split_once("@sha256:")
        .expect("digest image reference");
    assert_eq!(digest.len(), 64);
    assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert!(!request.image().contains(":latest"));
}

#[test]
fn safe_resource_defaults_and_explicit_values_are_bounded() {
    let (_temp, default_spec) = spec("version = 1\nnetwork = 'networked'\n");
    let defaults = PolicyCompiler::compile(default_spec, &capabilities())
        .expect("compile defaults")
        .resources()
        .to_owned();
    assert_eq!(defaults.cpus, Some(DEFAULT_CPUS));
    assert_eq!(defaults.memory_bytes, Some(DEFAULT_MEMORY_BYTES));
    assert_eq!(defaults.disk_bytes, Some(DEFAULT_DISK_BYTES));
    assert_eq!(defaults.process_count, Some(DEFAULT_PROCESS_COUNT));

    let source = format!(
        "version = 1\nnetwork = 'networked'\n[resources]\ncpus = {MAX_CPUS}\nmemory = '{}GiB'\ndisk = '{}GiB'\n",
        MAX_MEMORY_BYTES / 1024_u64.pow(3),
        MAX_DISK_BYTES / 1024_u64.pow(3)
    );
    let (_temp_max, max_spec) = spec(&source);
    let maximum = PolicyCompiler::compile(max_spec, &capabilities())
        .expect("accept documented maxima")
        .resources()
        .to_owned();
    assert_eq!(maximum.cpus, Some(MAX_CPUS));
    assert_eq!(maximum.memory_bytes, Some(MAX_MEMORY_BYTES));
    assert_eq!(maximum.disk_bytes, Some(MAX_DISK_BYTES));
}

#[test]
fn resources_above_any_maximum_are_rejected() {
    for (source, code) in [
        ("[resources]\ncpus = 17\n", "cpus_exceed_maximum"),
        ("[resources]\nmemory = '65GiB'\n", "memory_exceeds_maximum"),
        ("[resources]\ndisk = '513GiB'\n", "disk_exceeds_maximum"),
    ] {
        let manifest = format!("version = 1\nnetwork = 'networked'\n{source}");
        let (_temp, spec) = spec(&manifest);
        assert_eq!(
            PolicyCompiler::compile(spec, &capabilities())
                .expect_err("resource maximum must be enforced")
                .code(),
            code
        );
    }
}

#[test]
fn zero_and_duplicate_published_ports_are_rejected() {
    for (source, code) in [
        ("[ports]\ninvalid = 0\n", "invalid_port"),
        ("[ports]\nfirst = 3000\nsecond = 3000\n", "duplicate_port"),
    ] {
        let manifest = format!("version = 1\nnetwork = 'networked'\n{source}");
        let (_temp, spec) = spec(&manifest);
        assert_eq!(
            PolicyCompiler::compile(spec, &capabilities())
                .expect_err("unsafe port declaration must fail")
                .code(),
            code
        );
    }
}

#[test]
fn offline_policy_cannot_publish_ports() {
    let (_temp, spec) = spec("version = 1\nnetwork = 'offline'\n[ports]\nweb = 3000\n");
    assert_eq!(
        PolicyCompiler::compile(spec, &capabilities())
            .expect_err("offline and published ports conflict")
            .code(),
        "offline_ports_forbidden"
    );
}

#[test]
fn approved_json_shape_exposes_no_unsafe_backend_surface() {
    let (_temp, spec) = spec("version = 1\nnetwork = 'networked'\n[ports]\nweb = 3000\n");
    let request = PolicyCompiler::compile(spec, &capabilities()).expect("compile snapshot");
    let mut value = serde_json::to_value(&request).expect("serialize request");
    value["bind_mounts"][0]["source"] = Value::String("$CANONICAL_ROOT".to_owned());
    let snapshot = serde_json::to_string_pretty(&value).expect("render snapshot");
    let keys = value
        .as_object()
        .expect("request JSON object")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        keys,
        vec![
            "bind_mounts",
            "environment",
            "id",
            "image",
            "init",
            "network",
            "ownership",
            "ports",
            "resources",
            "user",
            "volumes"
        ]
    );
    for forbidden in [
        "/Users/",
        "AWS_",
        "SSH_AUTH_SOCK",
        "socket",
        "credential",
        "device",
        "privileged",
        "backend",
        "raw_options",
    ] {
        assert!(
            !snapshot.contains(forbidden),
            "snapshot contains {forbidden}: {snapshot}"
        );
    }
    assert_eq!(request.environment(), &BTreeMap::new());
}
