use gascand::{BackendSelection, backend_selection};

/// **`Fake` must not be reachable in a release build, and `Arca` must be.**
///
/// The two halves are the same assertion pointed in opposite directions.
/// `Fake` fabricates its answers, so a release binary that could be talked into
/// it would report sandboxes that do not exist. `Arca` is a real engine and a
/// shipped configuration, so a `#[cfg(debug_assertions)]` on it -- the obvious
/// thing to copy from the variant beside it -- would make the whole backend
/// vanish from the product while every debug test kept passing.
#[test]
fn production_selection_is_apple_arca_is_a_release_backend_and_fake_is_test_only() {
    assert_eq!(backend_selection(false, false), Ok(BackendSelection::Apple));
    assert_eq!(backend_selection(false, true), Ok(BackendSelection::Arca));

    #[cfg(debug_assertions)]
    assert_eq!(backend_selection(true, false), Ok(BackendSelection::Fake));
    #[cfg(not(debug_assertions))]
    assert_eq!(
        backend_selection(true, false),
        Ok(BackendSelection::Apple),
        "a release build must not resolve to the fabricating runtime"
    );
}

/// **Two backends requested at once is refused, not resolved by precedence.**
///
/// This is the one ambiguity the instance-record mismatch check downstream
/// cannot catch. Any precedence rule would hand the user a daemon on one
/// backend while they believed they had asked for the other -- and the daemon
/// would record the backend it actually built, so the record and the daemon
/// would agree perfectly and the client would connect happily.
#[test]
fn requesting_two_backends_is_refused_rather_than_resolved_by_precedence() {
    let refusal = backend_selection(true, true).expect_err("both requested must not resolve");
    let message = refusal.to_string();
    assert!(
        message.contains("GASCAN_ARCA_BACKEND"),
        "the refusal must name the variables to unset: {message}"
    );
    #[cfg(debug_assertions)]
    assert!(
        message.contains("GASCAN_TEST_FAKE_BACKEND"),
        "the refusal must name the variables to unset: {message}"
    );
}

#[test]
fn backend_environment_names_are_stable() {
    #[cfg(debug_assertions)]
    assert_eq!(gascand::TEST_FAKE_BACKEND_ENV, "GASCAN_TEST_FAKE_BACKEND");
    assert_eq!(gascand::ARCA_BACKEND_ENV, "GASCAN_ARCA_BACKEND");
    assert_eq!(gascand::ENGINE_SOCKET_ENV, "GASCAN_ENGINE_SOCKET");
    assert_eq!(gascand::ENGINE_BIN_ENV, "GASCAN_ENGINE_BIN");
    assert_eq!(
        gascand::ENGINE_STATE_ROOT_ENV,
        "GASCAN_ENGINE_STATE_ROOT",
        "the state root is part of the daemon's documented environment; renaming it \
         silently points a spawned engine at a store nothing else reads"
    );
}
