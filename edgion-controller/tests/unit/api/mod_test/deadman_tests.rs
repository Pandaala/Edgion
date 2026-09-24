use super::*;

// NOTE: LIFECYCLE_TERMINATED is process-global and sticky by design; these
// tests reset it and must not run concurrently with each other. Keeping
// them in one #[tokio::test] serializes them.
#[tokio::test]
async fn health_check_reflects_lifecycle_flag() {
    reset_lifecycle_terminated_for_test();
    let resp = health_check().await;
    assert_eq!(resp.status(), StatusCode::OK);

    mark_lifecycle_terminated();
    let resp = health_check().await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

    reset_lifecycle_terminated_for_test();
    let resp = health_check().await;
    assert_eq!(resp.status(), StatusCode::OK);
}
