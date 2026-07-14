use gascan_proto::{
    API_MAJOR, CheckedEventSequence, CheckedOperationId, EventSequenceError, FILE_DESCRIPTOR_SET,
    HandshakeRejection, OperationIdError, SESSION_TOKEN_EMPTY, SOCKET_DIRECTORY_MODE, SOCKET_MODE,
    SessionTokenError, TransportSecurityError, local_transport_security, validate_api_major,
    validate_session_token, validate_transport_security,
};
use prost::Message;
use prost_types::{
    DescriptorProto, FileDescriptorProto, FileDescriptorSet, field_descriptor_proto,
};
use std::collections::HashSet;

fn api_file(descriptor: &FileDescriptorSet) -> &FileDescriptorProto {
    descriptor
        .file
        .iter()
        .find(|file| file.package.as_deref() == Some("gascan.v1"))
        .expect("gascan.v1 file")
}

fn message<'a>(file: &'a FileDescriptorProto, name: &str) -> &'a DescriptorProto {
    file.message_type
        .iter()
        .find(|message| message.name.as_deref() == Some(name))
        .expect("required message")
}

fn assert_field(
    message: &DescriptorProto,
    name: &str,
    number: i32,
    field_type: field_descriptor_proto::Type,
    oneof_index: Option<i32>,
) {
    let field = message
        .field
        .iter()
        .find(|field| field.name.as_deref() == Some(name))
        .expect("required field");
    assert_eq!(field.number, Some(number), "field number for {name}");
    assert_eq!(field.r#type(), field_type, "field type for {name}");
    assert_eq!(field.oneof_index, oneof_index, "oneof for {name}");
}

fn assert_type_name(message: &DescriptorProto, field_name: &str, expected: &str) {
    let field = message
        .field
        .iter()
        .find(|field| field.name.as_deref() == Some(field_name))
        .expect("required typed field");
    assert_eq!(field.type_name.as_deref(), Some(expected));
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
fn durable_event_sequences_are_positive() {
    assert_eq!(
        CheckedEventSequence::try_from(1_u64).map(CheckedEventSequence::get),
        Ok(1)
    );
    assert_eq!(
        CheckedEventSequence::try_from(0_u64),
        Err(EventSequenceError::Zero)
    );
}

#[test]
fn attach_session_tokens_and_local_security_are_validated() {
    assert_eq!(validate_session_token(b""), Err(SessionTokenError::Empty));
    assert_eq!(validate_session_token(b"opaque\0bytes"), Ok(()));
    assert_eq!(SESSION_TOKEN_EMPTY, "empty_session_token");

    assert_eq!(SOCKET_DIRECTORY_MODE, 0o700);
    assert_eq!(SOCKET_MODE, 0o600);
    let security = local_transport_security();
    assert!(security.local_only);
    assert!(security.require_same_user);
    assert_eq!(security.socket_directory_mode, 448);
    assert_eq!(security.socket_mode, 384);
    assert_eq!(validate_transport_security(&security), Ok(()));
    let mut insecure = security;
    insecure.require_same_user = false;
    assert_eq!(
        validate_transport_security(&insecure),
        Err(TransportSecurityError::SameUserNotRequired)
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
fn v1_descriptor_has_exact_event_and_attach_layout() {
    use field_descriptor_proto::Type::{Bytes, Enum, Message, String, Uint64};
    let descriptor =
        FileDescriptorSet::decode(FILE_DESCRIPTOR_SET).expect("descriptor must decode");
    let file = api_file(&descriptor);

    let event = message(file, "OperationEvent");
    assert_eq!(event.field.len(), 9);
    assert_field(event, "operation_id", 1, Message, None);
    assert_field(event, "timestamp", 2, Message, None);
    assert_field(event, "phase", 3, String, None);
    assert_field(event, "payload", 4, Bytes, None);
    assert_field(event, "error", 5, Message, None);
    assert_field(event, "sequence", 6, Uint64, None);
    assert_field(event, "status", 7, Enum, None);
    assert_field(event, "content_type", 8, String, None);
    assert_field(event, "session_token", 9, Bytes, None);
    assert_type_name(event, "operation_id", ".gascan.v1.OperationId");
    assert_type_name(event, "timestamp", ".google.protobuf.Timestamp");
    assert_type_name(event, "error", ".gascan.v1.Error");
    assert_type_name(event, "status", ".gascan.v1.OperationStatus");
    assert!(
        event
            .reserved_range
            .iter()
            .any(|range| range.start == Some(10) && range.end == Some(11))
    );

    let client = message(file, "ClientFrame");
    assert_eq!(client.field.len(), 5);
    assert_eq!(client.oneof_decl.len(), 1);
    assert_eq!(client.oneof_decl[0].name.as_deref(), Some("frame"));
    assert_field(client, "stdin", 1, Bytes, Some(0));
    assert_field(client, "resize", 2, Message, Some(0));
    assert_field(client, "signal", 3, Message, Some(0));
    assert_field(client, "close", 4, Message, Some(0));
    assert_field(client, "session_token", 6, Bytes, None);
    assert_type_name(client, "resize", ".gascan.v1.Resize");
    assert_type_name(client, "signal", ".gascan.v1.Signal");
    assert_type_name(client, "close", ".gascan.v1.Close");
    assert!(
        client
            .reserved_range
            .iter()
            .any(|range| range.start == Some(5) && range.end == Some(6))
    );

    let server = message(file, "ServerFrame");
    assert_eq!(server.field.len(), 4);
    assert_eq!(server.oneof_decl.len(), 1);
    assert_field(server, "stdout", 1, Bytes, Some(0));
    assert_field(server, "stderr", 2, Bytes, Some(0));
    assert_field(server, "exit", 3, Message, Some(0));
    assert_field(server, "error", 4, Message, Some(0));
    assert_type_name(server, "exit", ".gascan.v1.Exit");
    assert_type_name(server, "error", ".gascan.v1.Error");

    let manifest = message(file, "ManifestPayload");
    assert_eq!(manifest.field.len(), 2);
    assert_field(manifest, "content", 1, Bytes, None);
    assert_field(manifest, "format", 2, String, None);
    let command = message(file, "CommandPayload");
    assert_eq!(command.field.len(), 2);
    assert_field(command, "argv", 1, Bytes, None);
    assert!(command.field[0].label() == prost_types::field_descriptor_proto::Label::Repeated);
    assert_field(command, "stdin", 2, Bytes, None);
}

#[test]
fn v1_descriptor_keeps_handshake_transport_and_enum_numbers_stable() {
    use field_descriptor_proto::Type::{Bool, Message, String, Uint32};
    let descriptor =
        FileDescriptorSet::decode(FILE_DESCRIPTOR_SET).expect("descriptor must decode");
    let file = api_file(&descriptor);
    let request = message(file, "HandshakeRequest");
    assert_eq!(request.field.len(), 3);
    assert_field(request, "api_major", 1, Uint32, None);
    assert_field(request, "api_minor", 2, Uint32, None);
    assert_field(request, "requested_capabilities", 3, String, None);
    let response = message(file, "HandshakeResponse");
    assert_eq!(response.field.len(), 5);
    assert_field(response, "api_major", 1, Uint32, None);
    assert_field(response, "api_minor", 2, Uint32, None);
    assert_field(response, "capabilities", 3, Message, None);
    assert_field(response, "transport_security", 4, Message, None);
    assert_field(response, "rejection", 5, Message, None);
    assert_type_name(response, "capabilities", ".gascan.v1.Capability");
    assert_type_name(
        response,
        "transport_security",
        ".gascan.v1.TransportSecurity",
    );
    assert_type_name(response, "rejection", ".gascan.v1.Error");
    let security = message(file, "TransportSecurity");
    assert_eq!(security.field.len(), 4);
    assert_field(security, "local_only", 1, Bool, None);
    assert_field(security, "socket_directory_mode", 2, Uint32, None);
    assert_field(security, "socket_mode", 3, Uint32, None);
    assert_field(security, "require_same_user", 4, Bool, None);
    for item in [request, response, security] {
        assert!(item.reserved_range.iter().any(|range| {
            range
                .start
                .zip(range.end)
                .is_some_and(|(start, end)| end == start + 1)
        }));
    }

    let status = file
        .enum_type
        .iter()
        .find(|item| item.name.as_deref() == Some("OperationStatus"))
        .expect("OperationStatus");
    let values: Vec<_> = status
        .value
        .iter()
        .map(|value| (value.name.as_deref(), value.number))
        .collect();
    assert_eq!(
        values,
        vec![
            (Some("OPERATION_STATUS_UNSPECIFIED"), Some(0)),
            (Some("OPERATION_STATUS_PENDING"), Some(1)),
            (Some("OPERATION_STATUS_COMPLETED"), Some(2)),
            (Some("OPERATION_STATUS_FAILED"), Some(3))
        ]
    );

    for (name, expected) in [
        ("DesiredState", vec![0, 1, 2, 3]),
        ("ActualState", vec![0, 1, 2, 3, 4, 5, 6]),
    ] {
        let item = file
            .enum_type
            .iter()
            .find(|item| item.name.as_deref() == Some(name))
            .expect("state enum");
        assert_eq!(
            item.value
                .iter()
                .filter_map(|value| value.number)
                .collect::<Vec<_>>(),
            expected
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
