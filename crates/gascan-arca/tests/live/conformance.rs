use crate::common::{LiveEngine, base_oci_layout, layout_running};
use camino::Utf8Path;
use gascan_arca::ArcaBackend;
use gascan_conformance::{CreateRequestFixture, backend_contract};

/// The tag the derived layout is loaded under.
const TAG: &str = "gascan-conformance:latest";

/// `user = 'root'` because the base layout is a stock alpine with no
/// `workspace` account -- see `lifecycle.rs`'s note on the same constant.
///
/// `network = 'networked'` and not `'offline'`: offline is the one capability
/// this engine is known NOT to honour
/// (`docs/evidence/2026-08-18-arca-engine-offline.md`), so an offline request
/// would test the refuted property by accident.
const MANIFEST: &str = "version = 1\nnetwork = 'networked'\nuser = 'root'\n";

/// The shared backend contract, run against a real `arca-engine`.
///
/// **The image is `engine.image(TAG)` and not `TAG`**, which is why the engine
/// is started before the fixture is built. `PolicyCompiler::compile_for_image`
/// refuses a mutable reference outright (`gascan-core/src/policy.rs:179`), and
/// `LiveEngine::image` is what turns the seeded tag into the store's own
/// `repository@sha256:...`. MEASURED, on `newcombe` 2026-08-20 with the bare
/// tag: the test panicked at `gascan-conformance/src/lib.rs:57` with
/// `compile backend-contract policy: InvalidWorkspaceImage`, in 0.66s -- before
/// a single call reached the backend. A fixture that cannot be built measures
/// nothing about arca.
#[tokio::test]
#[ignore = "requires a built arca-engine, a kernel, a vminit layout and a base OCI layout"]
async fn backend_contract_holds_on_arca() {
    let temp = tempfile::tempdir().expect("a temporary layout root");
    let destination = Utf8Path::from_path(temp.path()).expect("a utf-8 temporary path");
    let layout = layout_running(
        &base_oci_layout(),
        destination,
        TAG,
        &["sh", "-c", "while :; do sleep 1; done"],
    );
    let engine = LiveEngine::start_with_images(&[layout.as_path()]).await;
    let backend = ArcaBackend::new(engine.transport().await);
    let fixture = CreateRequestFixture::for_image("conformance", &engine.image(TAG), MANIFEST);
    backend_contract(&backend, &fixture).await;
}
