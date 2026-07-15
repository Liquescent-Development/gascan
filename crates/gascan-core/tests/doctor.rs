use gascan_core::doctor::{DoctorReport, DoctorStatus};
use gascan_core::runtime::{NetworkIsolation, RuntimeCapabilities, RuntimeVersion};

fn capabilities(offline: NetworkIsolation) -> RuntimeCapabilities {
    RuntimeCapabilities {
        version: RuntimeVersion::new(1, 1, 0),
        bind_mounts: true,
        named_volumes: true,
        tty: true,
        signals: true,
        loopback_publish: true,
        resource_limits: true,
        offline,
    }
}

#[test]
fn doctor_reports_offline_capability_as_release_blocker() {
    let report = DoctorReport::from_runtime(&capabilities(NetworkIsolation::Unsupported));
    let check = report.check("runtime.offline").unwrap();
    assert_eq!(check.status, DoctorStatus::Fail);
    assert!(check.remedy.contains("supported Apple container"));
}

#[test]
fn doctor_has_stable_ids_and_remedies_for_every_mandatory_capability() {
    let mut unsupported = capabilities(NetworkIsolation::Proven);
    unsupported.bind_mounts = false;
    unsupported.named_volumes = false;
    unsupported.tty = false;
    unsupported.signals = false;
    unsupported.loopback_publish = false;
    unsupported.resource_limits = false;
    let report = DoctorReport::from_runtime(&unsupported);

    for id in [
        "host.architecture",
        "host.macos",
        "runtime.cli",
        "runtime.service",
        "runtime.kernel",
        "runtime.schema",
        "storage.state",
        "storage.images",
        "workspace.access",
        "runtime.bind_mounts",
        "runtime.named_volumes",
        "runtime.tty",
        "runtime.signals",
        "runtime.loopback_publish",
        "runtime.resource_limits",
        "runtime.offline",
    ] {
        let check = report.check(id).unwrap();
        assert!(!check.remedy.is_empty(), "missing remedy for {id}");
    }
}

#[test]
fn doctor_report_json_uses_stable_machine_readable_shape() {
    let value = serde_json::to_value(DoctorReport::from_runtime(&capabilities(
        NetworkIsolation::Proven,
    )))
    .unwrap();
    assert_eq!(value["checks"][0]["id"], "runtime.version");
    assert_eq!(value["checks"][0]["status"], "pass");
    assert!(value["checks"][0]["remedy"].is_string());
}
