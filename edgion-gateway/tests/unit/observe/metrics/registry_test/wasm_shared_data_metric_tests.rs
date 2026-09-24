use super::*;

#[test]
fn empty_shared_data_metric_registry_reports_zero() {
    let stats = shared_data_metric_stats(&HashMap::new());
    assert_eq!(stats.count, 0);
    assert!(stats.keys.is_empty());
}
