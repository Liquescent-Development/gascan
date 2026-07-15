#[test]
fn production_selection_is_apple_and_fake_is_explicitly_test_only() {
    assert_eq!(
        gascand::backend_selection(false),
        gascand::BackendSelection::Apple
    );
    #[cfg(debug_assertions)]
    assert_eq!(
        gascand::backend_selection(true),
        gascand::BackendSelection::Fake
    );
    #[cfg(not(debug_assertions))]
    assert_eq!(
        gascand::backend_selection(true),
        gascand::BackendSelection::Apple
    );
}

#[test]
fn fake_backend_environment_name_is_stable_and_test_scoped() {
    #[cfg(debug_assertions)]
    assert_eq!(gascand::TEST_FAKE_BACKEND_ENV, "GASCAN_TEST_FAKE_BACKEND");
}
