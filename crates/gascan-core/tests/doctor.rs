use gascan_core::doctor::{AppleRemedies, DoctorFact, DoctorFacts, DoctorStatus};

fn ready_facts() -> DoctorFacts {
    DoctorFacts::all_supported_for_tests()
}

#[test]
fn unavailable_evidence_never_becomes_a_pass() {
    let mut facts = ready_facts();
    facts.kernel = DoctorFact::unknown("no stable public kernel readiness evidence");
    let report = facts.into_report(&AppleRemedies);
    let check = report.check("runtime.kernel").unwrap();
    assert_eq!(check.status, DoctorStatus::Unknown);
    assert!(!report.is_ready());
    assert!(check.remedy.contains("container system start"));
}

#[test]
fn doctor_reports_offline_capability_as_release_blocker() {
    let mut facts = ready_facts();
    facts.offline = DoctorFact::fail("hard offline networking is unsupported");
    let report = facts.into_report(&AppleRemedies);
    let check = report.check("runtime.offline").unwrap();
    assert_eq!(check.status, DoctorStatus::Fail);
    assert!(check.remedy.contains("supported Apple container"));
}

#[test]
fn stable_ids_each_have_a_remedy_and_evidence() {
    let report = ready_facts().into_report(&AppleRemedies);
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
        "ssh.client",
        "ssh.identity",
        "ssh.config",
        "ssh.native_publish",
    ] {
        let check = report.check(id).unwrap();
        assert!(!check.detail.is_empty(), "missing evidence for {id}");
        assert!(!check.remedy.is_empty(), "missing remedy for {id}");
    }
}

#[test]
fn doctor_reports_every_native_ssh_prerequisite_with_stable_ids() {
    let report = ready_facts().into_report(&AppleRemedies);
    let ssh = report
        .checks
        .iter()
        .filter(|check| check.id.starts_with("ssh."))
        .map(|check| check.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        ssh,
        [
            "ssh.client",
            "ssh.identity",
            "ssh.config",
            "ssh.native_publish",
        ]
    );
}

#[test]
fn doctor_json_preserves_unknown_status() {
    let mut facts = ready_facts();
    facts.image_storage = DoctorFact::unknown("image path unavailable");
    let value = serde_json::to_value(facts.into_report(&AppleRemedies)).unwrap();
    let checks = value["checks"].as_array().unwrap();
    assert_eq!(
        checks.iter().find(|c| c["id"] == "storage.images").unwrap()["status"],
        "unknown"
    );
}

#[test]
fn warning_status_keeps_runtime_ready_and_is_not_a_readiness_failure() {
    let mut facts = ready_facts();
    facts.version = DoctorFact::warning("untested 1.2.0");
    let report = facts.into_report(&AppleRemedies);

    assert!(report.is_ready());
    assert!(report.runtime_readiness_failure().is_none());
    assert_eq!(
        report.check("runtime.version").unwrap().status,
        DoctorStatus::Warning
    );
}

#[test]
fn request_scoped_workspace_unknown_does_not_block_runtime_readiness() {
    let mut facts = ready_facts();
    facts.workspace = DoctorFact::unknown("workspace access is evaluated for each Doctor request");
    let report = facts.into_report(&AppleRemedies);

    assert!(!report.is_ready());
    assert_eq!(report.runtime_readiness_failure(), None);
}

/// **No remedy under the Arca backend mentions Apple's runtime.**
///
/// This is the defect the remedy interface exists to close. Before it,
/// `into_report` paired every fact with hardcoded Apple prose, so an
/// Arca-backed daemon whose engine socket was dead told the user to "install
/// Apple container 1.1.0 in PATH" -- advice that is worse than silence, because
/// following it changes nothing and the user concludes the runtime is broken.
///
/// The whole report is swept rather than the five runtime checks, because the
/// Apple wording was never confined to them: the storage and capability
/// remedies named Apple's application root and Apple releases too.
#[test]
fn no_arca_remedy_names_apples_runtime() {
    let report = gascan_core::doctor::DoctorFacts::unavailable("evidence withheld")
        .into_report(&gascan_core::doctor::ArcaRemedies);
    for check in &report.checks {
        for forbidden in ["Apple container", "container system", "Apple application"] {
            assert!(
                !check.remedy.contains(forbidden),
                "{} carries Apple's prose under the Arca backend: {}",
                check.id,
                check.remedy
            );
        }
    }
}

/// **Every check has a remedy under every backend, and none is empty.**
///
/// Exhaustiveness itself is the compiler's job -- each implementation matches
/// on `DoctorCheckId`, so a new check fails to build in every backend rather
/// than falling back to someone else's advice. What that cannot catch is a
/// remedy that exists but says nothing, which is what an empty string or a
/// placeholder left behind during a rename would be.
#[test]
fn every_check_has_a_non_empty_remedy_under_every_backend() {
    let sets: [(&str, &dyn gascan_core::doctor::DoctorRemedies); 2] = [
        ("apple", &gascan_core::doctor::AppleRemedies),
        ("arca", &gascan_core::doctor::ArcaRemedies),
    ];
    for (name, remedies) in sets {
        let report = gascan_core::doctor::DoctorFacts::unavailable("evidence withheld")
            .into_report(remedies);
        assert_eq!(
            report.checks.len(),
            21,
            "{name} did not produce a remedy for every check"
        );
        for check in &report.checks {
            assert!(
                check.remedy.trim().len() > 10,
                "{name} gives {} no usable remedy: {:?}",
                check.id,
                check.remedy
            );
        }
    }
}

/// **A fact that carries its own remedy still overrides the backend's.**
///
/// The per-fact override predates this interface and is how a check reports
/// something specific to the observation rather than to the backend. Moving the
/// defaults behind a trait must not have quietly removed it.
#[test]
fn a_fact_specific_remedy_still_wins_over_the_backend_default() {
    let mut facts = gascan_core::doctor::DoctorFacts::unavailable("evidence withheld");
    facts.cli = gascan_core::doctor::DoctorFact::fail("engine executable unavailable")
        .with_remedy("set GASCAN_ENGINE_BIN to the path build-arca-engine.sh printed");
    let report = facts.into_report(&gascan_core::doctor::ArcaRemedies);
    let cli = report
        .check("runtime.cli")
        .expect("runtime.cli is reported");
    assert_eq!(
        cli.remedy,
        "set GASCAN_ENGINE_BIN to the path build-arca-engine.sh printed"
    );
}
