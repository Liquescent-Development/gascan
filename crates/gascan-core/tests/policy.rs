use camino::{Utf8Path, Utf8PathBuf};
use gascan_core::manifest::Manifest;
use gascan_core::policy::{
    ControlPlanePolicy, DEFAULT_CPUS, DEFAULT_MEMORY_BYTES, MAX_CPUS, MAX_MEMORY_BYTES,
    PolicyCompiler, filtered_host_environment,
};
use gascan_core::runtime::{
    NetworkIsolation, ResourceKind, RuntimeCapabilities, RuntimeNetwork, RuntimeUser,
    RuntimeVersion,
};
use gascan_core::sandbox::{SandboxSpec, WORKSPACE_TARGET};
use serde_json::Value;
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};

const SSH_PUBLIC_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEB";

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

fn compile_workspace_request() -> gascan_core::runtime::CreateRequest {
    let (_temp, spec) = spec("version = 1\nnetwork = 'networked'\n");
    PolicyCompiler::compile(spec, &capabilities()).expect("compile workspace request")
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
fn ssh_control_plane_appends_one_loopback_native_port_after_application_ports() {
    let private_key = "-----BEGIN OPENSSH PRIVATE KEY-----";
    let (_temp, spec) = spec("version = 1\nnetwork = 'networked'\n[ports]\nweb = 3000\n");
    let host_path = spec.canonical_root().as_str().to_owned();

    let request = PolicyCompiler::compile_with_control_plane(
        spec,
        &capabilities(),
        ControlPlanePolicy {
            ssh_authorized_key: Some(SSH_PUBLIC_KEY),
            ssh_host_port: Some(22222),
        },
    )
    .expect("compile SSH control-plane policy");

    assert_eq!(
        request.ports(),
        [
            gascan_core::runtime::RuntimePort {
                host_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
                host_port: 3000,
                guest_port: 3000,
            },
            gascan_core::runtime::RuntimePort {
                host_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
                host_port: 22222,
                guest_port: 22,
            },
        ]
    );
    assert_eq!(
        request
            .environment()
            .get("GASCAN_SSH_AUTHORIZED_KEY")
            .map(String::as_str),
        Some(SSH_PUBLIC_KEY)
    );
    assert_eq!(
        request
            .environment()
            .get("GASCAN_SSH_ENABLED")
            .map(String::as_str),
        Some("1")
    );
    assert!(
        request.environment().values().all(|value| {
            !value.contains(private_key) && !value.contains("22222") && !value.contains(&host_path)
        }),
        "guest environment excludes private keys, host ports, and host paths"
    );
}

#[test]
fn explicit_ssh_host_port_accepts_the_same_control_plane_port() {
    let (_temp, spec) = spec("version = 1\nnetwork = 'networked'\n[ssh]\nhost_port = 2222\n");
    let request = PolicyCompiler::compile_with_control_plane(
        spec,
        &capabilities(),
        ControlPlanePolicy {
            ssh_authorized_key: Some(SSH_PUBLIC_KEY),
            ssh_host_port: Some(2222),
        },
    )
    .expect("matching explicit SSH host port compiles");

    assert_eq!(
        request.ports(),
        [gascan_core::runtime::RuntimePort {
            host_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            host_port: 2222,
            guest_port: 22,
        }]
    );
}

#[test]
fn explicit_ssh_host_port_rejects_a_different_control_plane_port() {
    let (_temp, spec) = spec("version = 1\nnetwork = 'networked'\n[ssh]\nhost_port = 2222\n");
    let error = PolicyCompiler::compile_with_control_plane(
        spec,
        &capabilities(),
        ControlPlanePolicy {
            ssh_authorized_key: Some(SSH_PUBLIC_KEY),
            ssh_host_port: Some(22222),
        },
    )
    .expect_err("control plane must preserve an explicit manifest SSH port");

    assert_eq!(error.code(), "ssh_host_port_mismatch");
    assert_eq!(
        error.to_string(),
        "control-plane SSH host port 22222 does not match manifest SSH host port 2222"
    );
}

#[test]
fn omitted_ssh_host_port_accepts_an_automatic_control_plane_port() {
    let (_temp, spec) = spec("version = 1\nnetwork = 'networked'\n");
    let request = PolicyCompiler::compile_with_control_plane(
        spec,
        &capabilities(),
        ControlPlanePolicy {
            ssh_authorized_key: Some(SSH_PUBLIC_KEY),
            ssh_host_port: Some(22222),
        },
    )
    .expect("automatic SSH host port compiles");

    assert_eq!(
        request.ports(),
        [gascan_core::runtime::RuntimePort {
            host_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            host_port: 22222,
            guest_port: 22,
        }]
    );
}

#[test]
fn offline_and_disabled_ssh_policy_emit_no_key_or_native_port() {
    let (_temp_offline, offline) = spec("version = 1\nnetwork = 'offline'\n");
    let offline_request = PolicyCompiler::compile_with_control_plane(
        offline,
        &capabilities(),
        ControlPlanePolicy {
            ssh_authorized_key: Some(SSH_PUBLIC_KEY),
            ssh_host_port: Some(22222),
        },
    )
    .expect("offline SSH control policy compiles without a runtime port");
    assert!(offline_request.ports().is_empty());
    assert_eq!(
        offline_request
            .environment()
            .get("GASCAN_SSH_ENABLED")
            .map(String::as_str),
        Some("0")
    );
    assert!(
        !offline_request
            .environment()
            .contains_key("GASCAN_SSH_AUTHORIZED_KEY")
    );

    let (_temp_disabled, disabled) =
        spec("version = 1\nnetwork = 'networked'\n[ssh]\nenabled = false\n");
    let disabled_request = PolicyCompiler::compile_with_control_plane(
        disabled,
        &capabilities(),
        ControlPlanePolicy {
            ssh_authorized_key: Some(SSH_PUBLIC_KEY),
            ssh_host_port: Some(22222),
        },
    )
    .expect("disabled SSH control policy compiles");
    assert!(disabled_request.ports().is_empty());
    assert_eq!(
        disabled_request
            .environment()
            .get("GASCAN_SSH_ENABLED")
            .map(String::as_str),
        Some("0")
    );
    assert!(
        !disabled_request
            .environment()
            .contains_key("GASCAN_SSH_AUTHORIZED_KEY")
    );
}

#[test]
fn enabled_ssh_control_plane_requires_both_key_and_host_port() {
    for (control, expected_code) in [
        (
            ControlPlanePolicy {
                ssh_authorized_key: Some(SSH_PUBLIC_KEY),
                ssh_host_port: None,
            },
            "missing_ssh_host_port",
        ),
        (
            ControlPlanePolicy {
                ssh_authorized_key: None,
                ssh_host_port: Some(22222),
            },
            "missing_ssh_authorized_key",
        ),
    ] {
        let (_temp, spec) = spec("version = 1\nnetwork = 'networked'\n");
        let error = PolicyCompiler::compile_with_control_plane(spec, &capabilities(), control)
            .expect_err("enabled SSH requires complete control-plane inputs");
        assert_eq!(error.code(), expected_code);
    }
}

#[test]
fn legacy_compilers_leave_default_networked_ssh_disabled() {
    let (_temp, compile_spec) = spec("version = 1\nnetwork = 'networked'\n");
    let (_temp_image, image_spec) = spec("version = 1\nnetwork = 'networked'\n");
    let requests = [
        PolicyCompiler::compile(compile_spec, &capabilities()).expect("legacy compile"),
        PolicyCompiler::compile_for_image(
            image_spec,
            &capabilities(),
            PolicyCompiler::workspace_image(),
        )
        .expect("legacy compile for image"),
    ];

    for request in requests {
        assert!(request.ports().is_empty());
        assert_eq!(
            request
                .environment()
                .get("GASCAN_SSH_ENABLED")
                .map(String::as_str),
            Some("0")
        );
        assert!(
            !request
                .environment()
                .contains_key("GASCAN_SSH_AUTHORIZED_KEY")
        );
    }
}

#[test]
fn ssh_host_port_cannot_collide_with_an_application_port() {
    let (_temp, spec) = spec("version = 1\nnetwork = 'networked'\n[ports]\nweb = 22222\n");
    let error = PolicyCompiler::compile_with_control_plane(
        spec,
        &capabilities(),
        ControlPlanePolicy {
            ssh_authorized_key: Some(SSH_PUBLIC_KEY),
            ssh_host_port: Some(22222),
        },
    )
    .expect_err("SSH must not reuse an application host port");

    assert_eq!(error.code(), "duplicate_port");
}

#[test]
fn ssh_control_plane_rejects_zero_and_privileged_host_ports() {
    for host_port in [0, 1023] {
        let (_temp, spec) = spec("version = 1\nnetwork = 'networked'\n");
        let error = PolicyCompiler::compile_with_control_plane(
            spec,
            &capabilities(),
            ControlPlanePolicy {
                ssh_authorized_key: Some(SSH_PUBLIC_KEY),
                ssh_host_port: Some(host_port),
            },
        )
        .expect_err("SSH control-plane ports must be unprivileged");

        assert_eq!(error.code(), "invalid_ssh_host_port");
    }
}

#[test]
fn ssh_control_plane_rejects_private_key_material() {
    let (_temp, spec) = spec("version = 1\nnetwork = 'networked'\n");
    let error = PolicyCompiler::compile_with_control_plane(
        spec,
        &capabilities(),
        ControlPlanePolicy {
            ssh_authorized_key: Some("-----BEGIN OPENSSH PRIVATE KEY-----"),
            ssh_host_port: Some(22222),
        },
    )
    .expect_err("private key material must never enter the guest environment");

    assert_eq!(error.code(), "invalid_ssh_authorized_key");
}

#[test]
fn ssh_control_plane_rejects_private_material_after_a_public_key_record() {
    let (_temp, spec) = spec("version = 1\nnetwork = 'networked'\n");
    let error = PolicyCompiler::compile_with_control_plane(
        spec,
        &capabilities(),
        ControlPlanePolicy {
            ssh_authorized_key: Some(
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEB\n-----BEGIN OPENSSH PRIVATE KEY-----",
            ),
            ssh_host_port: Some(22222),
        },
    )
    .expect_err("private key material following a public key record must be rejected");

    assert_eq!(error.code(), "invalid_ssh_authorized_key");
}

#[test]
fn ssh_control_plane_rejects_a_private_key_blob_disguised_as_ed25519() {
    let (_temp, spec) = spec("version = 1\nnetwork = 'networked'\n");
    let error = PolicyCompiler::compile_with_control_plane(
        spec,
        &capabilities(),
        ControlPlanePolicy {
            ssh_authorized_key: Some("ssh-ed25519 b3BlbnNzaC1rZXktdjEAAAA="),
            ssh_host_port: Some(22222),
        },
    )
    .expect_err("an OpenSSH private-key blob is not an authorized public key");

    assert_eq!(error.code(), "invalid_ssh_authorized_key");
}

#[test]
fn canonical_request_has_one_root_mount_owned_volumes_loopback_ports_and_init() {
    let (_temp, spec) = spec(
        "version = 1\nnetwork = 'networked'\nuser = 'root'\n[storage]\ntools = '11GiB'\ncache = '12GiB'\nconfig = '2GiB'\n[ports]\napi = 8080\nweb = 3000\n",
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
    assert!(matches!(
        request.network(),
        RuntimeNetwork::Networked { .. }
    ));
    assert_eq!(request.user(), RuntimeUser::Root);
    assert!(request.init());
    assert_eq!(request.ownership().managed_by, "gascan");
    assert_eq!(request.ownership().sandbox_id, id);
    assert_eq!(request.volumes().len(), 3);
    assert_eq!(
        request
            .volumes()
            .iter()
            .map(|volume| volume.target.as_str())
            .collect::<Vec<_>>(),
        [
            "/home/workspace/.local/share/mise",
            "/home/workspace/.cache",
            "/home/workspace/.config/gascan",
        ]
    );
    assert!(request.volumes().iter().all(|volume| {
        volume.writable
            && volume.name.starts_with("gascan-")
            && &volume.ownership == request.ownership()
    }));
    let capacities = request
        .volumes()
        .iter()
        .map(|volume| (volume.target.as_str(), volume.capacity_bytes))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        capacities["/home/workspace/.local/share/mise"],
        11 * 1024_u64.pow(3)
    );
    assert_eq!(capacities["/home/workspace/.cache"], 12 * 1024_u64.pow(3));
    assert_eq!(
        capacities["/home/workspace/.config/gascan"],
        2 * 1024_u64.pow(3)
    );
    assert_eq!(
        request.environment(),
        &BTreeMap::from([
            ("GASCAN_SSH_ENABLED".to_owned(), "0".to_owned()),
            ("HOME".to_owned(), "/home/workspace".to_owned()),
            (
                "MISE_CACHE_DIR".to_owned(),
                "/home/workspace/.cache/mise".to_owned(),
            ),
            (
                "MISE_DATA_DIR".to_owned(),
                "/home/workspace/.local/share/mise".to_owned(),
            ),
            (
                "MISE_GLOBAL_CONFIG_FILE".to_owned(),
                "/home/workspace/.config/gascan/mise.toml".to_owned(),
            ),
            (
                "MISE_STATE_DIR".to_owned(),
                "/home/workspace/.config/gascan/mise-state".to_owned(),
            ),
            (
                "MISE_SYSTEM_DATA_DIR".to_owned(),
                "/opt/gascan/mise".to_owned(),
            ),
            (
                "PATH".to_owned(),
                "/home/workspace/.local/share/mise/shims:/opt/gascan/mise/shims:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_owned(),
            ),
        ])
    );
    assert_eq!(request.ports().len(), 2);
    assert!(request.ports().iter().all(|port| {
        port.host_address == IpAddr::V4(Ipv4Addr::LOCALHOST) && port.host_port == port.guest_port
    }));
}

#[test]
fn expected_resource_identities_are_derived_from_the_sealed_sandbox_id() {
    let id = gascan_core::sandbox::SandboxId::test("expected-resources");

    let identities = PolicyCompiler::expected_resource_identities(&id).unwrap();

    assert_eq!(identities.len(), 5);
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
            (ResourceKind::Network, format!("gascan-network-{id}")),
        ]
        .iter()
        .map(|(kind, name)| (*kind, name.as_str()))
        .collect::<Vec<_>>()
    );
}

#[test]
fn expected_resource_identities_include_the_managed_network() {
    let id = gascan_core::sandbox::SandboxId::test("expected-network");
    let identities = PolicyCompiler::expected_resource_identities(&id).unwrap();
    let network = identities
        .iter()
        .find(|identity| identity.kind() == ResourceKind::Network)
        .expect("managed network identity");

    assert_eq!(network.name(), PolicyCompiler::managed_network_name(&id));
    assert_eq!(network.name(), format!("gascan-network-{id}"));
}

#[test]
fn networked_policy_seals_the_exact_managed_network_name() {
    let (_temp, spec) = spec("version = 1\nnetwork = 'networked'\n");
    let request = PolicyCompiler::compile(spec, &capabilities()).unwrap();
    let expected = PolicyCompiler::managed_network_name(request.id());
    assert_eq!(request.network().managed_name(), Some(expected.as_str()));
}

#[test]
fn offline_policy_has_no_managed_network_name() {
    let (_temp, spec) = spec("version = 1\nnetwork = 'offline'\n");
    let request = PolicyCompiler::compile(spec, &capabilities()).unwrap();
    assert_eq!(request.network().managed_name(), None);
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
fn image_reference_is_the_gate_approved_connected_image() {
    let request = compile_workspace_request();
    let approved = include_str!("../../../images/workspace/approved-image.txt");
    assert_eq!(request.image(), approved);
    assert!(request.image().contains("@sha256:"));
    assert_eq!(request.image().matches('@').count(), 1);
    assert!(!request.image().chars().any(|ch| ch.is_ascii_whitespace()));
}

#[test]
fn explicit_immutable_candidate_can_be_compiled_without_changing_approved_policy() {
    let candidate = "ghcr.io/liquescent-development/gascan/workspace:candidate@sha256:\
                     aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let (_temp, spec) = spec("version = 1\nnetwork = 'networked'\n");
    let request = PolicyCompiler::compile_for_image(spec, &capabilities(), candidate)
        .expect("compile explicit immutable candidate");
    assert_eq!(request.image(), candidate);
    assert_eq!(
        compile_workspace_request().image(),
        include_str!("../../../images/workspace/approved-image.txt")
    );
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
    assert_eq!(defaults.disk_bytes, None);
    assert_eq!(defaults.process_count, None);

    let source = format!(
        "version = 1\nnetwork = 'networked'\n[resources]\ncpus = {MAX_CPUS}\nmemory = '{}GiB'\n",
        MAX_MEMORY_BYTES / 1024_u64.pow(3)
    );
    let (_temp_max, max_spec) = spec(&source);
    let maximum = PolicyCompiler::compile(max_spec, &capabilities())
        .expect("accept documented maxima")
        .resources()
        .to_owned();
    assert_eq!(maximum.cpus, Some(MAX_CPUS));
    assert_eq!(maximum.memory_bytes, Some(MAX_MEMORY_BYTES));
    assert_eq!(maximum.disk_bytes, None);
    assert_eq!(maximum.process_count, None);
}

#[test]
fn explicit_disk_control_is_rejected_as_unsupported() {
    let (_temp, spec) = spec("version = 1\nnetwork = 'networked'\n[resources]\ndisk = '80GiB'\n");
    assert_eq!(
        PolicyCompiler::compile(spec, &capabilities())
            .expect_err("unproven disk controls must fail closed")
            .code(),
        "disk_control_unsupported"
    );
}

#[test]
fn resources_above_any_maximum_are_rejected() {
    for (source, code) in [
        ("[resources]\ncpus = 17\n", "cpus_exceed_maximum"),
        ("[resources]\nmemory = '65GiB'\n", "memory_exceeds_maximum"),
        ("[resources]\ndisk = '513GiB'\n", "disk_control_unsupported"),
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
    assert_eq!(request.environment().len(), 8);
    assert_eq!(
        request
            .environment()
            .get("MISE_DATA_DIR")
            .map(String::as_str),
        Some("/home/workspace/.local/share/mise")
    );
    assert_eq!(
        request
            .environment()
            .get("MISE_SYSTEM_DATA_DIR")
            .map(String::as_str),
        Some("/opt/gascan/mise")
    );
    assert_eq!(
        request
            .environment()
            .get("MISE_STATE_DIR")
            .map(String::as_str),
        Some("/home/workspace/.config/gascan/mise-state")
    );
    assert!(request.environment().get("PATH").is_some_and(|path| {
        path.starts_with("/home/workspace/.local/share/mise/shims:")
            && path.contains(":/opt/gascan/mise/shims:")
    }));
}
