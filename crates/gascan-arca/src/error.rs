use gascan_core::runtime::RuntimeError;
use gascan_engine_proto::v1;

/// Maps an engine failure onto the variant that reports its code.
///
/// A table, not a judgment: the contract's own instruction is that a consumer
/// maps this with a table "so a new engine failure mode cannot quietly become an
/// existing one". Two codes are not an engine's to raise -- `injected_failure`
/// belongs to the fake runtime, and `unsupported_version` is the consumer's own
/// refusal to drive an engine and carries a version the wire never sends -- so
/// they are rejected alongside anything unrecognised.
///
/// Fields the wire cannot carry come from the RPC name, `None`, or the message.
/// An empty `resource` passes through: the contract says it is empty when the
/// failure is not about one, and failing the call over an empty diagnostic field
/// would replace a readable engine error with a confusing protocol one.
pub(crate) fn engine_error(operation: &str, error: &v1::EngineError) -> RuntimeError {
    let resource = error.resource.clone();
    let message = error.message.clone();
    match error.code.as_str() {
        "command_io" => RuntimeError::CommandIo {
            operation: operation.to_owned(),
            message,
        },
        "command_failed" => RuntimeError::CommandFailed {
            operation: operation.to_owned(),
            exit_code: None,
            stderr: message,
        },
        "invalid_output" => RuntimeError::InvalidOutput {
            operation: operation.to_owned(),
            message,
        },
        // The inner code is the wire code. Gas Can's own helper errors carry a
        // nested code, but `RuntimeError::code()` flattens it to "helper_error"
        // on the way out, so there is no field for the engine to have sent it in
        // and nothing to recover.
        "helper_error" => RuntimeError::HelperError {
            operation: operation.to_owned(),
            code: error.code.clone(),
            message,
        },
        "unsupported_capability" => RuntimeError::UnsupportedCapability {
            capability: message,
        },
        "ownership_mismatch" => RuntimeError::OwnershipMismatch { resource },
        "foreign_resource_refused" => RuntimeError::ForeignResourceRefused { resource },
        "invalid_resource_identity" => RuntimeError::InvalidResourceIdentity { name: resource },
        "resource_conflict" => RuntimeError::Conflict { resource, message },
        "not_found" => RuntimeError::NotFound { resource },
        "invalid_state" => RuntimeError::InvalidState { resource, message },
        "unknown_actual_state" => RuntimeError::UnknownActualState {
            resource,
            state: message,
        },
        unacceptable => RuntimeError::InvalidOutput {
            operation: operation.to_owned(),
            message: format!("engine returned unacceptable error code {unacceptable:?}: {message}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire(code: &str) -> v1::EngineError {
        v1::EngineError {
            code: code.to_owned(),
            resource: "code-a1b2c3d4e5f6".to_owned(),
            message: "the engine said so".to_owned(),
        }
    }

    #[test]
    fn every_accepted_code_round_trips_to_itself() {
        for code in [
            "command_io",
            "command_failed",
            "invalid_output",
            "helper_error",
            "unsupported_capability",
            "ownership_mismatch",
            "foreign_resource_refused",
            "invalid_resource_identity",
            "resource_conflict",
            "not_found",
            "invalid_state",
            "unknown_actual_state",
        ] {
            assert_eq!(
                engine_error("create", &wire(code)).code(),
                code,
                "an accepted code must map to the variant that reports it",
            );
        }
    }

    #[test]
    fn an_unknown_code_is_rejected_and_names_itself() {
        let error = engine_error("create", &wire("quantum_flux"));
        assert_eq!(error.code(), "invalid_output");
        let rendered = error.to_string();
        assert!(
            rendered.contains("quantum_flux"),
            "must name the code: {rendered}"
        );
    }

    #[test]
    fn a_code_no_engine_may_raise_is_rejected() {
        for code in ["injected_failure", "unsupported_version"] {
            let error = engine_error("create", &wire(code));
            assert_eq!(
                error.code(),
                "invalid_output",
                "{code} is not an engine's to raise",
            );
            assert!(error.to_string().contains(code), "must name {code}");
        }
    }

    #[test]
    fn an_empty_resource_passes_through_rather_than_failing_the_call() {
        let error = engine_error(
            "start",
            &v1::EngineError {
                code: "command_io".to_owned(),
                resource: String::new(),
                message: "socket closed".to_owned(),
            },
        );
        assert_eq!(error.code(), "command_io");
        assert!(error.to_string().contains("socket closed"));
    }

    #[test]
    fn the_fields_land_where_each_variant_expects_them() {
        // `code()` alone cannot catch a transposition. `unknown_actual_state` carries
        // the wire's message as its `state` and `invalid_resource_identity` carries
        // the wire's resource as its `name`, so swapping the two inputs would still
        // report the correct code and pass the round-trip test above. These assert
        // the rendered text, which is where a swap becomes visible.
        let state = engine_error("inspect", &wire("unknown_actual_state")).to_string();
        assert!(state.contains("code-a1b2c3d4e5f6"), "resource: {state}");
        assert!(
            state.contains("the engine said so"),
            "state carries the message: {state}"
        );

        let identity = engine_error("remove", &wire("invalid_resource_identity")).to_string();
        assert!(
            identity.contains("code-a1b2c3d4e5f6"),
            "the resource is the invalid name: {identity}",
        );

        let capability = engine_error("capabilities", &wire("unsupported_capability")).to_string();
        assert!(
            capability.contains("the engine said so"),
            "the message is the capability: {capability}",
        );

        // The RPC name is the operation for the variants the wire cannot supply one
        // for, and the message becomes the stderr rather than being dropped.
        let failed = engine_error("create", &wire("command_failed")).to_string();
        assert!(
            failed.contains("create"),
            "the rpc is the operation: {failed}"
        );
        assert!(
            failed.contains("the engine said so"),
            "the message is the stderr: {failed}"
        );
    }
}
