//! Amplifier for the intermittent `ssh-keygen` rejection.
//!
//! `ensure_host_identity` derives the public key by handing `ssh-keygen` a
//! `/dev/fd/<N>` pathname for a descriptor duplicated into the process-global
//! descriptor space. Descriptor numbers are per-process, not per-task, so the
//! suspected failure only appears when several of these run at once alongside
//! other descriptor traffic. A single test binary running one identity at a
//! time cannot exercise that; this does.
//!
//! **A failure here is the known open `ssh-keygen` rejection, not a new flake.**
//! It is `SshError::KeygenRejected` carrying `Bad file descriptor`, and it is
//! recorded in `docs/status/arca-integration-handoff.md`. This test exists to
//! reproduce that defect on demand -- it is what made the descriptor-numbering
//! comparison in `identity.rs` possible -- so it is expected to fail
//! occasionally until the defect is understood.
//!
//! The committed defaults keep the test cheap enough for the normal suite.
//! `GASCAN_IDENTITY_STRESS_ROUNDS` and `GASCAN_IDENTITY_STRESS_WIDTH` raise them
//! for a hunt.

use gascand::{SshPaths, ensure_host_identity, prepare_openssh_files};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn tuning(variable: &str, default: usize) -> Result<usize, Box<dyn std::error::Error>> {
    match std::env::var(variable) {
        Ok(value) => Ok(value.parse()?),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(error.into()),
    }
}

/// Keeps the process-global descriptor table churning so that the number a
/// duplicated private key lands on varies between spawns, as it does when the
/// real suite runs many tests in one binary.
fn spawn_descriptor_churn(stop: &Arc<AtomicBool>) -> Vec<std::thread::JoinHandle<()>> {
    (0..4)
        .map(|_| {
            let stop = Arc::clone(stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let mut held = Vec::new();
                    for _ in 0..16 {
                        if let Ok(file) = std::fs::File::open("/dev/null") {
                            held.push(file);
                        }
                    }
                    drop(held);
                }
            })
        })
        .collect()
}

/// Keeps unrelated `fork`/`exec` traffic going from other threads. The binary
/// that reproduced the rejection spawns blocking `std::process::Command`
/// children (`mkfifo`, readiness probes) from tests running beside the one that
/// failed; a fork taken while another thread holds a descriptor is the classic
/// shape of a descriptor that does not survive into the child.
fn spawn_process_churn(stop: &Arc<AtomicBool>) -> Vec<std::thread::JoinHandle<()>> {
    (0..4)
        .map(|_| {
            let stop = Arc::clone(stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let _ = std::process::Command::new("/usr/bin/true")
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status();
                }
            })
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_identity_derivation_never_loses_the_private_descriptor() -> TestResult {
    let rounds = tuning("GASCAN_IDENTITY_STRESS_ROUNDS", 4)?;
    let width = tuning("GASCAN_IDENTITY_STRESS_WIDTH", 16)?;
    let stop = Arc::new(AtomicBool::new(false));
    let mut churn = spawn_descriptor_churn(&stop);
    churn.extend(spawn_process_churn(&stop));

    let mut failure = None;
    'rounds: for round in 0..rounds {
        let mut tasks = Vec::with_capacity(width);
        for index in 0..width {
            tasks.push(tokio::spawn(async move {
                let temp = tempfile::tempdir()?;
                let home = temp.path().canonicalize()?;
                let paths = SshPaths::for_environment(None, Some(home.as_os_str()))?;
                let identity = ensure_host_identity(&paths).await.map_err(
                    |error| -> Box<dyn std::error::Error + Send + Sync> {
                        format!("round {round} identity {index} ensure: {error}").into()
                    },
                )?;
                // Re-derives through `open_revalidated_identity`, which runs the
                // spawn on a scoped thread inside a freshly built runtime rather
                // than on the caller's. That is the route the failing test took,
                // and `ensure_host_identity` alone never exercises it.
                prepare_openssh_files(&paths, &identity, &[]).map_err(
                    |error| -> Box<dyn std::error::Error + Send + Sync> {
                        format!("round {round} identity {index} prepare: {error}").into()
                    },
                )?;
                Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
            }));
        }
        for task in tasks {
            match task.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    failure = Some(error.to_string());
                    break 'rounds;
                }
                Err(error) => {
                    failure = Some(error.to_string());
                    break 'rounds;
                }
            }
        }
    }

    stop.store(true, Ordering::Relaxed);
    for handle in churn {
        handle
            .join()
            .map_err(|_| "descriptor churn thread panicked")?;
    }
    match failure {
        Some(message) => Err(message.into()),
        None => Ok(()),
    }
}
