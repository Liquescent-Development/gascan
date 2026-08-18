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
use camino::Utf8Path;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

/// The system OpenSSH client, whose presence is a fixed absolute path's stat.
pub const SYSTEM_SSH_CLIENT: &str = "/usr/bin/ssh";

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

/// Can the caller reach the workspace it named?
///
/// A `canonicalize` and a `metadata` on a path the caller already holds. The
/// daemon computes this from the path the CLI sends it, so the CLI computing it
/// directly is the same measurement with one fewer hop -- and it is the CLI's
/// own working directory, which the CLI knows with more authority than anyone.
#[must_use]
pub fn workspace_fact(path: &Utf8Path) -> DoctorFact {
    let metadata = path
        .canonicalize()
        .map_err(|error| error.to_string())
        .and_then(|path| std::fs::metadata(path).map_err(|error| error.to_string()));
    match metadata {
        Ok(metadata) if metadata.is_dir() => DoctorFact::pass("workspace directory is accessible"),
        Ok(_) => DoctorFact::fail("workspace is not a directory"),
        Err(error) => DoctorFact::fail(format!("workspace is inaccessible: {error}")),
    }
}

/// Is the system OpenSSH client present and executable?
///
/// `ssh.identity` and `ssh.config` are the daemon's -- they read the controller
/// store. `ssh.client` is not: it is a stat of [`SYSTEM_SSH_CLIENT`], and a
/// report that blamed a daemon's startup failure for the state of
/// `/usr/bin/ssh` would be saying something false.
#[must_use]
pub fn ssh_client_fact(client: &Path) -> DoctorFact {
    match std::fs::symlink_metadata(client) {
        Ok(metadata)
            if metadata.file_type().is_file() && metadata.permissions().mode() & 0o111 != 0 =>
        {
            DoctorFact::pass("system OpenSSH client is executable")
        }
        Ok(_) => DoctorFact::fail(format!(
            "system OpenSSH client at {} is not a regular executable",
            client.display()
        )),
        Err(error) => DoctorFact::fail(format!(
            "system OpenSSH client at {} is unavailable: {error}",
            client.display()
        )),
    }
}

/// What this host can say about itself with no daemon running.
///
/// **Split by provenance, not by convenience.** The distinction that matters is
/// whether a fact is the same in any process this account runs, or whether it is
/// scoped to the environment of the process that measured it -- because a report
/// assembled from a live daemon must not have the daemon's answers replaced by a
/// second process's reading of a different variable.
///
/// Which facts exist at all depends on the backend, and that is not a detail
/// either: the Arca backend's `runtime.cli` is an engine executable this account
/// can stat and its `runtime.kernel` is a digest check over files this account
/// owns, while Apple's are answers only the `container` CLI can give. A
/// collector that reported Apple's two from the host would be inventing them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostFacts {
    /// This process's compiled target. Every process of this build agrees.
    pub architecture: DoctorFact,
    /// A plist on this host. Every process on it agrees.
    pub macos: DoctorFact,
    /// The caller's workspace, when the caller named one. The daemon measures
    /// the path the CLI sent it, so the CLI's own reading is the same
    /// measurement one hop earlier. `None` where the caller supplies this fact
    /// from elsewhere, which is what the daemon does from its RPC request.
    pub workspace: Option<DoctorFact>,
    /// A stat of [`SYSTEM_SSH_CLIENT`], a fixed absolute path.
    pub ssh_client: DoctorFact,
    /// The engine's fetched boot artifacts, for backends that have them.
    /// Resolved through the passwd database by euid, so every process of this
    /// account agrees.
    pub engine_artifacts: Option<DoctorFact>,
    /// The engine executable, for backends that have one.
    ///
    /// **The one process-scoped fact here.** It is whatever `GASCAN_ENGINE_BIN`
    /// names in THIS process, and a daemon that is already running was launched
    /// from whatever it named in ITS process. See [`Self::apply_process_scoped`].
    pub engine_binary: Option<DoctorFact>,
}

impl HostFacts {
    /// Measures everything this host can answer for `backend`.
    #[must_use]
    pub fn collect(
        backend: BackendSelection,
        engine_binary: Option<&Path>,
        workspace: Option<&Utf8Path>,
    ) -> Self {
        let engine_backed = matches!(backend, BackendSelection::Arca);
        Self {
            architecture: current_architecture_fact(),
            macos: macos_fact(),
            workspace: workspace.map(workspace_fact),
            ssh_client: ssh_client_fact(Path::new(SYSTEM_SSH_CLIENT)),
            engine_artifacts: engine_backed.then(engine_artifact_fact),
            engine_binary: engine_backed.then(|| engine_binary_fact(engine_binary)),
        }
    }

    /// Writes the facts that are the same in any process of this account.
    ///
    /// Applied whether or not a daemon answered. For these the values really are
    /// equal by construction: none of them takes an input from the process
    /// environment. The architecture is compiled in, the macOS version is a
    /// plist on this host, and the artifact digests resolve through the passwd
    /// database by euid -- and `client.rs` refuses any daemon whose socket peer
    /// uid differs from `geteuid()`, so the account is shared too.
    pub fn apply_account_scoped(&self, facts: &mut super::DoctorFacts) {
        facts.architecture = self.architecture.clone();
        facts.macos = self.macos.clone();
        if let Some(engine_artifacts) = &self.engine_artifacts {
            facts.kernel = engine_artifacts.clone();
        }
    }

    /// Writes the facts measured from whoever is assembling the report.
    ///
    /// The workspace is the caller's own directory and `/usr/bin/ssh` is an
    /// absolute path; neither needs a daemon, and a report that blamed a
    /// daemon's startup failure for the state of either would be saying
    /// something false. The daemon does not use this: it replaces both from its
    /// own request and its own SSH paths after the report is built.
    pub fn apply_caller_scoped(&self, facts: &mut super::DoctorFacts) {
        if let Some(workspace) = &self.workspace {
            facts.workspace = workspace.clone();
        }
        facts.ssh_client = self.ssh_client.clone();
    }

    /// Writes the engine-executable fact. **Only valid when no daemon answered.**
    ///
    /// `GASCAN_ENGINE_BIN` is per-process and mutable, and a running daemon was
    /// launched from whatever it named in the launching shell. Applying this
    /// over a daemon's answer replaces a statement about the engine that is
    /// actually running with a statement about whatever this shell happens to
    /// export -- which fails two ways, both silent:
    ///
    /// - a shell without the variable turns a healthy `runtime.cli` into
    ///   "GASCAN_ENGINE_BIN must name the engine executable" and flips the exit
    ///   code, while the user's sandboxes are running on that very engine;
    /// - a shell with a *newer* path masks the daemon's honest
    ///   "engine executable unavailable at /old/build/arca-engine", which is the
    ///   exact condition [`engine_binary_fact`] exists to catch.
    ///
    /// Where a daemon answered, its `runtime.cli` is the authority. Where none
    /// did, this reading is the only one there is.
    pub fn apply_process_scoped(&self, facts: &mut super::DoctorFacts) {
        if let Some(engine_binary) = &self.engine_binary {
            facts.cli = engine_binary.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HostFacts, architecture_fact, engine_binary_fact, macos_fact_at, ssh_client_fact,
        workspace_fact,
    };
    use crate::backend::BackendSelection;
    use crate::doctor::{DoctorFact, DoctorFacts, DoctorStatus};

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

    /// The threshold, tested in the crate that implements it.
    ///
    /// It used to be asserted only from `gascand`, which meant `cargo test -p
    /// gascan-core` passed with a broken `>= 26` comparison.
    #[test]
    fn the_macos_threshold_is_twenty_six() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        for (version, expected) in [
            ("25.9", DoctorStatus::Fail),
            ("26.0", DoctorStatus::Pass),
            ("26.6.1", DoctorStatus::Pass),
            ("27.0", DoctorStatus::Pass),
        ] {
            let path = temp.path().join("SystemVersion.plist");
            let mut dictionary = plist::Dictionary::new();
            dictionary.insert(
                "ProductVersion".to_owned(),
                plist::Value::String(version.to_owned()),
            );
            plist::Value::Dictionary(dictionary).to_file_xml(&path)?;
            assert_eq!(
                macos_fact_at(&path).status,
                expected,
                "ProductVersion {version}"
            );
        }
        Ok(())
    }

    #[test]
    fn a_missing_workspace_is_inaccessible_and_a_file_is_not_a_directory()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = camino::Utf8PathBuf::from_path_buf(temp.path().to_path_buf())
            .map_err(|_| "non-UTF-8 temp dir")?;
        assert_eq!(workspace_fact(&root).status, DoctorStatus::Pass);
        let file = root.join("a-file");
        std::fs::write(&file, b"")?;
        assert_eq!(workspace_fact(&file).status, DoctorStatus::Fail);
        assert_eq!(
            workspace_fact(&root.join("absent")).status,
            DoctorStatus::Fail
        );
        Ok(())
    }

    #[test]
    fn a_missing_ssh_client_fails_and_names_its_path() {
        let fact = ssh_client_fact(std::path::Path::new("/nonexistent/ssh"));
        assert_eq!(fact.status, DoctorStatus::Fail);
        assert!(fact.detail.contains("/nonexistent/ssh"));
    }

    /// An absent `GASCAN_ENGINE_BIN` is a named failure and not a pass.
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
        let apple = HostFacts::collect(BackendSelection::Apple, None, None);
        assert_eq!(apple.engine_binary, None);
        assert_eq!(apple.engine_artifacts, None);
        let arca = HostFacts::collect(BackendSelection::Arca, None, None);
        assert!(arca.engine_binary.is_some());
        assert!(arca.engine_artifacts.is_some());
    }

    /// **The account-scoped facts never touch `runtime.cli`.**
    ///
    /// This is the regression guard for the defect the split exists to close: a
    /// daemon is running, it answered `runtime.cli` about the engine it was
    /// actually launched from, and this process's `GASCAN_ENGINE_BIN` says
    /// something else -- or nothing. Overwriting the daemon's answer turned a
    /// healthy engine into a missing one, and masked a genuinely missing one.
    #[test]
    fn the_account_scoped_facts_leave_a_daemons_engine_answer_alone() {
        let mut facts = DoctorFacts::unavailable("not collected");
        facts.cli = DoctorFact::pass("engine executable present at /opt/arca/bin/arca-engine");
        facts.service = DoctorFact::pass("the engine answered");
        HostFacts::collect(BackendSelection::Arca, None, None).apply_account_scoped(&mut facts);
        assert_eq!(
            facts.cli.detail, "engine executable present at /opt/arca/bin/arca-engine",
            "the CLI's own GASCAN_ENGINE_BIN overwrote the daemon's answer"
        );
        assert_eq!(facts.service.status, DoctorStatus::Pass);
        assert_eq!(facts.architecture.status, DoctorStatus::Pass);
        assert!(facts.kernel.detail.contains("engine artifacts"));
    }

    /// The process-scoped fact is applied only when it is asked for, and it is
    /// the only thing it writes.
    #[test]
    fn the_process_scoped_fact_writes_only_the_engine_executable() {
        let mut facts = DoctorFacts::unavailable("not collected");
        facts.service = DoctorFact::pass("the engine answered");
        HostFacts::collect(BackendSelection::Arca, None, None).apply_process_scoped(&mut facts);
        assert_eq!(facts.cli.status, DoctorStatus::Fail);
        assert!(facts.cli.detail.contains(crate::backend::ENGINE_BIN_ENV));
        assert_eq!(facts.service.status, DoctorStatus::Pass);
        assert_eq!(facts.architecture.status, DoctorStatus::Unknown);
    }
}
