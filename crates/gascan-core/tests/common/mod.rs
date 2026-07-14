use camino::Utf8Path;
use gascan_core::manifest::Manifest;
use gascan_core::policy::PolicyCompiler;
use gascan_core::runtime::{CreateRequest, NetworkIsolation, RuntimeCapabilities, RuntimeVersion};
use gascan_core::sandbox::SandboxSpec;
use std::ops::Deref;

pub struct CreateRequestFixture {
    _root: tempfile::TempDir,
    request: CreateRequest,
}

impl CreateRequestFixture {
    pub fn request(&self) -> CreateRequest {
        self.request.clone()
    }
}

impl Deref for CreateRequestFixture {
    type Target = CreateRequest;

    fn deref(&self) -> &Self::Target {
        &self.request
    }
}

pub fn capabilities() -> RuntimeCapabilities {
    RuntimeCapabilities {
        version: RuntimeVersion::new(1, 0, 0),
        bind_mounts: true,
        named_volumes: true,
        tty: true,
        signals: true,
        loopback_publish: true,
        resource_limits: true,
        offline: NetworkIsolation::Proven,
    }
}

pub fn create_request(name: &str) -> CreateRequestFixture {
    let temp = tempfile::tempdir().expect("temporary backend-contract root");
    let root = Utf8Path::from_path(temp.path()).expect("UTF-8 temporary path");
    std::fs::write(
        root.join("gascan.toml"),
        "version = 1\nnetwork = 'offline'\n",
    )
    .expect("write backend-contract manifest");
    let manifest = Manifest::load(root).expect("load backend-contract manifest");
    let spec = SandboxSpec::from_root(name, root, manifest).expect("build sealed sandbox spec");
    let request =
        PolicyCompiler::compile(spec, &capabilities()).expect("compile backend-contract policy");
    CreateRequestFixture {
        _root: temp,
        request,
    }
}
