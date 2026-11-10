use super::*;

fn secs(value: u64) -> Duration {
    Duration::from_secs(value)
}

#[test]
fn defaults_match_the_schema() {
    let policy = DnsRefreshPolicy::default();
    assert!(policy.endpoint_table);
    assert_eq!(policy.ttl_factor, 0.5);
    assert_eq!(policy.min_refresh, secs(5));
    assert_eq!(policy.max_refresh, secs(300));
    assert_eq!(policy.fallback_refresh, secs(5));
    assert_eq!(policy.refresh_interval, None);
    assert_eq!(policy.max_stale, secs(300));
    assert_eq!(policy.max_entries, 4096);
}

#[test]
fn ttl_scaled_by_factor_and_clamped() {
    let policy = DnsRefreshPolicy::default();
    assert_eq!(policy.success_interval(Some(secs(60))), secs(30));
    assert_eq!(policy.success_interval(Some(secs(4))), secs(5), "clamped to min");
    assert_eq!(policy.success_interval(Some(Duration::ZERO)), secs(5));
    assert_eq!(policy.success_interval(Some(secs(86_400))), secs(300), "clamped to max");
    assert_eq!(policy.success_interval(None), secs(5), "TTL-less uses fallback");
}

#[test]
fn huge_reported_ttls_never_panic() {
    let policy = DnsRefreshPolicy::from_config(&DnsResolverConfig {
        ttl_factor: 1.0,
        max_refresh_seconds: 2_147_483_647,
        ..Default::default()
    });
    for ttl in [secs(u64::MAX), secs(u64::MAX - 1024), Duration::MAX] {
        assert_eq!(policy.success_interval(Some(ttl)), secs(2_147_483_647));
        assert!(policy.success_delay(Some(ttl), 1.0) <= secs(2_147_483_647));
    }
}

#[test]
fn configured_bounds_reach_the_largest_dns_ttl() {
    let config = DnsResolverConfig {
        min_refresh_seconds: 2,
        max_refresh_seconds: 2_147_483_647,
        ttl_factor: 1.0,
        ..Default::default()
    };
    let policy = DnsRefreshPolicy::from_config(&config);
    assert_eq!(policy.success_interval(Some(secs(1))), secs(2));
    assert_eq!(policy.success_interval(Some(secs(2_147_483_647))), secs(2_147_483_647));
}

#[test]
fn unset_fallback_is_clamped_into_the_configured_range() {
    let raised = DnsRefreshPolicy::from_config(&DnsResolverConfig {
        min_refresh_seconds: 10,
        ..Default::default()
    });
    assert_eq!(raised.fallback_refresh, secs(10));
    let lowered = DnsRefreshPolicy::from_config(&DnsResolverConfig {
        min_refresh_seconds: 2,
        max_refresh_seconds: 3,
        ..Default::default()
    });
    assert_eq!(lowered.fallback_refresh, secs(3));
}

#[test]
fn fixed_interval_ignores_ttl_and_the_min_bound() {
    let config = DnsResolverConfig {
        refresh_interval_seconds: Some(2),
        ..Default::default()
    };
    let policy = DnsRefreshPolicy::from_config(&config);
    assert_eq!(policy.success_interval(Some(secs(3600))), secs(2));
    assert_eq!(policy.success_interval(None), secs(2));
    assert_eq!(
        policy.success_delay(None, 0.0),
        secs(2),
        "below minRefresh (5) but never below the 2 s floor"
    );
    assert_eq!(policy.success_delay(None, 1.0), Duration::from_millis(2200));
}

#[test]
fn unvalidated_values_are_clamped_to_safe_ranges() {
    let config = DnsResolverConfig {
        min_refresh_seconds: 0,
        max_refresh_seconds: 0,
        fallback_refresh_seconds: Some(0),
        refresh_interval_seconds: Some(0),
        ttl_factor: f64::NAN,
        max_entries: 0,
        ..Default::default()
    };
    let policy = DnsRefreshPolicy::from_config(&config);
    assert_eq!(policy.min_refresh, secs(2));
    assert_eq!(policy.max_refresh, secs(2));
    assert_eq!(policy.fallback_refresh, secs(2));
    assert_eq!(policy.refresh_interval, Some(secs(2)));
    assert_eq!(policy.ttl_factor, 0.5);
    assert_eq!(policy.max_entries, 1);
    let tiny = DnsRefreshPolicy::from_config(&DnsResolverConfig {
        ttl_factor: 0.0,
        ..Default::default()
    });
    assert_eq!(tiny.ttl_factor, 0.01);
}

#[test]
fn failure_backoff_doubles_up_to_sixty_seconds() {
    let policy = DnsRefreshPolicy::default();
    assert_eq!(policy.failure_backoff(1), secs(5));
    assert_eq!(policy.failure_backoff(2), secs(10));
    assert_eq!(policy.failure_backoff(3), secs(20));
    assert_eq!(policy.failure_backoff(4), secs(40));
    assert_eq!(policy.failure_backoff(5), secs(60));
    assert_eq!(policy.failure_backoff(u32::MAX), secs(60));
    assert_eq!(policy.failure_delay(10, 1.0), secs(60), "jitter cannot exceed the cap");
    assert_eq!(policy.failure_delay(1, 0.0), secs(5), "jitter cannot undercut min");
}

#[test]
fn failure_backoff_respects_a_small_max_refresh() {
    let config = DnsResolverConfig {
        min_refresh_seconds: 2,
        max_refresh_seconds: 8,
        ..Default::default()
    };
    let policy = DnsRefreshPolicy::from_config(&config);
    assert_eq!(policy.failure_backoff(1), secs(2));
    assert_eq!(policy.failure_backoff(10), secs(8));
}

#[test]
fn jitter_spreads_ten_percent_within_bounds() {
    let policy = DnsRefreshPolicy::default();
    assert_eq!(policy.success_delay(Some(secs(200)), 0.0), secs(90));
    assert_eq!(policy.success_delay(Some(secs(200)), 0.5), secs(100));
    assert_eq!(policy.success_delay(Some(secs(200)), 1.0), secs(110));
    assert_eq!(policy.success_delay(None, 0.0), secs(5), "never below min");
    assert_eq!(
        policy.success_delay(Some(secs(86_400)), 1.0),
        secs(300),
        "never above max"
    );
    assert_eq!(policy.success_delay(Some(secs(200)), f64::NAN), secs(100));
}
