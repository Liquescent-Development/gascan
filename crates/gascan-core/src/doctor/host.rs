//! The doctor facts a process can measure without a daemon.
//!
//! **These lived in `gascand/src/main.rs` only because that is where the report
//! was assembled.** None of them asks the daemon, the runtime or the engine
//! anything: the architecture is this process's target, the macOS version is a
//! plist on disk, the engine's boot artifacts are files under this account's
//! own directory, and the engine executable is a path this account can stat.
//! Keeping them behind the daemon meant `gascan doctor` had nothing to say
//! about a host whose daemon could not start -- which is the state a user runs
//! `gascan doctor` in.
//!
//! They live here rather than being copied into the CLI so that the daemon and
//! the CLI cannot measure the same host and disagree about it. Both crates
//! already depend on `gascan-core`; neither depends on the other.

use super::DoctorFact;
use crate::backend::BackendSelection;
use std::path::Path;

/// Is this process running on the architecture the product supports?
#[must_use]
pub fn architecture_fact(architecture: &str) -> DoctorFact {
    if architecture == "aarch64" {
        DoctorFact::pass("current process target is aarch64")
    } else {
        DoctorFact::fail(format!("current process target is {architecture}"))
    }
}

/// The architecture this process was compiled for.
#[must_use]
pub fn current_architecture_fact() -> DoctorFact {
    architecture_fact(std::env::consts::ARCH)
}

/// Is the host's macOS new enough?
#[must_use]
pub fn macos_fact() -> DoctorFact {
    macos_fact_at(Path::new(
        "/System/Library/CoreServices/SystemVersion.plist",
    ))
}

/// [`macos_fact`] against a named plist, so the parse can be tested.
#[must_use]
pub fn macos_fact_at(path: &Path) -> DoctorFact {
    let result = plist::Value::from_file(path).ok().and_then(|value| {
        value
            .as_dictionary()
            .and_then(|dictionary| dictionary.get("ProductVersion"))
            .and_then(plist::Value::as_string)
            .map(str::to_owned)
    });
    match result {
        Some(version)
            if version
                .split('.')
                .next()
                .and_then(|major| major.parse::<u64>().ok())
                .is_some_and(|major| major >= 26) =>
        {
            DoctorFact::pass(format!("SystemVersion.plist ProductVersion is {version}"))
        }
        Some(version) => DoctorFact::fail(format!(
            "SystemVersion.plist ProductVersion is {version}; macOS 26+ required"
        )),
        None => DoctorFact::fail("could not parse ProductVersion from SystemVersion.plist"),
    }
}

/// Are the engine's boot artifacts installed and still what the pin describes?
///
/// The remedy names `gascan engine fetch` in every failing case, which is the
/// pattern the product already uses for a prerequisite the installer cannot
/// satisfy. It is a per-fact remedy rather than the backend default because it
/// is specific to what was observed: a missing artifact and a moved pin need
/// the same command but the user should be told which happened.
///
/// The digest check and not a presence check. A doctor that only looked for the
/// files would report a moved pin, a truncated download and a corrupted one all
/// as healthy, which is the entire class of failure the digests exist to catch.
/// It runs the SAME verification the fetch runs, so the two cannot disagree
/// about whether what is installed is usable.
#[must_use]
pub fn engine_artifact_fact() -> DoctorFact {
    use crate::engine_artifacts::{ArtifactError, ArtifactPaths, Pin};
    let pin = match Pin::compiled_in() {
        Ok(pin) => pin,
        // Not reachable through a released binary -- a malformed pin fails this
        // crate's own tests -- but reported rather than unwrapped, because this
        // is the command a user reaches for when nothing else works.
        Err(error) => return DoctorFact::fail(error.to_string()),
    };
    let paths = match ArtifactPaths::for_user() {
        Ok(paths) => paths,
        Err(error) => return DoctorFact::fail(error.to_string()),
    };
    match crate::engine_artifacts::verify_installed(&paths, &pin) {
        Ok(()) => DoctorFact::pass(format!(
            "engine artifacts under {} match {}",
            paths.root().display(),
            pin.tag
        )),
        Err(ArtifactError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            DoctorFact::fail(format!(
                "engine artifacts are not installed under {}",
                paths.root().display()
            ))
            .with_remedy("run `gascan engine fetch`")
        }
        Err(error) => DoctorFact::fail(error.to_string())
            .with_remedy("run `gascan engine fetch` to reinstall the artifacts this build expects"),
    }
}

/// Does `GASCAN_ENGINE_BIN` still name an engine executable?
///
/// Checked on disk rather than inferred from the engine answering. A running
/// engine proves an engine ran; it does not prove that the variable still names
/// one, and the next daemon start is what discovers that it does not.
#[must_use]
pub fn engine_binary_fact(engine_binary: Option<&Path>) -> DoctorFact {
    let Some(engine_binary) = engine_binary else {
        return DoctorFact::fail(format!(
            "{} selects the Arca engine backend, so {} must name the engine executable",
            crate::backend::ARCA_BACKEND_ENV,
            crate::backend::ENGINE_BIN_ENV
        ));
    };
    match std::fs::metadata(engine_binary) {
        Ok(metadata) if metadata.is_file() => DoctorFact::pass(format!(
            "engine executable present at {}",
            engine_binary.display()
        )),
        Ok(_) => DoctorFact::fail(format!("{} is not a file", engine_binary.display())),
        Err(error) => DoctorFact::fail(format!(
            "engine executable unavailable at {}: {error}",
            engine_binary.display()
        )),
    }
}

/// What this host can say about itself with no daemon running.
///
/// Which facts those are depends on the backend, and that is not a detail: the
/// Arca backend's `runtime.cli` is an engine executable this account can stat
/// and its `runtime.kernel` is a digest check over files this account owns,
/// while Apple's are answers only the `container` CLI can give. A collector
/// that reported Apple's two from the host would be inventing them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostFacts {
    pub architecture: DoctorFact,
    pub macos: DoctorFact,
    /// The engine executable, for backends that have one.
    pub engine_binary: Option<DoctorFact>,
    /// The engine's fetched boot artifacts, for backends that have them.
    pub engine_artifacts: Option<DoctorFact>,
}

impl HostFacts {
    /// Measures everything this host can answer for `backend`.
    #[must_use]
    pub fn collect(backend: BackendSelection, engine_binary: Option<&Path>) -> Self {
        let engine_backed = matches!(backend, BackendSelection::Arca);
        Self {
            architecture: current_architecture_fact(),
            macos: macos_fact(),
            engine_binary: engine_backed.then(|| engine_binary_fact(engine_binary)),
            engine_artifacts: engine_backed.then(engine_artifact_fact),
        }
    }

    /// Writes these facts into `facts`, leaving every other field alone.
    ///
    /// Applied whether or not a daemon answered. When one did, the values are
    /// equal by construction -- same functions, same host, same account -- and
    /// applying them unconditionally is what keeps that true rather than
    /// assumed: there is no branch in which the host half comes from somewhere
    /// else.
    pub fn apply(self, facts: &mut super::DoctorFacts) {
        facts.architecture = self.architecture;
        facts.macos = self.macos;
        if let Some(engine_binary) = self.engine_binary {
            facts.cli = engine_binary;
        }
        if let Some(engine_artifacts) = self.engine_artifacts {
            facts.kernel = engine_artifacts;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{HostFacts, architecture_fact, engine_binary_fact, macos_fact_at};
    use crate::backend::BackendSelection;
    use crate::doctor::DoctorStatus;

    #[test]
    fn only_aarch64_passes() {
        assert_eq!(architecture_fact("aarch64").status, DoctorStatus::Pass);
        assert_eq!(architecture_fact("x86_64").status, DoctorStatus::Fail);
    }

    #[test]
    fn an_unreadable_plist_fails_rather_than_panicking() {
        let fact = macos_fact_at(std::path::Path::new("/nonexistent/SystemVersion.plist"));
        assert_eq!(fact.status, DoctorStatus::Fail);
    }

    /// An absent `GASCAN_ENGINE_BIN` is a named failure and not a pass.
    ///
    /// The daemon reports this as a startup diagnostic; the doctor has to be
    /// able to say the same thing without one, because the daemon that would
    /// have said it is the daemon that could not start.
    #[test]
    fn an_unset_engine_binary_names_the_variable() {
        let fact = engine_binary_fact(None);
        assert_eq!(fact.status, DoctorStatus::Fail);
        assert!(fact.detail.contains(crate::backend::ENGINE_BIN_ENV));
    }

    /// **Apple's `runtime.cli` and `runtime.kernel` are not host-measurable.**
    /// They are what the `container` CLI reports, so collecting them here would
    /// be inventing an answer the host cannot give.
    #[test]
    fn only_the_engine_backend_contributes_engine_facts() {
        let apple = HostFacts::collect(BackendSelection::Apple, None);
        assert_eq!(apple.engine_binary, None);
        assert_eq!(apple.engine_artifacts, None);
        let arca = HostFacts::collect(BackendSelection::Arca, None);
        assert!(arca.engine_binary.is_some());
        assert!(arca.engine_artifacts.is_some());
    }

    /// `apply` writes the host half and nothing else, so a runtime fact a
    /// daemon supplied cannot be overwritten by a host that never measured it.
    #[test]
    fn apply_leaves_every_runtime_fact_alone() {
        let mut facts = crate::doctor::DoctorFacts::unavailable("not collected");
        facts.service = crate::doctor::DoctorFact::pass("the engine answered");
        HostFacts::collect(BackendSelection::Apple, None).apply(&mut facts);
        assert_eq!(facts.service.status, DoctorStatus::Pass);
        assert_eq!(facts.architecture.status, DoctorStatus::Pass);
        assert_eq!(facts.cli.status, DoctorStatus::Unknown);
        assert_eq!(facts.kernel.status, DoctorStatus::Unknown);
    }
}
