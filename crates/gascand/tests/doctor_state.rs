use gascan_core::doctor::DoctorFacts;
use gascand::DoctorState;

#[tokio::test]
async fn pending_doctor_callers_converge_on_one_completed_report() {
    let (state, completer) = DoctorState::pending();
    let left = tokio::spawn({
        let state = state.clone();
        async move { state.report().await }
    });
    let right = tokio::spawn({
        let state = state.clone();
        async move { state.report().await }
    });
    tokio::task::yield_now().await;
    assert!(!left.is_finished());
    assert!(!right.is_finished());
    let expected = DoctorFacts::all_supported_for_tests().into_report();
    completer.complete(expected.clone());
    assert_eq!(left.await.unwrap().checks, expected.checks);
    assert_eq!(right.await.unwrap().checks, expected.checks);
}

#[tokio::test]
async fn abandoned_doctor_collection_fails_closed() {
    let (state, completer) = DoctorState::pending();
    drop(completer);
    let report = state.report().await;
    assert!(report.checks.iter().all(|check| {
        check.status != gascan_core::doctor::DoctorStatus::Pass
            && check.detail.contains("failed or exceeded")
    }));
}
