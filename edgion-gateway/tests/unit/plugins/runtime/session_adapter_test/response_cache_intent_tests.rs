use super::*;
use crate::plugins::http::response_cache::ResponseCachePolicy;
use crate::plugins::runtime::traits::session::ResponseCacheIntentAdmission as Admission;
use crate::response_cache::tests::Harness;
use edgion_resources::ResponseCacheConfig;

#[test]
fn response_cache_real_adapter_admits_once_and_conflicts_stay_bypassed() {
    let mut harness = Harness::new(1024);
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.in_request_body_stage = true;
    harness.capture_publication(&mut ctx);
    let baseline = harness.totals().0;
    let (_, stream) = tokio::io::duplex(4096);
    let mut session = Session::new_h1(Box::new(stream));
    let policy = ResponseCachePolicy::compile(&ResponseCacheConfig::default()).unwrap();
    let (outcome, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        PingoraSessionAdapter::new(&mut session, &mut ctx).nominate_response_cache_in(&policy, harness.local_store())
    });
    assert_eq!(outcome, Admission::Admitted);
    let control = std::mem::size_of::<crate::response_cache::RequestState>() + 16;
    assert!(stats.requested_bytes <= control);
    assert_eq!(harness.totals().0, baseline + control);
    let equal = ResponseCachePolicy::compile(&ResponseCacheConfig::default()).unwrap();
    let (outcome, calls) = crate::test_allocation_counter::count_allocations(|| {
        PingoraSessionAdapter::new(&mut session, &mut ctx).nominate_response_cache_in(&equal, harness.local_store())
    });
    assert_eq!(outcome, Admission::AlreadyAdmitted);
    assert_eq!(calls, 0);
    assert_eq!(harness.totals().0, baseline + control);
    let conflict = ResponseCachePolicy::compile(&ResponseCacheConfig {
        graphql_queries: true,
        ..Default::default()
    })
    .unwrap();
    let (outcome, calls) = crate::test_allocation_counter::count_allocations(|| {
        PingoraSessionAdapter::new(&mut session, &mut ctx).nominate_response_cache_in(&conflict, harness.local_store())
    });
    assert_eq!(outcome, Admission::Bypassed);
    assert_eq!(calls, 0);
    assert!(ctx.response_cache.is_none());
    assert!(ctx.response_cache_bypassed);
    assert_eq!(harness.totals().0, baseline);
    assert_eq!(
        PingoraSessionAdapter::new(&mut session, &mut ctx).nominate_response_cache_in(&policy, harness.local_store()),
        Admission::Bypassed
    );
    drop(ctx);
    assert_eq!(harness.totals().0, baseline);
    assert_eq!(
        harness.totals().0,
        harness.totals().1,
        "final context teardown returns to standing infrastructure"
    );
}

#[test]
fn response_cache_missing_publication_and_missing_process_store_allocate_no_intent() {
    let harness = Harness::new(1024);
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.in_request_body_stage = true;
    let (_, stream) = tokio::io::duplex(4096);
    let mut session = Session::new_h1(Box::new(stream));
    let policy = ResponseCachePolicy::compile(&ResponseCacheConfig::default()).unwrap();
    let (outcome, calls) = crate::test_allocation_counter::count_allocations(|| {
        PingoraSessionAdapter::new(&mut session, &mut ctx).nominate_response_cache_in(&policy, harness.local_store())
    });
    assert_eq!(outcome, Admission::Bypassed);
    assert_eq!(calls, 0);
    assert!(ctx.response_cache.is_none());
    assert!(
        crate::response_cache::process_store().is_none(),
        "unit tests never initialize the process store"
    );
    let (outcome, calls) = crate::test_allocation_counter::count_allocations(|| {
        PingoraSessionAdapter::new(&mut session, &mut ctx).nominate_response_cache(&policy)
    });
    assert_eq!(outcome, Admission::Bypassed);
    assert_eq!(calls, 0);
}
