use gascan_core::doctor::{DoctorFact, DoctorFacts, DoctorStatus};

fn ready_facts() -> DoctorFacts {
    DoctorFacts::all_supported_for_tests()
}

#[test]
fn unavailable_evidence_never_becomes_a_pass() {
    let mut facts = ready_facts();
    facts.kernel = DoctorFact::unknown("no stable public kernel readiness evidence");
    let report = facts.into_report();
    let check = report.check("runtime.kernel").unwrap();
    assert_eq!(check.status, DoctorStatus::Unknown);
    assert!(!report.is_ready());
    assert!(check.remedy.contains("container system start"));
}

#[test]
fn doctor_reports_offline_capability_as_release_blocker() {
    let mut facts = ready_facts();
    facts.offline = DoctorFact::fail("hard offline networking is unsupported");
    let report = facts.into_report();
    let check = report.check("runtime.offline").unwrap();
    assert_eq!(check.status, DoctorStatus::Fail);
    assert!(check.remedy.contains("supported Apple container"));
}

#[test]
fn stable_ids_each_have_a_remedy_and_evidence() {
    let report = ready_facts().into_report();
    for id in [
        "host.architecture",
        "host.macos",
        "runtime.cli",
        "runtime.version",
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
        assert!(!check.detail.is_empty(), "missing evidence for {id}");
        assert!(!check.remedy.is_empty(), "missing remedy for {id}");
    }
}

#[test]
fn doctor_json_preserves_unknown_status() {
    let mut facts = ready_facts();
    facts.image_storage = DoctorFact::unknown("image path unavailable");
    let value = serde_json::to_value(facts.into_report()).unwrap();
    let checks = value["checks"].as_array().unwrap();
    assert_eq!(
        checks.iter().find(|c| c["id"] == "storage.images").unwrap()["status"],
        "unknown"
    );
}
