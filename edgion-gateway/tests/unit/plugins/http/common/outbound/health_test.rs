use super::*;

fn passive(threshold: u32, max_percent: u32) -> PassiveHealthCheck {
    let mut passive: PassiveHealthCheck = serde_json::from_str("{}").unwrap();
    passive.unhealthy_threshold = threshold;
    passive.max_ejection_percent = max_percent;
    passive
}

fn active(healthy: u32, unhealthy: u32) -> ActiveHealthCheck {
    ActiveHealthCheck {
        path: Some("/healthz".to_string()),
        interval_sec: 10,
        timeout_ms: 2_000,
        healthy_threshold: healthy,
        unhealthy_threshold: unhealthy,
    }
}

fn addr(last: u8) -> SocketAddr {
    SocketAddr::from(([10, 0, 0, last], 8080))
}

fn pool(addrs: &[SocketAddr], passive: Option<&PassiveHealthCheck>) -> Arc<OutboundHealth> {
    OutboundHealth::with_endpoints_for_test(addrs, passive)
}

fn dest_for(health: &OutboundHealth, target: SocketAddr) -> Arc<DestHealth> {
    health
        .current()
        .dests
        .iter()
        .find(|dest| dest.addr == Some(target))
        .cloned()
        .unwrap()
}

/// Fail one specific endpoint until its passive threshold is reached.
fn fail_endpoint(health: &OutboundHealth, target: SocketAddr, times: u32) {
    let dest = dest_for(health, target);
    for _ in 0..times {
        let pick = admit(&dest).expect("endpoint must be admitted while failing it");
        health.record(pick, AttemptOutcome::Failure);
    }
}

#[test]
fn passive_only_static_ejection_is_time_bounded_with_one_probe() {
    let health = OutboundHealth::new(HealthTarget::Static("http://10.0.0.1:80".into()), Some(&passive(2, 50)));
    let pick = health.pick(&[]).unwrap();
    health.record(pick, AttemptOutcome::Failure);
    assert!(health.pick(&[]).is_ok(), "one failure stays below the threshold");
    let pick = health.pick(&[]).unwrap();
    health.record(pick, AttemptOutcome::Failure);
    assert_eq!(health.pick(&[]).err(), Some(PickError::Unhealthy));

    // Without backoff configuration the ejection still expires.
    health.pool.state.expire_deadlines_for_test();
    let probe = health.pick(&[]).unwrap();
    assert!(probe.probe.is_some(), "the expired window admits a half-open probe");
    assert_eq!(
        health.pick(&[]).err(),
        Some(PickError::Unhealthy),
        "only one probe is admitted per window"
    );
    health.record(probe, AttemptOutcome::Success);
    let pick = health.pick(&[]).unwrap();
    assert!(pick.probe.is_none());
    assert!(health.all_usable_for_test());
}

#[test]
fn failed_probe_reopens_and_dropped_probe_releases_lease() {
    let health = OutboundHealth::new(HealthTarget::Static("http://10.0.0.1:80".into()), Some(&passive(1, 50)));
    let pick = health.pick(&[]).unwrap();
    health.record(pick, AttemptOutcome::Failure);
    health.pool.state.expire_deadlines_for_test();

    let probe = health.pick(&[]).unwrap();
    drop(probe);
    let probe = health.pick(&[]).expect("a dropped probe releases its lease");
    assert!(probe.probe.is_some());
    health.record(probe, AttemptOutcome::Failure);
    assert_eq!(health.pick(&[]).err(), Some(PickError::Unhealthy));
    assert_eq!(health.pool.state.num_ejections(), 2);

    // An ignored outcome is no verdict: the lease is released, state unchanged.
    health.pool.state.expire_deadlines_for_test();
    let probe = health.pick(&[]).unwrap();
    health.record(probe, AttemptOutcome::Ignore);
    assert!(health.pick(&[]).unwrap().probe.is_some());
}

#[test]
fn success_resets_the_consecutive_counter() {
    let health = OutboundHealth::new(HealthTarget::Static("http://10.0.0.1:80".into()), Some(&passive(2, 50)));
    for outcome in [
        AttemptOutcome::Failure,
        AttemptOutcome::Success,
        AttemptOutcome::Failure,
    ] {
        let pick = health.pick(&[]).unwrap();
        health.record(pick, outcome);
    }
    assert!(health.all_usable_for_test());
}

#[test]
fn without_passive_config_failures_never_eject() {
    let health = OutboundHealth::new(HealthTarget::Static("http://10.0.0.1:80".into()), None);
    for _ in 0..10 {
        let pick = health.pick(&[]).unwrap();
        health.record(pick, AttemptOutcome::Failure);
    }
    assert!(health.all_usable_for_test());
    assert!(health.count_timeout());
    assert!(!health.is_failure_status(503));
}

#[test]
fn passive_policy_reports_count_timeout_and_failure_statuses() {
    let mut config = passive(3, 50);
    config.count_timeout = false;
    let health = OutboundHealth::new(HealthTarget::Static("http://10.0.0.1:80".into()), Some(&config));
    assert!(!health.count_timeout());
    assert!(health.is_failure_status(503));
    assert!(!health.is_failure_status(404));
}

#[test]
fn one_bad_endpoint_is_ejected_without_failing_the_pool() {
    let endpoints = [addr(1), addr(2), addr(3)];
    let health = pool(&endpoints, Some(&passive(2, 50)));
    fail_endpoint(&health, addr(1), 2);
    for _ in 0..9 {
        let pick = health.pick(&[]).unwrap();
        assert_ne!(pick.dest().addr(), Some(addr(1)), "the ejected endpoint is skipped");
        assert!(pick.dest().url().starts_with("http://10.0.0."));
    }
}

#[test]
fn max_ejection_percent_caps_ejected_endpoints() {
    let endpoints = [addr(1), addr(2), addr(3), addr(4)];
    let health = pool(&endpoints, Some(&passive(1, 50)));
    fail_endpoint(&health, addr(1), 1);
    fail_endpoint(&health, addr(2), 1);
    fail_endpoint(&health, addr(3), 1);
    let open = health
        .current()
        .dests
        .iter()
        .filter(|dest| dest.state.is_open())
        .count();
    assert_eq!(open, 2, "50% of four endpoints may be ejected");
    assert!(health.pick(&[]).is_ok());

    // A zero percent disables the cap.
    let health = pool(&endpoints, Some(&passive(1, 0)));
    for endpoint in endpoints {
        fail_endpoint(&health, endpoint, 1);
    }
    assert_eq!(health.pick(&[]).err(), Some(PickError::Unhealthy));
}

#[test]
fn retry_pick_avoids_tried_endpoints_until_none_remain() {
    let endpoints = [addr(1), addr(2)];
    let health = pool(&endpoints, None);
    for _ in 0..4 {
        assert_eq!(health.pick(&[addr(1)]).unwrap().dest().addr(), Some(addr(2)));
    }
    let both = [addr(1), addr(2)];
    assert!(
        health.pick(&both).is_ok(),
        "a retry reuses a tried endpoint when nothing else remains"
    );

    // An ejected untried endpoint is not a substitute.
    let health = pool(&endpoints, Some(&passive(1, 0)));
    fail_endpoint(&health, addr(2), 1);
    assert_eq!(health.pick(&[addr(1)]).unwrap().dest().addr(), Some(addr(1)));
}

#[test]
fn active_verdicts_are_per_endpoint_and_lift_passive_ejection() {
    let endpoints = [addr(1), addr(2)];
    let health = pool(&endpoints, Some(&passive(1, 0)));
    let probe = active(2, 2);
    let first = dest_for(&health, addr(1));

    health.record_active(&first, &probe, false);
    assert!(health.pick(&[addr(2)]).unwrap().dest().addr() == Some(addr(1)));
    health.record_active(&first, &probe, false);
    for _ in 0..4 {
        assert_eq!(health.pick(&[]).unwrap().dest().addr(), Some(addr(2)));
    }

    // Recovery needs the healthy threshold; it also clears a passive ejection.
    fail_endpoint(&health, addr(2), 1);
    let second = dest_for(&health, addr(2));
    health.record_active(&first, &probe, true);
    assert_eq!(health.pick(&[]).err(), Some(PickError::Unhealthy));
    health.record_active(&first, &probe, true);
    health.record_active(&second, &probe, true);
    health.record_active(&second, &probe, true);
    assert!(health.all_usable_for_test());
}

#[test]
fn unsafe_endpoints_are_skipped_and_reported_as_no_endpoint() {
    let loopback = SocketAddr::from(([127, 0, 0, 1], 8080));
    let health = pool(&[loopback], None);
    assert_eq!(health.pick(&[]).err(), Some(PickError::NoEndpoint));
    assert!(health.probe_targets().is_empty());

    let health = pool(&[loopback, addr(9)], None);
    for _ in 0..3 {
        assert_eq!(health.pick(&[]).unwrap().dest().addr(), Some(addr(9)));
    }
    assert_eq!(pool(&[], None).pick(&[]).err(), Some(PickError::NoEndpoint));
}

#[test]
fn churn_carries_state_of_remaining_endpoints_and_prunes_departed_ones() {
    let config = passive(1, 0).resolve();
    let previous = carry_over(&[], vec![addr(1), addr(2)], "https", Some(&config));
    let next = carry_over(&previous, vec![addr(2), addr(3)], "https", Some(&config));
    assert_eq!(next.len(), 2);
    assert!(
        Arc::ptr_eq(&next[0], &previous[1]),
        "a remaining endpoint keeps its state"
    );
    assert_eq!(next[1].addr(), Some(addr(3)));
    assert_eq!(next[1].url(), "https://10.0.0.3:8080");
    assert!(next.iter().all(|dest| dest.addr() != Some(addr(1))));
}

#[test]
fn authority_calls_share_the_bounded_pool_destination() {
    let health = pool(&[addr(1)], Some(&passive(1, 50)));
    let pick = health.pick_pool().unwrap();
    health.record(pick, AttemptOutcome::Failure);
    assert_eq!(health.pick_pool().err(), Some(PickError::Unhealthy));
    assert!(health.pick(&[]).is_ok(), "pool state does not eject endpoints");
    health.pool.state.expire_deadlines_for_test();
    let probe = health.pick_pool().unwrap();
    assert!(probe.probe.is_some());
    health.record(probe, AttemptOutcome::Success);
    assert!(health.pick_pool().is_ok());
}
