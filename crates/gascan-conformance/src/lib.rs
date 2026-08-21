//! Backend conformance: one contract, run against every `RuntimeBackend`.
//!
//! This crate exists because `gascan-core/src/lib.rs:2` denies
//! `clippy::unwrap_used`, and a conformance suite is built from unwrapping
//! assertions. It is a dev-dependency of its consumers and ships nowhere.

use camino::Utf8Path;
use gascan_core::manifest::Manifest;
use gascan_core::policy::PolicyCompiler;
use gascan_core::runtime::{
    ContainerState, CreateRequest, ExecInput, ExecOutput, ExecRequest, NetworkIsolation,
    RemoveRequest, ResourceKind, RuntimeBackend, RuntimeCapabilities, RuntimeVersion,
};
use gascan_core::sandbox::SandboxSpec;
use std::ops::Deref;

pub struct CreateRequestFixture {
    _root: tempfile::TempDir,
    request: CreateRequest,
}

impl CreateRequestFixture {
    /// A request against the approved workspace image.
    ///
    /// Correct for the fake and for apple. **Wrong for a live engine**, whose
    /// store holds only what the tier seeded -- use [`Self::for_image`] there.
    pub fn pinned(name: &str, network: &str) -> Self {
        assert!(matches!(network, "offline" | "networked"));
        Self::build(name, &format!("version = 1\nnetwork = '{network}'\n"), None)
    }

    /// A request against `image`, for a backend whose store was seeded with it.
    ///
    /// The manifest is the only knob, matching `policy_request_from_manifest`
    /// in arca's live tier: the guest user and any ports are manifest facts,
    /// and a caller reaching around them would build a request gascan itself
    /// cannot produce.
    pub fn for_image(name: &str, image: &str, manifest: &str) -> Self {
        Self::build(name, manifest, Some(image))
    }

    pub fn request(&self) -> CreateRequest {
        self.request.clone()
    }

    fn build(name: &str, manifest_text: &str, image: Option<&str>) -> Self {
        let temp = tempfile::tempdir().expect("temporary backend-contract root");
        let root = Utf8Path::from_path(temp.path()).expect("UTF-8 temporary path");
        std::fs::write(root.join("gascan.toml"), manifest_text)
            .expect("write backend-contract manifest");
        let manifest = Manifest::load(root).expect("load backend-contract manifest");
        let spec = SandboxSpec::from_root(name, root, manifest).expect("build sealed sandbox spec");
        let request = match image {
            None => PolicyCompiler::compile(spec, &capabilities()),
            Some(image) => PolicyCompiler::compile_for_image(spec, &capabilities(), image),
        }
        .expect("compile backend-contract policy");
        Self {
            _root: temp,
            request,
        }
    }
}

impl Deref for CreateRequestFixture {
    type Target = CreateRequest;

    fn deref(&self) -> &Self::Target {
        &self.request
    }
}

/// Every flag true. The compiler gates on what a runtime CLAIMS, and the
/// contract only needs a well-formed request; what is under test is the
/// backend's behaviour, not the compiler's gating.
pub fn capabilities() -> RuntimeCapabilities {
    RuntimeCapabilities {
        version: RuntimeVersion::new(1, 1, 0),
        bind_mounts: true,
        named_volumes: true,
        tty: true,
        signals: true,
        loopback_publish: true,
        resource_limits: true,
        offline: NetworkIsolation::Proven,
    }
}

/// The contract every `RuntimeBackend` owes, whatever it is implemented over.
///
/// `fixture` is a parameter and not built here because `PolicyCompiler::compile`
/// pins the approved workspace image, which a live engine's seeded store does
/// not hold -- see `CreateRequestFixture::for_image`.
pub async fn backend_contract(backend: &dyn RuntimeBackend, fixture: &CreateRequestFixture) {
    let id = fixture.id().clone();
    assert_eq!(backend.inspect(&id).await.unwrap(), None);
    let created = backend.create(fixture.request()).await.unwrap();
    assert!(
        created
            .created()
            .iter()
            .any(|resource| resource.kind() == ResourceKind::Container)
    );
    assert_eq!(
        backend.inspect(&id).await.unwrap().unwrap().state,
        ContainerState::Stopped
    );
    // A second `create` of an id the backend already holds is a conflict, not a
    // silent re-create. Each backend detects this its own way -- the fake by
    // map lookup, apple by pre-flight inventory scan, arca engine-side -- so the
    // stable code is the only portable thing to assert.
    let conflict = backend.create(fixture.request()).await.unwrap_err();
    assert_eq!(conflict.code(), "resource_conflict");
    // `conflict.created()` is deliberately neither removed nor asserted about.
    // A rejected `create` may report resources it built before the collision,
    // but both creates here use the same request, so those names are the live
    // sandbox's own -- removing them would tear down the sandbox this walk still
    // has to start, exec, stop and remove. The one live measurement of a
    // conflicting create reporting what it made
    // (`gascan-arca/tests/live/lifecycle.rs:259-278`) does not settle this case:
    // there the container and volumes had been removed first and only the
    // network name was still held, so its three volumes were genuinely
    // orphaned. What a same-request collision reports is unmeasured everywhere.

    // Doubled deliberately: `start` and `stop` are idempotent, so the second
    // call of each must succeed and not report the sandbox's current state as
    // an error. Collapsing either pair deletes the assertion.
    backend.start(&id).await.unwrap();
    backend.start(&id).await.unwrap();
    let mut session = backend
        .exec(ExecRequest::fixture(id.clone(), ["true"]))
        .await
        .unwrap();
    session.send(ExecInput::Close).await.unwrap();
    assert_eq!(
        session.next().await.unwrap().unwrap(),
        ExecOutput::Exit { code: 0, signal: 0 }
    );
    // `Exit` is terminal: the stream ends there, and every consumer that drains
    // to completion depends on it -- `gascand/src/service.rs:2336` and
    // `gascand/src/ssh/manager.rs:700` both loop `while let Some(..) =
    // session.next().await` and would hang forever against a backend that kept
    // the stream open. Each backend closes it in its own code: the fake by its
    // spawned task returning after the `Exit` send
    // (`gascan-core/src/fake_runtime.rs:1123`), apple by breaking on `terminal`
    // so the sender drops (`gascan-apple/src/backend.rs:614`), arca by the same
    // break over engine frames (`gascan-arca/src/backend.rs:387`).
    assert!(session.next().await.is_none());
    backend.stop(&id).await.unwrap();
    backend.stop(&id).await.unwrap();
    backend
        .remove(RemoveRequest::from_resources(created.created().to_vec()).unwrap())
        .await
        .unwrap();
    assert_eq!(backend.inspect(&id).await.unwrap(), None);
}
