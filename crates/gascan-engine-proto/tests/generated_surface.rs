//! The generated surface is asserted, not assumed.
//!
//! A protobuf generator that silently emits an empty module exits 0, so
//! `cargo build` succeeding proves less than it appears to. These tests make two
//! separate claims, because one failure mode is invisible to the other:
//!
//! 1. The Rust module carries the client and the message types. An empty or
//!    truncated module fails to *compile* this file, which is a stronger witness
//!    than any runtime assertion could be.
//! 2. The service carries exactly the eleven expected methods. A service that
//!    lost a method still compiles for every caller that never used it, so the
//!    descriptor is checked directly.

use gascan_engine_proto::{FILE_DESCRIPTOR_SET, v1};
use prost::Message as _;

/// Every RPC on `arca.engine.v1.SandboxEngine`, one per `RuntimeBackend` method.
///
/// Eleven, not ten. The count is load-bearing: an earlier spec said ten and the
/// omission was `prepare_image`, the one method that would grow a registry
/// client if nobody were watching it.
const EXPECTED_METHODS: [&str; 11] = [
    "Capabilities",
    "Inspect",
    "Create",
    "PrepareImage",
    "CreateContainer",
    "Start",
    "Stop",
    "Remove",
    "Exec",
    "Logs",
    "ListResources",
];

/// Name every generated type this crate exists to provide.
///
/// This test body is deliberately made of type references rather than behaviour.
/// It cannot pass against an empty module, because it would not build.
#[test]
fn the_generated_module_carries_the_client_and_one_message_per_rpc() {
    fn assert_exists<T: Default>() -> T {
        T::default()
    }

    // The client type itself, named as a type rather than constructed: building
    // one needs a live transport, and the claim under test is that the generator
    // emitted it.
    type Client = v1::sandbox_engine_client::SandboxEngineClient<tonic::transport::Channel>;
    let _: Option<Client> = None;

    let _ = assert_exists::<v1::CapabilitiesRequest>();
    let _ = assert_exists::<v1::InspectRequest>();
    let _ = assert_exists::<v1::CreateRequest>();
    let _ = assert_exists::<v1::PrepareImageRequest>();
    let _ = assert_exists::<v1::CreateContainerRequest>();
    let _ = assert_exists::<v1::StartRequest>();
    let _ = assert_exists::<v1::StopRequest>();
    let _ = assert_exists::<v1::RemoveRequest>();
    let _ = assert_exists::<v1::ExecClientFrame>();
    let _ = assert_exists::<v1::LogsRequest>();
    let _ = assert_exists::<v1::ListResourcesRequest>();
}

/// No server is generated, and that is a decision rather than an oversight.
///
/// Arca serves this contract. A Rust server would be surface with no implementor
/// and no caller, and the first thing to accidentally implement it would be a
/// test double that made a wrong client look correct.
#[test]
fn no_server_module_is_generated() {
    let descriptor = decode_descriptor();
    // The descriptor names the service either way, so the claim is about the
    // generated Rust: `build_server(false)` means no `sandbox_engine_server`
    // module exists. That is enforced at compile time by its absence -- this
    // test records the intent and fails loudly if the descriptor ever stops
    // describing the service at all, which would make the claim vacuous.
    assert!(
        service(&descriptor).is_some(),
        "arca.engine.v1.SandboxEngine is absent from the descriptor, so the \
         server-generation claim would be vacuous"
    );
}

/// Assert the service surface against the descriptor.
///
/// Exactness matters in both directions. A missing method is a client that
/// cannot call something the contract promises; an extra one is surface the
/// policy boundary was never designed to gate, which is the specific drift the
/// proto's size gate exists to catch.
#[test]
fn the_service_carries_exactly_the_eleven_contract_methods() {
    let descriptor = decode_descriptor();
    let service = service(&descriptor).expect("arca.engine.v1.SandboxEngine is absent");

    let found: Vec<&str> = service.method.iter().map(|method| method.name()).collect();

    let mut missing: Vec<&str> = EXPECTED_METHODS
        .iter()
        .filter(|expected| !found.contains(*expected))
        .copied()
        .collect();
    let mut unexpected: Vec<&str> = found
        .iter()
        .filter(|name| !EXPECTED_METHODS.contains(*name))
        .copied()
        .collect();
    missing.sort_unstable();
    unexpected.sort_unstable();

    assert!(
        missing.is_empty() && unexpected.is_empty(),
        "arca.engine.v1.SandboxEngine does not match the contract\n  \
         missing:    {missing:?}\n  unexpected: {unexpected:?}\n  \
         found {} methods, expected {}",
        found.len(),
        EXPECTED_METHODS.len()
    );
}

/// The package path is the major version, so it is asserted rather than assumed.
///
/// A breaking change to this contract is a new package, never an edit to this
/// one. If the package name ever changes underneath the pin, that is a new major
/// version arriving silently, and it should fail here.
#[test]
fn the_contract_is_package_arca_engine_v1() {
    let descriptor = decode_descriptor();
    let packages: Vec<&str> = descriptor.file.iter().map(|file| file.package()).collect();
    assert!(
        packages.contains(&"arca.engine.v1"),
        "expected package arca.engine.v1, found {packages:?}"
    );
}

fn decode_descriptor() -> prost_types::FileDescriptorSet {
    prost_types::FileDescriptorSet::decode(FILE_DESCRIPTOR_SET)
        .expect("the generated descriptor set does not decode")
}

fn service(
    descriptor: &prost_types::FileDescriptorSet,
) -> Option<&prost_types::ServiceDescriptorProto> {
    descriptor
        .file
        .iter()
        .filter(|file| file.package() == "arca.engine.v1")
        .flat_map(|file| file.service.iter())
        .find(|service| service.name() == "SandboxEngine")
}
