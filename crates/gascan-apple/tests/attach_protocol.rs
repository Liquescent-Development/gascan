use gascan_apple::{AttachInput, AttachOutput, HELPER_PROTOCOL_VERSION, HelperInput, HelperOutput};
use serde_json::json;

#[test]
fn start_is_versioned_and_keeps_guest_argv_literal() {
    let frame = HelperInput::start(
        "container-id".to_owned(),
        vec!["printf".to_owned(), "literal arg".to_owned()],
        true,
    );
    assert_eq!(
        serde_json::to_value(frame).unwrap(),
        json!({
            "version": 1,
            "type": "start",
            "container": "container-id",
            "argv": ["printf", "literal arg"],
            "tty": true
        })
    );
    assert_eq!(HELPER_PROTOCOL_VERSION, 1);
}

#[test]
fn binary_frames_use_base64_and_round_trip() {
    let input = HelperInput::from(AttachInput::Stdin(vec![0, 255]));
    assert_eq!(
        serde_json::to_value(&input).unwrap(),
        json!({"version": 1, "type": "stdin", "data": "AP8="})
    );
    assert_eq!(
        serde_json::from_value::<HelperInput>(json!({
            "version": 1, "type": "stdin", "data": "AP8="
        }))
        .unwrap(),
        input
    );

    let output = HelperOutput::stdout(vec![254, 1]);
    assert_eq!(
        serde_json::to_value(&output).unwrap(),
        json!({"version": 1, "type": "stdout", "data": "/gE="})
    );
    assert_eq!(
        output.into_attach_output().unwrap(),
        AttachOutput::Stdout(vec![254, 1])
    );
}

#[test]
fn control_terminal_and_typed_error_frames_are_stable() {
    let cases = [
        (
            HelperInput::from(AttachInput::Resize {
                rows: 41,
                cols: 113,
            }),
            json!({"version": 1, "type": "resize", "rows": 41, "cols": 113}),
        ),
        (
            HelperInput::from(AttachInput::Signal(15)),
            json!({"version": 1, "type": "signal", "signal": 15}),
        ),
        (
            HelperInput::from(AttachInput::Close),
            json!({"version": 1, "type": "close"}),
        ),
    ];
    for (frame, expected) in cases {
        assert_eq!(serde_json::to_value(frame).unwrap(), expected);
    }

    let typed = HelperOutput::error("invalid_signal", "only SIGINT and SIGTERM are allowed");
    assert_eq!(
        serde_json::to_value(typed).unwrap(),
        json!({
            "version": 1,
            "type": "error",
            "code": "invalid_signal",
            "message": "only SIGINT and SIGTERM are allowed"
        })
    );
    assert_eq!(
        serde_json::to_value(HelperOutput::exit(42)).unwrap(),
        json!({"version": 1, "type": "exit", "code": 42})
    );
}

#[test]
fn invalid_base64_and_protocol_versions_are_rejected() {
    assert!(
        serde_json::from_value::<HelperOutput>(json!({
            "version": 1, "type": "stdout", "data": "not base64!"
        }))
        .is_err()
    );
    let future: HelperOutput = serde_json::from_value(json!({
        "version": 2, "type": "exit", "code": 0
    }))
    .unwrap();
    assert!(future.into_attach_output().is_err());
}
