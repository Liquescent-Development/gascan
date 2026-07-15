use gascan_core::doctor::DoctorFacts;
use gascand::DoctorState;
use std::time::Duration;

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
            && check.detail.contains("was abandoned")
    }));
}

#[tokio::test(start_paused = true)]
async fn producer_timeout_is_cached_for_late_and_concurrent_callers() {
    let expected = DoctorFacts::all_supported_for_tests().into_report();
    let state = DoctorState::collect(Duration::from_secs(60), {
        let expected = expected.clone();
        async move {
            tokio::time::sleep(Duration::from_secs(61)).await;
            expected
        }
    });
    let left = tokio::spawn({
        let state = state.clone();
        async move { state.report().await }
    });
    let right = tokio::spawn({
        let state = state.clone();
        async move { state.report().await }
    });
    tokio::time::advance(Duration::from_secs(60)).await;
    tokio::task::yield_now().await;
    let left = left.await.unwrap();
    let right = right.await.unwrap();
    assert_eq!(left.checks, right.checks);
    assert!(
        left.checks
            .iter()
            .all(|check| check.detail.contains("exceeded its 60 second bound"))
    );

    tokio::time::advance(Duration::from_secs(2)).await;
    let late = state.report().await;
    assert_eq!(late.checks, left.checks);
}
