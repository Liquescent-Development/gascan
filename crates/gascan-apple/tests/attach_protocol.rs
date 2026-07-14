use gascan_apple::{AttachInput, AttachOutput};
use serde_json::json;

#[test]
fn attach_input_has_a_stable_json_protocol() {
    let cases = [
        (AttachInput::Stdin(vec![0, 255]), json!({"stdin": [0, 255]})),
        (
            AttachInput::Resize {
                rows: 41,
                cols: 113,
            },
            json!({"resize": {"rows": 41, "cols": 113}}),
        ),
        (AttachInput::Signal(15), json!({"signal": 15})),
        (AttachInput::Close, json!("close")),
    ];

    for (message, expected) in cases {
        assert_eq!(serde_json::to_value(&message).unwrap(), expected);
        assert_eq!(
            serde_json::from_value::<AttachInput>(expected).unwrap(),
            message
        );
    }
}

#[test]
fn attach_output_preserves_binary_streams_and_exact_exit_codes() {
    let cases = [
        (
            AttachOutput::Stdout(vec![0, 255]),
            json!({"stdout": [0, 255]}),
        ),
        (
            AttachOutput::Stderr(vec![254, 1]),
            json!({"stderr": [254, 1]}),
        ),
        (AttachOutput::Exit(42), json!({"exit": 42})),
        (AttachOutput::Exit(127), json!({"exit": 127})),
    ];

    for (message, expected) in cases {
        assert_eq!(serde_json::to_value(&message).unwrap(), expected);
        assert_eq!(
            serde_json::from_value::<AttachOutput>(expected).unwrap(),
            message
        );
    }
}
