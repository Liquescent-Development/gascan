use crate::runtime::{NetworkIsolation, RuntimeCapabilities};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoctorStatus {
    Pass,
    Fail,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DoctorCheck {
    pub id: String,
    pub status: DoctorStatus,
    pub detail: String,
    pub remedy: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    pub fn from_runtime(capabilities: &RuntimeCapabilities) -> Self {
        let mut checks = vec![DoctorCheck {
            id: "runtime.version".to_owned(),
            status: DoctorStatus::Pass,
            detail: format!(
                "Apple container {}.{}.{}",
                capabilities.version.major, capabilities.version.minor, capabilities.version.patch
            ),
            remedy: "install a supported Apple container CLI release".to_owned(),
        }];
        for (id, detail, remedy) in [
            (
                "host.architecture",
                "aarch64 host architecture",
                "run gascan on Apple silicon",
            ),
            (
                "host.macos",
                "macOS 26 or newer",
                "upgrade this host to macOS 26 or newer",
            ),
            (
                "runtime.cli",
                "structured Apple container CLI",
                "install the supported Apple container CLI",
            ),
            (
                "runtime.service",
                "Apple container service response",
                "start or repair the Apple container service",
            ),
            (
                "runtime.kernel",
                "Apple container kernel readiness",
                "allow Apple container to install and start its kernel",
            ),
            (
                "runtime.schema",
                "supported structured response schema",
                "install a supported Apple container CLI and service",
            ),
            (
                "storage.state",
                "free state storage",
                "free disk space in the gascan state directory",
            ),
            (
                "storage.images",
                "free image storage",
                "free disk space in Apple container image storage",
            ),
            (
                "workspace.access",
                "workspace accessibility",
                "grant gascan access to the workspace",
            ),
        ] {
            checks.push(capability_check(id, true, detail, remedy));
        }
        for (id, available, detail, remedy) in [
            (
                "runtime.bind_mounts",
                capabilities.bind_mounts,
                "bind mounts",
                "use an Apple container release with bind-mount support",
            ),
            (
                "runtime.named_volumes",
                capabilities.named_volumes,
                "named volumes",
                "use an Apple container release with named-volume support",
            ),
            (
                "runtime.tty",
                capabilities.tty,
                "TTY attachment",
                "use an Apple container release with TTY support",
            ),
            (
                "runtime.signals",
                capabilities.signals,
                "signal forwarding",
                "use an Apple container release with signal support",
            ),
            (
                "runtime.loopback_publish",
                capabilities.loopback_publish,
                "loopback-only port publication",
                "use an Apple container release with loopback publication support",
            ),
            (
                "runtime.resource_limits",
                capabilities.resource_limits,
                "resource limits",
                "use an Apple container release with resource-limit support",
            ),
        ] {
            checks.push(capability_check(id, available, detail, remedy));
        }
        checks.push(capability_check(
            "runtime.offline",
            capabilities.offline == NetworkIsolation::Proven,
            "hard offline networking",
            "install a supported Apple container release with proven offline isolation",
        ));
        Self { checks }
    }

    pub fn check(&self, id: &str) -> Option<&DoctorCheck> {
        self.checks.iter().find(|check| check.id == id)
    }

    pub fn is_ready(&self) -> bool {
        self.checks
            .iter()
            .all(|check| check.status == DoctorStatus::Pass)
    }
}

fn capability_check(id: &str, available: bool, detail: &str, remedy: &str) -> DoctorCheck {
    DoctorCheck {
        id: id.to_owned(),
        status: if available {
            DoctorStatus::Pass
        } else {
            DoctorStatus::Fail
        },
        detail: detail.to_owned(),
        remedy: remedy.to_owned(),
    }
}
