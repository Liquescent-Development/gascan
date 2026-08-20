use gascan_conformance::{CreateRequestFixture, backend_contract, capabilities};
use gascan_core::fake_runtime::FakeRuntime;
use gascan_core::runtime::RuntimeBackend;

#[tokio::test]
async fn fake_runtime_satisfies_the_backend_contract() {
    let backend: Box<dyn RuntimeBackend> = Box::new(FakeRuntime::new(capabilities()));
    let fixture = CreateRequestFixture::pinned("contract", "offline");
    backend_contract(backend.as_ref(), &fixture).await;
}
