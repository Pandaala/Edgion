use super::*;
use edgion_common::dns::{DnsAnswer, DnsCachePolicy, DnsQueryPolicy, ResolverSpec, StandardDnsServers};
use std::future::Future;
use std::net::IpAddr;
use std::task::{Context, Poll, Waker};

// ── mode parsing ──────────────────────────────────────────────────────────

#[test]
fn mode_parse_defaults_to_controller() {
    assert_eq!(DnsResolutionMode::parse(None), DnsResolutionMode::Controller);
    assert_eq!(DnsResolutionMode::parse(Some("bogus")), DnsResolutionMode::Controller);
    assert_eq!(DnsResolutionMode::parse(Some("  gateway ")), DnsResolutionMode::Gateway);
    assert_eq!(
        DnsResolutionMode::parse(Some("per-request")),
        DnsResolutionMode::PerRequest
    );
}

// ── decision function ─────────────────────────────────────────────────────

#[test]
fn per_request_never_consults_slice() {
    // Even with a ready slice, per-request self-resolves without caching.
    assert_eq!(
        decide_external_name_path(DnsResolutionMode::PerRequest, true),
        ExternalNamePath::SelfResolve { cache: false }
    );
    assert_eq!(
        decide_external_name_path(DnsResolutionMode::PerRequest, false),
        ExternalNamePath::SelfResolve { cache: false }
    );
}

#[test]
fn gateway_never_consults_slice_and_caches() {
    assert_eq!(
        decide_external_name_path(DnsResolutionMode::Gateway, true),
        ExternalNamePath::SelfResolve { cache: true }
    );
    assert_eq!(
        decide_external_name_path(DnsResolutionMode::Gateway, false),
        ExternalNamePath::SelfResolve { cache: true }
    );
}

#[test]
fn controller_uses_slice_when_ready_else_bridges() {
    assert_eq!(
        decide_external_name_path(DnsResolutionMode::Controller, true),
        ExternalNamePath::EndpointSlice
    );
    assert_eq!(
        decide_external_name_path(DnsResolutionMode::Controller, false),
        ExternalNamePath::SelfResolve { cache: true }
    );
}

// ── upstream answer cache ─────────────────────────────────────────────────

#[test]
fn effective_shards_applies_the_floor_without_exceeding_capacity() {
    // `upstream_dns_cache()` is only reachable through the global LazyLock, so
    // no other test executes this arithmetic — and it is exactly the expression
    // whose failure mode is a `ShardedLruCache::new` panic at startup.
    for (configured, capacity, expected) in [
        // At or below the floor (including the production default of 0).
        (0, 1024, MIN_DNS_CACHE_SHARDS),
        (1, 1024, MIN_DNS_CACHE_SHARDS),
        (4, 1024, MIN_DNS_CACHE_SHARDS),
        // A larger configured count wins.
        (32, 1024, 32),
        (64, 65_536, 64),
        // A capacity at or below the floor clamps to the capacity.
        (0, 1, 1),
        (0, 2, 2),
        (0, 7, 7),
        (0, 8, MIN_DNS_CACHE_SHARDS),
    ] {
        let shards = effective_shards(configured, capacity);
        assert_eq!(
            shards, expected,
            "effective_shards({configured}, {capacity}) should be {expected}"
        );
        // The invariant ShardedLruCache::new asserts on.
        assert!(shards >= 1 && shards <= capacity, "shards must be in 1..=capacity");
        // Construct a cache at a capped capacity (so the test does not
        // allocate a 65k-node pool) with the clamp recomputed for that
        // capacity — a smoke check that the clamp yields a constructible
        // pair. The `shards <= capacity` assert above is what actually
        // guards the row under test.
        let capped = capacity.min(64);
        let _ = DnsAnswerCache::new(capped, effective_shards(configured, capped));
    }
}

#[test]
fn warm_hit_allocates_only_the_owned_address_result() {
    let cache = DnsAnswerCache::new(64, 8);
    let spec = ResolverSpec::StandardDns(StandardDnsServers::new(vec!["192.0.2.53".into()]));
    let policy = DnsCachePolicy::new(Duration::from_secs(60));
    let query = DnsQueryPolicy::Family(edgion_common::dns::DnsLookupFamily::All);
    let addresses: Vec<IpAddr> = vec!["192.0.2.4".parse().unwrap(), "2001:db8::4".parse().unwrap()];
    // Poll once: a warm hit completes without suspending.
    let poll_once = || {
        let lookup = cache.lookup_with(&spec, "a.test", query, &policy, |_, _, _| {
            std::future::ready(Ok(DnsAnswer {
                addrs: addresses.clone(),
                ttl: Some(Duration::from_secs(300)),
            }))
        });
        let mut lookup = std::pin::pin!(lookup);
        match lookup.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(result) => result,
            Poll::Pending => panic!("a cached lookup must not suspend"),
        }
    };
    assert_eq!(poll_once().unwrap(), addresses);
    let (_, result_allocations) = crate::test_allocation_counter::count_allocations(|| addresses.clone());
    let (hit, allocations) = crate::test_allocation_counter::count_allocations(poll_once);
    assert_eq!(hit.unwrap(), addresses);
    assert_eq!(
        allocations, result_allocations,
        "the resolver identity and cache key are borrowed on a warm hit"
    );
}
