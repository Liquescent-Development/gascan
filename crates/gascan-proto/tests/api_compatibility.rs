use gascan_proto::{
    API_MAJOR, AttachSessionBinder, AttachSessionError, CheckedEventSequence, CheckedOperationId,
    EventSequenceError, FILE_DESCRIPTOR_SET, HandshakeRejection, OperationIdError,
    SESSION_TOKEN_EMPTY, SOCKET_DIRECTORY_MODE, SOCKET_MODE, SessionTokenError,
    TransportSecurityError, local_transport_security, validate_api_major, validate_session_token,
    validate_transport_security,
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

type FieldSpec<'a> = (
    &'a str,
    i32,
    field_descriptor_proto::Type,
    field_descriptor_proto::Label,
    Option<i32>,
    Option<&'a str>,
);
type MessageSpec<'a> = (
    &'a str,
    &'a [FieldSpec<'a>],
    &'a [&'a str],
    &'a [(i32, i32)],
);

fn assert_message_exact(
    file: &FileDescriptorProto,
    name: &str,
    fields: &[FieldSpec<'_>],
    oneofs: &[&str],
    reserved: &[(i32, i32)],
) {
    let descriptor = message(file, name);
    assert_eq!(
        descriptor.field.len(),
        fields.len(),
        "field count for {name}"
    );
    for (actual, expected) in descriptor.field.iter().zip(fields) {
        assert_eq!(
            actual.name.as_deref(),
            Some(expected.0),
            "field name in {name}"
        );
        assert_eq!(actual.number, Some(expected.1), "field number in {name}");
        assert_eq!(actual.r#type(), expected.2, "field type in {name}");
        assert_eq!(actual.label(), expected.3, "field label in {name}");
        assert_eq!(actual.oneof_index, expected.4, "field oneof in {name}");
        assert_eq!(
            actual.type_name.as_deref(),
            expected.5,
            "type name in {name}"
        );
    }
    assert_eq!(
        descriptor
            .oneof_decl
            .iter()
            .map(|oneof| oneof.name.as_deref().expect("oneof name"))
            .collect::<Vec<_>>(),
        oneofs,
        "oneofs for {name}"
    );
    assert_eq!(
        descriptor
            .reserved_range
            .iter()
            .map(|range| (
                range.start.expect("reserved start"),
                range.end.expect("reserved end")
            ))
            .collect::<Vec<_>>(),
        reserved,
        "reserved ranges for {name}"
    );
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
    assert_eq!(
        CheckedEventSequence::try_from(i64::MAX as u64).map(CheckedEventSequence::get),
        Ok(i64::MAX)
    );
    assert_eq!(
        CheckedEventSequence::try_from((i64::MAX as u64) + 1),
        Err(EventSequenceError::OutOfRange)
    );
}

#[test]
fn attach_stream_binds_once_and_rejects_token_changes() {
    let mut binder = AttachSessionBinder::new();
    assert_eq!(binder.validate_frame(b""), Err(AttachSessionError::Empty));
    assert_eq!(binder.validate_frame(b"session-a"), Ok(()));
    assert_eq!(binder.session_token(), Some(&b"session-a"[..]));
    assert_eq!(binder.validate_frame(b"session-a"), Ok(()));
    assert_eq!(
        binder.validate_frame(b"session-b"),
        Err(AttachSessionError::Mismatch)
    );
    assert_eq!(binder.session_token(), Some(&b"session-a"[..]));
    assert_eq!(
        AttachSessionError::Mismatch.code(),
        "session_token_mismatch"
    );
    assert!(gascan_proto::error_code::ALL.contains(&"unknown_session_token"));
    assert!(gascan_proto::error_code::ALL.contains(&"expired_session_token"));
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
    assert!(codes.contains(&"disk_control_unsupported"));
    assert!(codes.contains(&"sandbox_not_found"));
    assert!(codes.contains(&"operation_conflict"));
    assert_eq!(
        codes.len(),
        codes.iter().copied().collect::<HashSet<_>>().len()
    );
}

#[test]
fn v1_descriptor_has_exact_event_and_attach_layout() {
    use field_descriptor_proto::Type::{Bool, Bytes, Enum, Message, String, Uint64};
    let descriptor =
        FileDescriptorSet::decode(FILE_DESCRIPTOR_SET).expect("descriptor must decode");
    let file = api_file(&descriptor);

    let event = message(file, "OperationEvent");
    assert_eq!(event.field.len(), 10);
    assert_field(event, "operation_id", 1, Message, None);
    assert_field(event, "timestamp", 2, Message, None);
    assert_field(event, "phase", 3, String, None);
    assert_field(event, "payload", 4, Bytes, None);
    assert_field(event, "error", 5, Message, None);
    assert_field(event, "sequence", 6, Uint64, None);
    assert_field(event, "status", 7, Enum, None);
    assert_field(event, "content_type", 8, String, None);
    assert_field(event, "session_token", 9, Bytes, None);
    assert_field(event, "provision_step", 11, Enum, None);
    assert_type_name(event, "operation_id", ".gascan.v1.OperationId");
    assert_type_name(event, "timestamp", ".google.protobuf.Timestamp");
    assert_type_name(event, "error", ".gascan.v1.Error");
    assert_type_name(event, "status", ".gascan.v1.OperationStatus");
    assert_type_name(event, "provision_step", ".gascan.v1.ProvisionStep");
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
    assert_eq!(command.field.len(), 3);
    assert_field(command, "argv", 1, Bytes, None);
    assert!(command.field[0].label() == prost_types::field_descriptor_proto::Label::Repeated);
    assert_field(command, "environment", 3, Message, None);
    assert_field(command, "tty", 4, Bool, None);
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
    assert_eq!(response.field.len(), 9);
    assert_field(response, "api_major", 1, Uint32, None);
    assert_field(response, "api_minor", 2, Uint32, None);
    assert_field(response, "capabilities", 3, Message, None);
    assert_field(response, "transport_security", 4, Message, None);
    assert_field(response, "rejection", 5, Message, None);
    assert_field(response, "daemon_instance_token", 6, String, None);
    assert_field(response, "daemon_pid", 7, Uint32, None);
    assert_field(response, "daemon_executable", 8, String, None);
    assert_field(response, "daemon_start_identity", 9, String, None);
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
fn v1_descriptor_exactly_covers_every_exported_message_enum_and_rpc() {
    use field_descriptor_proto::{
        Label::{Optional as O, Repeated as R},
        Type::{Bool, Bytes, Enum, Int32, Message, String, Uint32, Uint64},
    };
    macro_rules! f {
        ($name:literal, $number:literal, $kind:expr) => {
            ($name, $number, $kind, O, None, None)
        };
        ($name:literal, $number:literal, $kind:expr, $label:expr, $oneof:expr, $type_name:expr) => {
            ($name, $number, $kind, $label, $oneof, $type_name)
        };
    }
    let descriptor =
        FileDescriptorSet::decode(FILE_DESCRIPTOR_SET).expect("descriptor must decode");
    let file = api_file(&descriptor);

    let messages: &[MessageSpec<'_>] = &[
        ("OperationId", &[f!("value", 1, Uint64)], &[], &[(2, 3)]),
        (
            "Capability",
            &[
                f!("name", 1, String),
                f!("available", 2, Bool),
                f!("detail", 3, String),
            ],
            &[],
            &[(4, 5)],
        ),
        (
            "Error",
            &[
                f!("code", 1, String),
                f!("message", 2, String),
                f!("details", 3, Bytes),
            ],
            &[],
            &[(4, 5)],
        ),
        (
            "TransportSecurity",
            &[
                f!("local_only", 1, Bool),
                f!("socket_directory_mode", 2, Uint32),
                f!("socket_mode", 3, Uint32),
                f!("require_same_user", 4, Bool),
            ],
            &[],
            &[(5, 6)],
        ),
        (
            "HandshakeRequest",
            &[
                f!("api_major", 1, Uint32),
                f!("api_minor", 2, Uint32),
                f!("requested_capabilities", 3, String, R, None, None),
            ],
            &[],
            &[(4, 5)],
        ),
        (
            "HandshakeResponse",
            &[
                f!("api_major", 1, Uint32),
                f!("api_minor", 2, Uint32),
                f!(
                    "capabilities",
                    3,
                    Message,
                    R,
                    None,
                    Some(".gascan.v1.Capability")
                ),
                f!(
                    "transport_security",
                    4,
                    Message,
                    O,
                    None,
                    Some(".gascan.v1.TransportSecurity")
                ),
                f!("rejection", 5, Message, O, None, Some(".gascan.v1.Error")),
                f!("daemon_instance_token", 6, String),
                f!("daemon_pid", 7, Uint32),
                f!("daemon_executable", 8, String),
                f!("daemon_start_identity", 9, String),
            ],
            &[],
            &[(10, 11)],
        ),
        (
            "SandboxSelector",
            &[f!("sandbox_id", 1, String)],
            &[],
            &[(2, 3)],
        ),
        (
            "ManifestPayload",
            &[f!("content", 1, Bytes), f!("format", 2, String)],
            &[],
            &[(3, 4)],
        ),
        (
            "EnvironmentVariable",
            &[f!("name", 1, String), f!("value", 2, String)],
            &[],
            &[(3, 4)],
        ),
        (
            "CommandPayload",
            &[
                f!("argv", 1, Bytes, R, None, None),
                f!(
                    "environment",
                    3,
                    Message,
                    R,
                    None,
                    Some(".gascan.v1.EnvironmentVariable")
                ),
                f!("tty", 4, Bool),
            ],
            &[],
            &[(2, 3), (5, 6)],
        ),
        (
            "UpRequest",
            &[f!("project_root", 1, String)],
            &[],
            &[(2, 3)],
        ),
        (
            "ApplyRequest",
            &[f!("project_root", 1, String)],
            &[],
            &[(2, 3)],
        ),
        (
            "RunRequest",
            &[
                f!(
                    "sandbox",
                    1,
                    Message,
                    O,
                    None,
                    Some(".gascan.v1.SandboxSelector")
                ),
                f!(
                    "command",
                    2,
                    Message,
                    O,
                    None,
                    Some(".gascan.v1.CommandPayload")
                ),
            ],
            &[],
            &[(3, 4)],
        ),
        (
            "ShellRequest",
            &[
                f!(
                    "sandbox",
                    1,
                    Message,
                    O,
                    None,
                    Some(".gascan.v1.SandboxSelector")
                ),
                f!(
                    "command",
                    2,
                    Message,
                    O,
                    None,
                    Some(".gascan.v1.CommandPayload")
                ),
            ],
            &[],
            &[(3, 4)],
        ),
        (
            "DownRequest",
            &[f!(
                "sandbox",
                1,
                Message,
                O,
                None,
                Some(".gascan.v1.SandboxSelector")
            )],
            &[],
            &[(2, 3)],
        ),
        (
            "DestroyRequest",
            &[f!(
                "sandbox",
                1,
                Message,
                O,
                None,
                Some(".gascan.v1.SandboxSelector")
            )],
            &[],
            &[(2, 3)],
        ),
        (
            "LogsRequest",
            &[
                f!(
                    "sandbox",
                    1,
                    Message,
                    O,
                    None,
                    Some(".gascan.v1.SandboxSelector")
                ),
                f!(
                    "since",
                    2,
                    Message,
                    O,
                    None,
                    Some(".google.protobuf.Timestamp")
                ),
                f!("follow", 3, Bool),
            ],
            &[],
            &[(4, 5)],
        ),
        (
            "OperationEvent",
            &[
                f!(
                    "operation_id",
                    1,
                    Message,
                    O,
                    None,
                    Some(".gascan.v1.OperationId")
                ),
                f!(
                    "timestamp",
                    2,
                    Message,
                    O,
                    None,
                    Some(".google.protobuf.Timestamp")
                ),
                f!("phase", 3, String),
                f!("payload", 4, Bytes),
                f!("error", 5, Message, O, None, Some(".gascan.v1.Error")),
                f!("sequence", 6, Uint64),
                f!(
                    "status",
                    7,
                    Enum,
                    O,
                    None,
                    Some(".gascan.v1.OperationStatus")
                ),
                f!("content_type", 8, String),
                f!("session_token", 9, Bytes),
                f!(
                    "provision_step",
                    11,
                    Enum,
                    O,
                    None,
                    Some(".gascan.v1.ProvisionStep")
                ),
            ],
            &[],
            &[(10, 11)],
        ),
        (
            "StatusRequest",
            &[f!(
                "sandbox",
                1,
                Message,
                O,
                None,
                Some(".gascan.v1.SandboxSelector")
            )],
            &[],
            &[(2, 3)],
        ),
        (
            "SandboxStatus",
            &[
                f!("sandbox_id", 1, String),
                f!(
                    "desired_state",
                    2,
                    Enum,
                    O,
                    None,
                    Some(".gascan.v1.DesiredState")
                ),
                f!(
                    "actual_state",
                    3,
                    Enum,
                    O,
                    None,
                    Some(".gascan.v1.ActualState")
                ),
                f!(
                    "last_operation_id",
                    4,
                    Message,
                    O,
                    None,
                    Some(".gascan.v1.OperationId")
                ),
                f!(
                    "updated_at",
                    5,
                    Message,
                    O,
                    None,
                    Some(".google.protobuf.Timestamp")
                ),
                f!(
                    "capabilities",
                    6,
                    Message,
                    R,
                    None,
                    Some(".gascan.v1.Capability")
                ),
            ],
            &[],
            &[(7, 8)],
        ),
        (
            "StatusResponse",
            &[f!(
                "sandbox",
                1,
                Message,
                O,
                None,
                Some(".gascan.v1.SandboxStatus")
            )],
            &[],
            &[(2, 3)],
        ),
        ("ListRequest", &[], &[], &[(1, 2)]),
        (
            "ListResponse",
            &[f!(
                "sandboxes",
                1,
                Message,
                R,
                None,
                Some(".gascan.v1.SandboxStatus")
            )],
            &[],
            &[(2, 3)],
        ),
        ("DoctorRequest", &[], &[], &[(1, 2)]),
        (
            "DoctorResponse",
            &[
                f!(
                    "capabilities",
                    1,
                    Message,
                    R,
                    None,
                    Some(".gascan.v1.Capability")
                ),
                f!("findings", 2, Message, R, None, Some(".gascan.v1.Error")),
            ],
            &[],
            &[(3, 4)],
        ),
        (
            "Resize",
            &[f!("columns", 1, Uint32), f!("rows", 2, Uint32)],
            &[],
            &[(3, 4)],
        ),
        ("Signal", &[f!("number", 1, Int32)], &[], &[(2, 3)]),
        ("Close", &[], &[], &[(1, 2)]),
        (
            "ClientFrame",
            &[
                f!("stdin", 1, Bytes, O, Some(0), None),
                f!("resize", 2, Message, O, Some(0), Some(".gascan.v1.Resize")),
                f!("signal", 3, Message, O, Some(0), Some(".gascan.v1.Signal")),
                f!("close", 4, Message, O, Some(0), Some(".gascan.v1.Close")),
                f!("session_token", 6, Bytes),
            ],
            &["frame"],
            &[(5, 6)],
        ),
        (
            "Exit",
            &[f!("code", 1, Int32), f!("signal", 2, Int32)],
            &[],
            &[(3, 4)],
        ),
        (
            "ServerFrame",
            &[
                f!("stdout", 1, Bytes, O, Some(0), None),
                f!("stderr", 2, Bytes, O, Some(0), None),
                f!("exit", 3, Message, O, Some(0), Some(".gascan.v1.Exit")),
                f!("error", 4, Message, O, Some(0), Some(".gascan.v1.Error")),
            ],
            &["frame"],
            &[(5, 6)],
        ),
    ];
    assert_eq!(file.message_type.len(), messages.len());
    for (name, fields, oneofs, reserved) in messages {
        assert_message_exact(file, name, fields, oneofs, reserved);
    }

    let enums: &[(&str, &[(&str, i32)])] = &[
        (
            "DesiredState",
            &[
                ("DESIRED_STATE_UNSPECIFIED", 0),
                ("DESIRED_STATE_RUNNING", 1),
                ("DESIRED_STATE_STOPPED", 2),
                ("DESIRED_STATE_ABSENT", 3),
            ],
        ),
        (
            "ActualState",
            &[
                ("ACTUAL_STATE_UNSPECIFIED", 0),
                ("ACTUAL_STATE_PENDING", 1),
                ("ACTUAL_STATE_RUNNING", 2),
                ("ACTUAL_STATE_STOPPED", 3),
                ("ACTUAL_STATE_ABSENT", 4),
                ("ACTUAL_STATE_FAILED", 5),
                ("ACTUAL_STATE_UNKNOWN", 6),
            ],
        ),
        (
            "OperationStatus",
            &[
                ("OPERATION_STATUS_UNSPECIFIED", 0),
                ("OPERATION_STATUS_PENDING", 1),
                ("OPERATION_STATUS_COMPLETED", 2),
                ("OPERATION_STATUS_FAILED", 3),
            ],
        ),
        (
            "ProvisionStep",
            &[
                ("PROVISION_STEP_UNSPECIFIED", 0),
                ("PROVISION_STEP_WRITE_SAFE_MISE_CONFIG", 1),
                ("PROVISION_STEP_INSTALL_TOOLS", 2),
                ("PROVISION_STEP_RUN_SETUP", 3),
                ("PROVISION_STEP_VERIFY_GASCAMP", 4),
                ("PROVISION_STEP_HEALTH_CHECK", 5),
            ],
        ),
    ];
    assert_eq!(file.enum_type.len(), enums.len());
    for (name, expected) in enums {
        let actual = file
            .enum_type
            .iter()
            .find(|item| item.name.as_deref() == Some(name))
            .expect("exported enum");
        assert_eq!(actual.value.len(), expected.len());
        for (value, (expected_name, expected_number)) in actual.value.iter().zip(*expected) {
            assert_eq!(value.name.as_deref(), Some(*expected_name));
            assert_eq!(value.number, Some(*expected_number));
        }
    }

    let service = &file.service[0];
    let rpcs = [
        (
            "Handshake",
            ".gascan.v1.HandshakeRequest",
            ".gascan.v1.HandshakeResponse",
            false,
            false,
        ),
        (
            "Status",
            ".gascan.v1.StatusRequest",
            ".gascan.v1.StatusResponse",
            false,
            false,
        ),
        (
            "List",
            ".gascan.v1.ListRequest",
            ".gascan.v1.ListResponse",
            false,
            false,
        ),
        (
            "Doctor",
            ".gascan.v1.DoctorRequest",
            ".gascan.v1.DoctorResponse",
            false,
            false,
        ),
        (
            "Up",
            ".gascan.v1.UpRequest",
            ".gascan.v1.OperationEvent",
            false,
            true,
        ),
        (
            "Apply",
            ".gascan.v1.ApplyRequest",
            ".gascan.v1.OperationEvent",
            false,
            true,
        ),
        (
            "Run",
            ".gascan.v1.RunRequest",
            ".gascan.v1.OperationEvent",
            false,
            true,
        ),
        (
            "Shell",
            ".gascan.v1.ShellRequest",
            ".gascan.v1.OperationEvent",
            false,
            true,
        ),
        (
            "Down",
            ".gascan.v1.DownRequest",
            ".gascan.v1.OperationEvent",
            false,
            true,
        ),
        (
            "Destroy",
            ".gascan.v1.DestroyRequest",
            ".gascan.v1.OperationEvent",
            false,
            true,
        ),
        (
            "Logs",
            ".gascan.v1.LogsRequest",
            ".gascan.v1.OperationEvent",
            false,
            true,
        ),
        (
            "Attach",
            ".gascan.v1.ClientFrame",
            ".gascan.v1.ServerFrame",
            true,
            true,
        ),
    ];
    assert_eq!(file.service.len(), 1);
    assert_eq!(service.name.as_deref(), Some("GasCan"));
    assert_eq!(service.method.len(), rpcs.len());
    for (method, (name, input, output, client, server)) in service.method.iter().zip(rpcs) {
        assert_eq!(method.name.as_deref(), Some(name));
        assert_eq!(method.input_type.as_deref(), Some(input));
        assert_eq!(method.output_type.as_deref(), Some(output));
        assert_eq!(method.client_streaming(), client);
        assert_eq!(method.server_streaming(), server);
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

#[test]
fn request_validation_codes_are_public_and_unique() {
    let codes = gascan_proto::error_code::ALL;
    assert!(codes.contains(&"invalid_manifest"));
    assert!(codes.contains(&"invalid_project_root"));
    assert!(codes.contains(&"storage_change_requires_recreate"));
    assert_eq!(
        gascan_proto::error_code::STORAGE_CHANGE_REQUIRES_RECREATE,
        "storage_change_requires_recreate"
    );
    assert_eq!(
        codes.len(),
        codes.iter().copied().collect::<HashSet<_>>().len()
    );
}

#[test]
fn error_detail_round_trips_the_human_cause() {
    let encoded = gascan_proto::error_detail::encode(
        gascan_proto::error_code::INVALID_MANIFEST,
        "unknown variant `kiener`, expected `workspace` or `root`",
    );
    assert_eq!(
        gascan_proto::error_detail::decode_message(&encoded).as_deref(),
        Some("unknown variant `kiener`, expected `workspace` or `root`")
    );
}

#[test]
fn error_detail_round_trips_structured_failure_details() {
    let details = br#"{"changes":[{"volume":"tools","recorded_bytes":10737418240,"requested_bytes":21474836480}]}"#;
    let encoded = gascan_proto::error_detail::encode_with_details(
        gascan_proto::error_code::STORAGE_CHANGE_REQUIRES_RECREATE,
        "storage settings changed",
        details,
    );
    assert_eq!(
        gascan_proto::error_detail::decode_details(&encoded).as_deref(),
        Some(details.as_slice())
    );
}

#[test]
fn error_detail_degrades_instead_of_failing() {
    // Absent details: an older daemon sends none, and the caller must fall back
    // to the stable code rather than error.
    assert_eq!(gascan_proto::error_detail::decode_message(&[]), None);
    // Truncated: field 1 is a length-5 string with no bytes following.
    assert_eq!(
        gascan_proto::error_detail::decode_message(&[0x0a, 0x05]),
        None
    );
    // Well-formed but empty message: nothing useful to show.
    let empty = gascan_proto::error_detail::encode("invalid_manifest", "");
    assert_eq!(gascan_proto::error_detail::decode_message(&empty), None);
}
