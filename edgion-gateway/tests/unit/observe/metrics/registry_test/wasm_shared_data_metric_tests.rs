#[test]
fn empty_shared_data_metric_registry_reports_zero() {
    let recorder = super::tests::lifetime_recorder();
    let stats = recorder.stats();
    assert_eq!(stats.gauge_owners, 0);
    assert_eq!(stats.attached_gauge_owners, 0);
    assert_eq!(stats.live_epochs, 0);
    assert!(stats.keys.is_empty());
}
