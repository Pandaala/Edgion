use super::*;

#[test]
fn duration_millis_conversion_preserves_values_and_saturates() {
    assert_eq!(duration_millis_saturating(Duration::from_millis(30_000)), 30_000);
    assert_eq!(duration_millis_saturating(Duration::MAX), u64::MAX);
}

#[test]
fn configure_egress_defaults_updates_accessors() {
    configure_egress_defaults(true, Duration::from_millis(30_000));
    assert!(default_block_private());
    assert_eq!(egress_dns_cache_ttl(), Duration::from_millis(30_000));

    configure_egress_defaults(false, Duration::from_millis(5_000));
    assert!(!default_block_private());
    assert_eq!(egress_dns_cache_ttl(), Duration::from_millis(5_000));
}
