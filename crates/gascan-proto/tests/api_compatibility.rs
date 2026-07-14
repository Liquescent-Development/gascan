use gascan_proto::{
    API_MAJOR, CheckedOperationId, FILE_DESCRIPTOR_SET, HandshakeRejection, OperationIdError,
    validate_api_major,
};
use prost::Message;
use prost_types::FileDescriptorSet;
use std::collections::HashSet;

fn descriptor_debug(descriptor: &[u8]) -> Result<String, prost::DecodeError> {
    FileDescriptorSet::decode(descriptor).map(|descriptor| format!("{descriptor:#?}"))
}

#[test]
fn v1_descriptor_contains_required_rpc_surface() {
    let descriptor =
        FileDescriptorSet::decode(FILE_DESCRIPTOR_SET).expect("descriptor must decode");
    let service = descriptor
        .file
        .iter()
        .flat_map(|file| &file.service)
        .find(|service| service.name.as_deref() == Some("GasCan"))
        .expect("GasCan service");
    for method in &service.method {
        let name = method.name.as_deref().expect("method name");
        let expected = match name {
            "Handshake" | "Status" | "List" | "Doctor" => (false, false),
            "Up" | "Apply" | "Run" | "Shell" | "Down" | "Destroy" | "Logs" => (false, true),
            "Attach" => (true, true),
            other => panic!("unexpected RPC {other}"),
        };
        assert_eq!(
            (method.client_streaming(), method.server_streaming()),
            expected,
            "wrong streaming shape for {name}"
        );
    }
    assert_eq!(service.method.len(), 12);
}

#[test]
fn operation_ids_are_nonzero_and_fit_the_durable_signed_range() {
    assert_eq!(
        CheckedOperationId::try_from(1_u64).map(CheckedOperationId::get),
        Ok(1)
    );
    assert_eq!(
        CheckedOperationId::try_from(0_u64),
        Err(OperationIdError::Zero)
    );
    assert_eq!(
        CheckedOperationId::try_from((i64::MAX as u64) + 1),
        Err(OperationIdError::OutOfRange)
    );
}

#[test]
fn public_error_codes_are_stable_and_unique() {
    let codes = gascan_proto::error_code::ALL;
    assert!(codes.contains(&"incompatible_api_major"));
    assert!(codes.contains(&"invalid_request"));
    assert!(codes.contains(&"sandbox_not_found"));
    assert!(codes.contains(&"operation_conflict"));
    assert_eq!(
        codes.len(),
        codes.iter().copied().collect::<HashSet<_>>().len()
    );
}

#[test]
fn v1_descriptor_carries_explicit_protocol_contracts() {
    let text = descriptor_debug(FILE_DESCRIPTOR_SET).expect("descriptor must decode");
    for contract in [
        "OperationId",
        "Timestamp",
        "DesiredState",
        "ActualState",
        "Capability",
        "Error",
        "TransportSecurity",
        "stdin",
        "resize",
        "signal",
        "close",
        "stdout",
        "stderr",
        "exit",
        "error",
    ] {
        assert!(
            text.contains(contract),
            "missing protocol contract {contract}"
        );
    }
}

#[test]
fn handshake_rejects_a_different_api_major_with_a_stable_code() {
    assert_eq!(API_MAJOR, 1);
    assert_eq!(validate_api_major(API_MAJOR), Ok(()));
    assert_eq!(
        validate_api_major(API_MAJOR + 1),
        Err(HandshakeRejection::IncompatibleApiMajor {
            supported: API_MAJOR,
            requested: API_MAJOR + 1,
        })
    );
    assert_eq!(
        HandshakeRejection::IncompatibleApiMajor {
            supported: API_MAJOR,
            requested: API_MAJOR + 1,
        }
        .code(),
        "incompatible_api_major"
    );
}
