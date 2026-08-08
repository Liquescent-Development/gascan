mod fake_transport;

use fake_transport::{Call, FakeEngine};
use gascan_arca::ArcaBackend;
use gascan_core::runtime::{ContainerState, ResourceOwnership, RuntimeBackend};
use gascan_core::sandbox::SandboxId;
use gascan_engine_proto::v1;

const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn digest() -> v1::ImageDigest {
    v1::ImageDigest {
        repository: "registry.example/workspace".to_owned(),
        sha256_hex: DIGEST.to_owned(),
    }
}

fn owner(id: &SandboxId) -> v1::OwnerLabels {
    v1::OwnerLabels {
        managed_by: "gascan".to_owned(),
        sandbox_id: id.as_str().to_owned(),
    }
}

/// The one `Ack` shape the engine returns for a success with nothing to say.
fn ok_ack() -> v1::AckResponse {
    v1::AckResponse {
        outcome: Some(v1::ack_response::Outcome::Ok(v1::Ack {})),
    }
}

/// A policy-validated `CreateRequest`, which is the only kind that exists.
///
/// `CreateRequest`'s fields are `pub(crate)` to `gascan-core` and it derives no
/// `Deserialize`, so `PolicyCompiler` is the only construction path — there is
/// deliberately no fixture constructor. This mirrors `request_with_manifest` in
/// `gascan-apple/tests/backend_fake_runner.rs`, which solves the same problem the
/// same way. The `TempDir` must outlive the request: the compiled request names
/// its canonical root.
fn policy_request(name: &str) -> (tempfile::TempDir, gascan_core::runtime::CreateRequest) {
    use camino::Utf8Path;
    use gascan_core::manifest::Manifest;
    use gascan_core::policy::PolicyCompiler;
    use gascan_core::runtime::{NetworkIsolation, RuntimeCapabilities, RuntimeVersion};
    use gascan_core::sandbox::SandboxSpec;

    let root = tempfile::tempdir().expect("a temporary project root");
    let path = Utf8Path::from_path(root.path()).expect("a utf-8 temporary path");
    std::fs::write(
        path.join("gascan.toml"),
        "version = 1\nnetwork = 'networked'\n",
    )
    .expect("a manifest");
    let spec = SandboxSpec::from_root(name, path, Manifest::load(path).expect("a manifest"))
        .expect("a spec");
    let capabilities = RuntimeCapabilities {
        version: RuntimeVersion::new(1, 1, 0),
        bind_mounts: true,
        named_volumes: true,
        tty: true,
        signals: true,
        loopback_publish: true,
        resource_limits: true,
        offline: NetworkIsolation::Proven,
    };
    let request = PolicyCompiler::compile(spec, &capabilities).expect("a validated request");
    (root, request)
}

#[tokio::test]
async fn capabilities_reads_the_engine_and_renames_project_mount() {
    let engine = FakeEngine::default();
    // ONE representative case, not an exhaustive check of the mapping. This test's
    // job is that the Capabilities arm is read at all and the mapping is reached;
    // the field-by-field pin -- every flag raised alone, all six fields read each
    // time -- is `translate::tests::each_capability_flag_maps_to_exactly_one_field`,
    // which is where a transposition or a hardcoded `true` is caught. The fixture
    // raises `project_mount` alone so the assertions below are not vacuous.
    *engine.capabilities.lock().expect("test lock") = Some(v1::CapabilitiesResponse {
        outcome: Some(v1::capabilities_response::Outcome::Capabilities(
            v1::Capabilities {
                engine_version: Some(v1::Version {
                    major: 1,
                    minor: 0,
                    patch: 0,
                }),
                contract_minor: 0,
                project_mount: true,
                named_volumes: false,
                tty: false,
                signals: false,
                loopback_publish: false,
                resource_limits: false,
                offline: v1::Isolation::Proven as i32,
            },
        )),
    });

    let capabilities = ArcaBackend::new(engine)
        .capabilities()
        .await
        .expect("a capability set maps");
    assert!(
        capabilities.bind_mounts,
        "project_mount is Gas Can's bind_mounts"
    );
    assert!(!capabilities.named_volumes);
    assert!(!capabilities.tty);
    assert!(!capabilities.signals);
    assert!(!capabilities.loopback_publish);
    assert!(!capabilities.resource_limits);
}

#[tokio::test]
async fn an_engine_error_arrives_as_its_own_code() {
    let engine = FakeEngine::default();
    *engine.capabilities.lock().expect("test lock") = Some(v1::CapabilitiesResponse {
        outcome: Some(v1::capabilities_response::Outcome::Error(
            FakeEngine::engine_error("not_found"),
        )),
    });

    let error = ArcaBackend::new(engine)
        .capabilities()
        .await
        .expect_err("the engine refused");
    assert_eq!(error.code(), "not_found");
}

#[tokio::test]
async fn inspect_distinguishes_absent_from_a_failure_to_tell() {
    let id = SandboxId::test("observed");

    let present = FakeEngine::default();
    *present.inspect.lock().expect("test lock") = Some(v1::InspectResponse {
        outcome: Some(v1::inspect_response::Outcome::Sandbox(v1::Sandbox {
            sandbox_id: id.as_str().to_owned(),
            image: Some(digest()),
            state: v1::SandboxState::Running as i32,
            owner: Some(owner(&id)),
            ports: vec![v1::PortMapping {
                host_port: 22222,
                guest_port: 22,
            }],
        })),
    });
    let backend = ArcaBackend::new(present);
    let observed = backend.inspect(&id).await.expect("present").expect("some");
    assert_eq!(observed.state, ContainerState::Running);
    assert_eq!(observed.ports().len(), 1);

    let absent = FakeEngine::default();
    *absent.inspect.lock().expect("test lock") = Some(v1::InspectResponse {
        outcome: Some(v1::inspect_response::Outcome::Absent(v1::Absent {})),
    });
    assert!(
        ArcaBackend::new(absent)
            .inspect(&id)
            .await
            .expect("absent is an answer, not a failure")
            .is_none(),
    );

    let unset = FakeEngine::default();
    *unset.inspect.lock().expect("test lock") = Some(v1::InspectResponse { outcome: None });
    assert_eq!(
        ArcaBackend::new(unset)
            .inspect(&id)
            .await
            .expect_err("an unset oneof is not an answer")
            .code(),
        "invalid_output",
    );
}

#[tokio::test]
async fn start_stop_and_prepare_image_report_an_ack() {
    let id = SandboxId::test("lifecycle");

    let starting = FakeEngine::default();
    *starting.ack.lock().expect("test lock") = Some(ok_ack());
    let backend = ArcaBackend::new(starting);
    backend.start(&id).await.expect("an ack is success");
    assert_eq!(
        backend.into_transport().calls(),
        [Call::Start(v1::StartRequest {
            sandbox_id: id.as_str().to_owned()
        })],
    );

    // stop, which this test's name promises and an earlier draft did not deliver.
    let stopping = FakeEngine::default();
    *stopping.ack.lock().expect("test lock") = Some(ok_ack());
    let backend = ArcaBackend::new(stopping);
    backend.stop(&id).await.expect("an ack is success");
    assert_eq!(
        backend.into_transport().calls(),
        [Call::Stop(v1::StopRequest {
            sandbox_id: id.as_str().to_owned()
        })],
    );

    let preparing = FakeEngine::default();
    *preparing.prepare_image.lock().expect("test lock") = Some(v1::PrepareImageResponse {
        outcome: Some(v1::prepare_image_response::Outcome::Ok(v1::Ack {})),
    });
    let backend = ArcaBackend::new(preparing);
    backend
        .prepare_image(&format!("registry.example/workspace@sha256:{DIGEST}"))
        .await
        .expect("a digest the engine holds");
    assert_eq!(
        backend.into_transport().calls(),
        [Call::PrepareImage(v1::PrepareImageRequest {
            image: Some(digest())
        })],
    );
}

#[tokio::test]
async fn prepare_image_refuses_a_reference_without_a_digest_before_calling() {
    let backend = ArcaBackend::new(FakeEngine::default());
    let error = backend
        .prepare_image("registry.example/workspace:latest")
        .await
        .expect_err("a tag-only reference is not expressible");
    assert_eq!(error.code(), "invalid_state");
    assert!(
        backend.into_transport().calls().is_empty(),
        "a request that cannot be expressed must not reach the engine",
    );
}

#[tokio::test]
async fn list_resources_classifies_what_the_engine_returned() {
    let id = SandboxId::test("owned");
    let engine = FakeEngine::default();
    *engine.list_resources.lock().expect("test lock") = Some(v1::ListResourcesResponse {
        outcome: Some(v1::list_resources_response::Outcome::Resources(
            v1::ResourceList {
                resources: vec![
                    v1::Resource {
                        identity: Some(v1::ResourceIdentity {
                            kind: v1::ResourceKind::Container as i32,
                            name: id.as_str().to_owned(),
                        }),
                        owner: Some(owner(&id)),
                    },
                    v1::Resource {
                        identity: Some(v1::ResourceIdentity {
                            kind: v1::ResourceKind::Volume as i32,
                            name: "someone-elses-volume".to_owned(),
                        }),
                        owner: None,
                    },
                ],
            },
        )),
    });

    let resources = ArcaBackend::new(engine)
        .list_resources()
        .await
        .expect("a mixed inventory maps");
    assert_eq!(
        resources
            .iter()
            .map(|resource| (resource.name(), resource.ownership()))
            .collect::<Vec<_>>(),
        [
            (id.as_str(), ResourceOwnership::GasCanOwned),
            ("someone-elses-volume", ResourceOwnership::Foreign),
        ],
    );
}

/// Builds the `Created` a well-behaved engine would answer a compiled request
/// with: the container, every requested volume, and the managed network.
fn created_for(request: &gascan_core::runtime::CreateRequest) -> v1::Created {
    let id = request.id();
    let mut created = vec![v1::Resource {
        identity: Some(v1::ResourceIdentity {
            kind: v1::ResourceKind::Container as i32,
            name: id.as_str().to_owned(),
        }),
        owner: Some(owner(id)),
    }];
    for volume in request.volumes() {
        created.push(v1::Resource {
            identity: Some(v1::ResourceIdentity {
                kind: v1::ResourceKind::Volume as i32,
                name: volume.name.clone(),
            }),
            owner: Some(owner(id)),
        });
    }
    if let Some(name) = request.network().managed_name() {
        created.push(v1::Resource {
            identity: Some(v1::ResourceIdentity {
                kind: v1::ResourceKind::Network as i32,
                name: name.to_owned(),
            }),
            owner: Some(owner(id)),
        });
    }
    v1::Created { created }
}

#[tokio::test]
async fn create_sends_the_compiled_request_and_reports_what_was_made() {
    let (_root, request) = policy_request("creating");
    let engine = FakeEngine::default();
    *engine.create.lock().expect("test lock") = Some(v1::CreateResponse {
        outcome: Some(v1::create_response::Outcome::Created(created_for(&request))),
    });

    let expected_resources = created_for(&request).created.len();
    let backend = ArcaBackend::new(engine);
    let outcome = backend
        .create(request.clone())
        .await
        .expect("a well-formed Created maps");
    assert_eq!(outcome.created().len(), expected_resources);

    let calls = backend.into_transport().calls();
    let [Call::Create(sent)] = calls.as_slice() else {
        panic!("create must reach the engine exactly once: {calls:?}");
    };
    assert_eq!(sent.sandbox_id, request.id().as_str());
    assert!(
        sent.project.is_some(),
        "the one project mount is always sent"
    );
    assert!(
        sent.owner.is_some(),
        "labels are how the engine recognises us later"
    );
}

#[tokio::test]
async fn a_created_naming_a_resource_outside_the_request_is_refused() {
    let (_root, request) = policy_request("creating");
    let mut created = created_for(&request);
    created.created.push(v1::Resource {
        identity: Some(v1::ResourceIdentity {
            kind: v1::ResourceKind::Volume as i32,
            name: "a-volume-nobody-asked-for".to_owned(),
        }),
        owner: Some(owner(request.id())),
    });

    let engine = FakeEngine::default();
    *engine.create.lock().expect("test lock") = Some(v1::CreateResponse {
        outcome: Some(v1::create_response::Outcome::Created(created)),
    });

    let failure = ArcaBackend::new(engine)
        .create(request)
        .await
        .expect_err("a resource outside the requested topology is not ours to accept");
    assert_eq!(
        failure.code(),
        "ownership_mismatch",
        "gascan-core's own constructor is the boundary check",
    );
}

#[tokio::test]
async fn a_partial_create_keeps_the_evidence_and_the_engines_reason() {
    let (_root, request) = policy_request("creating");
    let engine = FakeEngine::default();
    *engine.create.lock().expect("test lock") = Some(v1::CreateResponse {
        outcome: Some(v1::create_response::Outcome::Failed(v1::CreateFailed {
            created: vec![v1::Resource {
                identity: Some(v1::ResourceIdentity {
                    kind: v1::ResourceKind::Container as i32,
                    name: request.id().as_str().to_owned(),
                }),
                owner: Some(owner(request.id())),
            }],
            error: Some(FakeEngine::engine_error("resource_conflict")),
        })),
    });

    let failure = ArcaBackend::new(engine)
        .create(request)
        .await
        .expect_err("a partial create is a failure");
    assert_eq!(failure.code(), "resource_conflict");
    assert_eq!(
        failure.created().len(),
        1,
        "losing partial-create evidence leaks resources nothing later knows to look for",
    );
}

#[tokio::test]
async fn a_malformed_resource_blames_the_rpc_that_actually_carried_it() {
    // `Resource` is returned by ListResources, Create, and CreateContainer alike,
    // so the operation name in the diagnostic has to be threaded from the call
    // site. When it was hardcoded, a malformed resource in a create response read
    // "invalid output from list_resources" and pointed an operator at an RPC that
    // was never made.
    let malformed = || v1::Resource {
        identity: None,
        owner: None,
    };

    let creating = FakeEngine::default();
    *creating.create.lock().expect("test lock") = Some(v1::CreateResponse {
        outcome: Some(v1::create_response::Outcome::Created(v1::Created {
            created: vec![malformed()],
        })),
    });
    let (_root, request) = policy_request("creating");
    let failure = ArcaBackend::new(creating)
        .create(request)
        .await
        .expect_err("a resource with no identity is not addressable");
    assert_eq!(failure.code(), "invalid_output");
    let rendered = failure.to_string();
    assert!(
        rendered.contains("from create:"),
        "a create must blame create: {rendered}",
    );
    assert!(
        !rendered.contains("list_resources"),
        "naming an RPC that was never called sends an operator to the wrong place: {rendered}",
    );

    // The same resource on the same response type, reached through the other
    // create path, must blame that path instead.
    let recreating = FakeEngine::default();
    *recreating.create.lock().expect("test lock") = Some(v1::CreateResponse {
        outcome: Some(v1::create_response::Outcome::Created(v1::Created {
            created: vec![malformed()],
        })),
    });
    let (_root, request) = policy_request("recreating");
    let retained = gascan_core::runtime::RetainedResources::new(&request, retained_for(&request))
        .expect("the retained set matches the requested topology exactly");
    let recreate =
        gascan_core::runtime::RecreateRequest::new(request, retained).expect("a recreate request");
    let rendered = ArcaBackend::new(recreating)
        .create_container(recreate)
        .await
        .expect_err("a resource with no identity is not addressable")
        .to_string();
    assert!(
        rendered.contains("from create_container:"),
        "a recreate must blame create_container: {rendered}",
    );

    // And list_resources, which is where the hardcoded name came from, still
    // blames itself.
    let listing = FakeEngine::default();
    *listing.list_resources.lock().expect("test lock") = Some(v1::ListResourcesResponse {
        outcome: Some(v1::list_resources_response::Outcome::Resources(
            v1::ResourceList {
                resources: vec![malformed()],
            },
        )),
    });
    let rendered = ArcaBackend::new(listing)
        .list_resources()
        .await
        .expect_err("a resource with no identity is not addressable")
        .to_string();
    assert!(
        rendered.contains("from list_resources:"),
        "an inventory must blame list_resources: {rendered}",
    );
}

/// The retained set a recreate needs: every volume and the managed network, but
/// NOT the container, which is the thing being rebuilt.
///
/// Derived from the request rather than hardcoded, because `RetainedResources::new`
/// requires an exact match against the request's topology and the manifest decides
/// how many volumes that is.
fn retained_for(
    request: &gascan_core::runtime::CreateRequest,
) -> Vec<gascan_core::runtime::RuntimeResource> {
    use gascan_core::runtime::{
        ResourceIdentity, ResourceKind, ResourceOwnership, RuntimeResource,
    };

    let mut retained: Vec<RuntimeResource> = request
        .volumes()
        .iter()
        .map(|volume| {
            RuntimeResource::discovered(
                ResourceIdentity::new(ResourceKind::Volume, volume.name.clone())
                    .expect("a policy-compiled volume name is valid"),
                Some(request.id().clone()),
                ResourceOwnership::GasCanOwned,
            )
        })
        .collect();
    if let Some(name) = request.network().managed_name() {
        retained.push(RuntimeResource::discovered(
            ResourceIdentity::new(ResourceKind::Network, name.to_owned())
                .expect("a policy-compiled network name is valid"),
            Some(request.id().clone()),
            ResourceOwnership::GasCanOwned,
        ));
    }
    retained
}

#[tokio::test]
async fn create_container_sends_the_retained_resources_and_rebuilds_only_the_container() {
    use gascan_core::runtime::{RecreateRequest, RetainedResources};

    let (_root, request) = policy_request("recreating");
    let retained = RetainedResources::new(&request, retained_for(&request))
        .expect("the retained set matches the requested topology exactly");
    let recreate = RecreateRequest::new(request.clone(), retained).expect("a recreate request");
    let expected_retained = retained_for(&request).len();

    // A recreate's outcome is the container alone, which is why this path calls
    // `CreateOutcome::for_recreate` and not `CreateOutcome::new`. The paired test
    // below feeds it a full topology and requires a refusal.
    let engine = FakeEngine::default();
    *engine.create.lock().expect("test lock") = Some(v1::CreateResponse {
        outcome: Some(v1::create_response::Outcome::Created(v1::Created {
            created: vec![v1::Resource {
                identity: Some(v1::ResourceIdentity {
                    kind: v1::ResourceKind::Container as i32,
                    name: request.id().as_str().to_owned(),
                }),
                owner: Some(owner(request.id())),
            }],
        })),
    });

    let backend = ArcaBackend::new(engine);
    let outcome = backend
        .create_container(recreate)
        .await
        .expect("a container-only Created maps");
    assert_eq!(
        outcome.created().len(),
        1,
        "a recreate rebuilds the container alone"
    );

    let calls = backend.into_transport().calls();
    let [Call::CreateContainer(sent)] = calls.as_slice() else {
        panic!("create_container must reach the engine exactly once: {calls:?}");
    };
    assert!(
        sent.create.is_some(),
        "the compiled request travels with it"
    );
    assert_eq!(
        sent.retained.len(),
        expected_retained,
        "every retained resource is named, or the engine would recreate it",
    );
}

#[tokio::test]
async fn a_recreate_answered_with_the_whole_topology_is_refused() {
    use gascan_core::runtime::{RecreateRequest, RetainedResources};

    let (_root, request) = policy_request("recreating");
    let retained = RetainedResources::new(&request, retained_for(&request))
        .expect("the retained set matches the requested topology exactly");
    let recreate = RecreateRequest::new(request.clone(), retained).expect("a recreate request");

    // A full create outcome -- the container, every volume, and the managed
    // network -- is precisely what `CreateOutcome::new` accepts and what
    // `for_recreate` must refuse. That makes this the one test that can tell the
    // two constructors apart, and it is why `create_container` does not reuse
    // `create`'s: sharing them would silently accept a recreate that rebuilt
    // resources the caller asked it to retain.
    let engine = FakeEngine::default();
    *engine.create.lock().expect("test lock") = Some(v1::CreateResponse {
        outcome: Some(v1::create_response::Outcome::Created(created_for(&request))),
    });

    let failure = ArcaBackend::new(engine)
        .create_container(recreate)
        .await
        .expect_err("a recreate rebuilds the container and nothing else");
    assert_eq!(failure.code(), "invalid_state");
    assert!(
        failure
            .to_string()
            .contains("exactly the requested container"),
        "the refusal must name the recreate contract: {failure}",
    );
}

#[tokio::test]
async fn remove_names_exactly_the_resources_and_surfaces_an_ack_error() {
    use gascan_core::runtime::{
        RemoveRequest, ResourceIdentity, ResourceKind, ResourceOwnership, RuntimeResource,
    };

    let id = SandboxId::test("removing");
    let resource = |name: &str| {
        RuntimeResource::discovered(
            ResourceIdentity::new(ResourceKind::Volume, name).expect("a valid identity"),
            Some(id.clone()),
            ResourceOwnership::GasCanOwned,
        )
    };
    let build = || {
        RemoveRequest::from_resources(vec![resource("removing-data"), resource("removing-cache")])
            .expect("two owned resources")
    };

    let engine = FakeEngine::default();
    *engine.ack.lock().expect("test lock") = Some(ok_ack());
    let backend = ArcaBackend::new(engine);
    backend.remove(build()).await.expect("an ack is success");

    let calls = backend.into_transport().calls();
    let [Call::Remove(sent)] = calls.as_slice() else {
        panic!("remove must reach the engine exactly once: {calls:?}");
    };
    assert_eq!(
        sent.resources.len(),
        2,
        "exactly the named resources, no predicate form"
    );
    assert_eq!(
        sent.owner.as_ref().map(|owner| owner.sandbox_id.as_str()),
        Some(id.as_str()),
    );

    // The error arm of an Ack response, which nothing else exercises.
    let refusing = FakeEngine::default();
    *refusing.ack.lock().expect("test lock") = Some(v1::AckResponse {
        outcome: Some(v1::ack_response::Outcome::Error(FakeEngine::engine_error(
            "foreign_resource_refused",
        ))),
    });
    let error = ArcaBackend::new(refusing)
        .remove(build())
        .await
        .expect_err("the engine refused");
    assert_eq!(error.code(), "foreign_resource_refused");
}
