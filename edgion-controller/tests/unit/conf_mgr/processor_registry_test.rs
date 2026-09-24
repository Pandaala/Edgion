use super::*;

#[test]
fn test_registry_basic() {
    let registry = ProcessorRegistry::new();
    assert_eq!(registry.count(), 0);
    // Empty registry is NOT ready - must have at least one processor
    assert!(!registry.is_all_ready());
    assert_eq!(registry.collect_stats_snapshot(), (HashMap::new(), 0));
}
