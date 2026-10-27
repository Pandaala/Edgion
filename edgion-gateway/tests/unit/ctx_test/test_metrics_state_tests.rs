use super::*;
use crate::observe::metrics::TestType;

fn context(mode: Option<TestType>) -> EdgionHttpContext {
    let mut gateway = GatewayInfo::default();
    gateway.metrics_test_type = mode;
    let mut ctx = EdgionHttpContext::new();
    ctx.gateway_info = Some(Arc::new(gateway));
    ctx
}

#[test]
fn test_metrics_absent_reads_and_inactive_modes_do_not_allocate() {
    for mode in [None, Some(TestType::None), Some(TestType::Retry), Some(TestType::Lb)] {
        let mut ctx = context(mode);
        let (_, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
            assert!(ctx.test_hash_key().is_none());
            assert!(ctx.test_upstream_start_time().is_none());
            ctx.record_test_upstream_start();
            ctx.record_test_hash_key(None);
        });
        assert!(ctx.test_metrics.is_none());
        assert_eq!(stats.calls, 0);
        assert_eq!(stats.requested_bytes, 0);
    }
}

#[test]
fn test_metrics_latency_allocates_once_and_survives_backend_replacement() {
    let mut ctx = context(Some(TestType::Latency));
    let (_, first) = crate::test_allocation_counter::count_allocation_stats(|| {
        ctx.record_test_upstream_start();
    });
    assert_eq!(first.calls, 1);
    let state = ctx.test_metrics.as_deref_mut().unwrap();
    // A distinguishable timestamp proves retries retain the original capture.
    let start = Instant::now() - Duration::from_secs(1);
    state.upstream_start_time = Some(start);
    let owner = state as *const RequestTestMetricsState;
    ctx.init_backend_context(
        Rsc::new("Service", "test", "replacement"),
        "test/replacement".into(),
        ServiceBackendKind::Service,
        None,
    );
    let (_, later) = crate::test_allocation_counter::count_allocation_stats(|| {
        ctx.record_test_upstream_start();
        assert_eq!(ctx.test_upstream_start_time(), Some(start));
        assert!(ctx.test_hash_key().is_none());
    });
    assert_eq!(later.calls, 0);
    assert_eq!(
        ctx.test_metrics.as_deref().unwrap() as *const RequestTestMetricsState,
        owner
    );
}

#[test]
fn test_metrics_hash_capture_is_lb_only_and_clear_does_not_allocate() {
    for mode in [
        None,
        Some(TestType::None),
        Some(TestType::Retry),
        Some(TestType::Latency),
        Some(TestType::Lb),
    ] {
        let lb = matches!(mode, Some(TestType::Lb));
        let mut ctx = context(mode);
        let key = "route-key".to_owned();
        let (_, first) = crate::test_allocation_counter::count_allocation_stats(|| {
            ctx.record_test_hash_key(Some(key));
        });
        assert_eq!(first.calls, usize::from(lb));
        assert_eq!(ctx.test_hash_key(), lb.then_some("route-key"));
        assert!(ctx.test_upstream_start_time().is_none());
        let (_, clear) = crate::test_allocation_counter::count_allocation_stats(|| {
            ctx.record_test_hash_key(None);
            assert!(ctx.test_hash_key().is_none());
        });
        assert_eq!(clear.calls, 0);
        assert_eq!(ctx.test_metrics.is_some(), lb);
    }
}

#[test]
fn ordinary_constructor_keeps_test_metrics_absent_without_allocating() {
    let config = crate::config::default_gateway_config();
    let (ctx, stats) =
        crate::test_allocation_counter::count_allocation_stats(|| EdgionHttpContext::with_gateway_config(config));
    assert!(ctx.test_metrics.is_none());
    assert_eq!(stats.calls, 0);
    assert_eq!(stats.requested_bytes, 0);
}
