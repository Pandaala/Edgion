use super::*;

#[test]
fn scalar_admission_preserves_gateway_loopback_and_address_policy() {
    for allow_loopback in [false, true] {
        for block_private in [false, true] {
            let policy = EgressPolicy { block_private };
            for address in ["127.0.0.1", "::1", "::ffff:127.0.0.1", "93.184.216.34"] {
                assert!(is_ip_allowed(address.parse().unwrap(), policy, allow_loopback));
            }
            for address in ["169.254.169.254", "fe80::1", "::ffff:169.254.169.254", "0.0.0.0", "::"] {
                assert!(!is_ip_allowed(address.parse().unwrap(), policy, allow_loopback));
            }
            for address in ["10.1.2.3", "fd00::1", "::ffff:192.168.1.1"] {
                assert_eq!(
                    is_ip_allowed(address.parse().unwrap(), policy, allow_loopback),
                    !block_private
                );
            }
        }
    }
}

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
