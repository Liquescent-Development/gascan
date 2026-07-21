//! Shared access to the fake attach helper used by the attach test binaries.

use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

use gascan_apple::AppleAttach;

/// Build the fake attach helper once, then hand back its executable path.
///
/// The helper deliberately runs as a real child process, so every attach test
/// spawns one. Reaching it through `cargo run` is not safe under that
/// concurrency: cargo serializes the build behind the target-directory lock but
/// execs the resulting binary *outside* that lock, so one invocation can catch
/// the path while another is relinking it and die with ENOENT before reading a
/// single frame. The bridge then reports `helper closed without a terminal
/// frame`, which reads as a protocol fault rather than the harness defect it
/// is. Building once here and execing the binary directly keeps cargo off the
/// concurrent path entirely.
///
/// `--target-dir` is pinned rather than inferred so an ambient
/// `CARGO_TARGET_DIR` cannot move the binary out from under this path.
fn fake_helper_binary() -> &'static Path {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY.get_or_init(|| {
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-attach-helper");
        let target = fixture.join("target");
        let status = Command::new(env!("CARGO"))
            .arg("build")
            .arg("--quiet")
            .arg("--manifest-path")
            .arg(fixture.join("Cargo.toml"))
            .arg("--target-dir")
            .arg(&target)
            .status()
            .expect("fake attach helper must be buildable");
        assert!(status.success(), "fake attach helper failed to build");
        let binary = target.join("debug/gascan-fake-attach-helper");
        assert!(
            binary.is_file(),
            "fake attach helper is missing after a successful build: {}",
            binary.display()
        );
        binary
    })
}

/// An `AppleAttach` that execs the prebuilt fake helper directly.
pub fn fake_helper() -> AppleAttach {
    AppleAttach::new(fake_helper_binary())
}
