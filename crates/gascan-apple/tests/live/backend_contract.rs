use gascan_apple::{AppleBackend, ProcessRunner};
use gascan_conformance::{CreateRequestFixture, backend_contract};
use gascan_core::runtime::RuntimeBackend;
use std::time::{SystemTime, UNIX_EPOCH};

/// The shared backend contract, run against a real `container` CLI.
///
/// **THIS TEST FAILS TODAY AND THE FAILURE IS THE RECORDED RESULT, NOT A
/// REGRESSION.** MEASURED on `newcombe` 2026-08-20 with `container` 1.1.0:
/// `panicked at gascan-conformance/src/lib.rs:104:5 ... left: Running, right:
/// Stopped` -- apple's `create` compiles to `container run`
/// (`gascan-apple/src/translate.rs:100`), so there is no window in which it has
/// produced a `Stopped` container. Everything after that assertion, including
/// the `list_resources` tail below, was NOT REACHED. **Do not weaken the
/// assertion to make this green**; see
/// `docs/evidence/2026-08-20-backend-conformance.md` and
/// `docs/status/START-HERE.md` open item 10.
///
/// The panic quoted above says `104` because that is where the assertion sat
/// when it was measured; the comment now standing over it moved the assertion
/// down. Re-derive the line rather than trusting either number.
///
/// **No CI job runs this tier**, so that measurement is the only evidence that
/// will exist until someone runs it again by hand on a machine with the
/// `container` service.
#[tokio::test]
#[ignore = "requires Apple silicon macOS 26+ with container service and locked workspace image"]
async fn backend_contract_holds_on_apple() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let name = format!("gascan-live-backend-{}-{nonce}", std::process::id());
    let fixture = CreateRequestFixture::pinned(&name, "offline");
    let backend = AppleBackend::new(ProcessRunner);
    backend_contract(&backend, &fixture).await;
    assert!(
        !backend
            .list_resources()
            .await
            .unwrap()
            .iter()
            .any(|resource| resource.name().starts_with(&name))
    );
}
