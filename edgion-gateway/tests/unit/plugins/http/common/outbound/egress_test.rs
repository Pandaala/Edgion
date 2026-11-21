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
fn scalar_admission_rejects_operator_blocked_cidr_under_every_policy() {
    use crate::test_blocked_cidr::{install, BLOCKED_CIDR_TEST_IP};
    install();
    let blocked: IpAddr = BLOCKED_CIDR_TEST_IP.parse().unwrap();
    for allow_loopback in [false, true] {
        for block_private in [false, true] {
            let policy = EgressPolicy { block_private };
            assert!(!is_ip_allowed(blocked, policy, allow_loopback));
            assert_eq!(
                filter_resolved_addrs(vec![blocked, "93.184.216.34".parse().unwrap()], policy, allow_loopback),
                vec!["93.184.216.34".parse::<IpAddr>().unwrap()]
            );
        }
    }
}

#[test]
fn configure_egress_blocked_cidrs_rejects_invalid_lists_and_skips_empty() {
    // Neither call installs anything: the process-wide list is install-once and
    // owned by `crate::test_blocked_cidr` in this test process.
    assert!(configure_egress_blocked_cidrs(&[]).is_ok());
    for invalid in ["100.100.100.200", "100.100.100.200/33", "not-an-ip/8"] {
        let error = configure_egress_blocked_cidrs(&[invalid.to_string()]).unwrap_err();
        assert!(error.contains("invalid blocked CIDR"), "{invalid}: {error}");
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

#[tokio::test]
async fn relaxed_request_resolver_filters_configured_and_non_dialable_answers() {
    use crate::test_blocked_cidr::{install, BLOCKED_CIDR_TEST_IP};
    use edgion_common::dns::{DnsAnswer, DnsCachePolicy, DnsQueryPolicy};
    install();
    let host = "dsl-cidr-policy.invalid";
    let ttl = Duration::from_secs(60);
    let addrs = [BLOCKED_CIDR_TEST_IP, "224.0.0.1", "10.0.0.1", "127.0.0.1"].map(|ip| ip.parse().unwrap());
    request_egress_answer_cache()
        .lookup_with(
            &ResolverSpec::System,
            host,
            DnsQueryPolicy::Native,
            &DnsCachePolicy::new(ttl),
            |_, _, _| std::future::ready(Ok(DnsAnswer::without_ttl(addrs.to_vec()))),
        )
        .await
        .unwrap();
    let resolver =
        EgressResolver::new_request_influenced(ResolverSpec::System, ResolverAddressPolicy::AllowPrivateAndLocal, ttl);
    assert_eq!(
        resolver.resolve_socket_addrs(host, 80).await.unwrap(),
        [
            "10.0.0.1:80".parse::<SocketAddr>().unwrap(),
            "127.0.0.1:80".parse().unwrap()
        ]
    );
}
