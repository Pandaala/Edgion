use super::*;

#[test]
fn family_is_part_of_borrowed_cache_identity_and_deletion_reclaims_all_families() {
    let cache = cache();
    let now = Instant::now();
    for (family, address) in [
        (DnsLookupFamily::Ipv4Only, "192.0.2.1"),
        (DnsLookupFamily::Ipv6Only, "2001:db8::1"),
        (DnsLookupFamily::All, "192.0.2.2"),
    ] {
        cache.insert("ns/svc", "example.test", "system", vec![ip(address)], SEC5, now, family);
    }
    for (family, address) in [
        (DnsLookupFamily::Ipv4Only, "192.0.2.1"),
        (DnsLookupFamily::Ipv6Only, "2001:db8::1"),
        (DnsLookupFamily::All, "192.0.2.2"),
    ] {
        assert_eq!(
            cache.get("ns/svc", "example.test", "system", now, family),
            Some(vec![ip(address)])
        );
    }
    cache.invalidate_services(&HashSet::from(["ns/svc".to_owned()]));
    for family in [
        DnsLookupFamily::Ipv4Only,
        DnsLookupFamily::Ipv6Only,
        DnsLookupFamily::All,
    ] {
        assert!(cache.get("ns/svc", "example.test", "system", now, family).is_none());
    }
}

/// A TTL long enough that no test hits it accidentally.
const SEC5: Duration = Duration::from_secs(5);
/// For entries that must stay live regardless of how long the surrounding
/// test run stalls: the entry-level check uses the caller's fake `now`, but
/// the LRU stamps its own deadline from the real clock.
const MIN1: Duration = Duration::from_secs(60);

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

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

// ── cache ───────────────────────────────────────────────────────────

/// Cache with a generous bound, so a test exercises identity and TTL rather
/// than capacity eviction.
fn cache() -> DnsCache {
    DnsCache::with_capacity(64, 4)
}

#[test]
fn effective_shards_applies_the_floor_without_exceeding_capacity() {
    // `DnsCache::new()` is only reachable through the global LazyLock, so no
    // other test executes this arithmetic — and it is exactly the expression
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
        let _ = DnsCache::with_capacity(capped, effective_shards(configured, capped));
    }
}

#[test]
fn cache_hit_within_ttl() {
    let cache = cache();
    let now = Instant::now();
    cache.insert(
        "ns/svc",
        "a.example.com",
        "fp1",
        vec![ip("1.2.3.4")],
        SEC5,
        now,
        DnsLookupFamily::All,
    );
    let got = cache.get(
        "ns/svc",
        "a.example.com",
        "fp1",
        now + Duration::from_secs(1),
        DnsLookupFamily::All,
    );
    assert_eq!(got, Some(vec![ip("1.2.3.4")]));
}

#[test]
fn cache_miss_after_expiry() {
    let cache = cache();
    let now = Instant::now();
    cache.insert(
        "ns/svc",
        "a.example.com",
        "fp1",
        vec![ip("1.2.3.4")],
        SEC5,
        now,
        DnsLookupFamily::All,
    );
    assert_eq!(
        cache.get(
            "ns/svc",
            "a.example.com",
            "fp1",
            now + Duration::from_secs(6),
            DnsLookupFamily::All
        ),
        None
    );
}

#[test]
fn cache_miss_exactly_at_the_deadline() {
    // The entry check is `expires_at > now`, so the deadline instant itself
    // is already a miss. Pins the comparison against a `>=` slip.
    let cache = cache();
    let now = Instant::now();
    cache.insert(
        "ns/svc",
        "a.example.com",
        "fp1",
        vec![ip("1.2.3.4")],
        SEC5,
        now,
        DnsLookupFamily::All,
    );
    assert_eq!(
        cache.get(
            "ns/svc",
            "a.example.com",
            "fp1",
            now + SEC5 - Duration::from_nanos(1),
            DnsLookupFamily::All
        ),
        Some(vec![ip("1.2.3.4")])
    );
    assert_eq!(
        cache.get("ns/svc", "a.example.com", "fp1", now + SEC5, DnsLookupFamily::All),
        None
    );
}

#[test]
fn cache_miss_on_fingerprint_change() {
    let cache = cache();
    let now = Instant::now();
    cache.insert(
        "ns/svc",
        "a.example.com",
        "fp1",
        vec![ip("1.2.3.4")],
        SEC5,
        now,
        DnsLookupFamily::All,
    );
    // A different resolver fingerprint must not hit the old entry.
    assert_eq!(
        cache.get("ns/svc", "a.example.com", "fp2", now, DnsLookupFamily::All),
        None
    );
}

#[test]
fn cache_miss_on_external_name_change() {
    // The core identity regression: one Service repointed from hostname A to
    // hostname B must never be served A's addresses, TTL notwithstanding.
    let cache = cache();
    let now = Instant::now();
    cache.insert(
        "ns/svc",
        "a.example.com",
        "fp1",
        vec![ip("1.2.3.4")],
        SEC5,
        now,
        DnsLookupFamily::All,
    );
    assert_eq!(
        cache.get("ns/svc", "b.example.com", "fp1", now, DnsLookupFamily::All),
        None
    );
    // The new hostname resolves and caches independently of the old one.
    cache.insert(
        "ns/svc",
        "b.example.com",
        "fp1",
        vec![ip("5.6.7.8")],
        SEC5,
        now,
        DnsLookupFamily::All,
    );
    assert_eq!(
        cache.get("ns/svc", "b.example.com", "fp1", now, DnsLookupFamily::All),
        Some(vec![ip("5.6.7.8")])
    );
}

#[test]
fn distinct_fingerprints_for_same_service_coexist() {
    // Two effective resolvers for one service (e.g. two Gateways) must NOT
    // evict each other — both entries live until their own TTL.
    let cache = cache();
    let now = Instant::now();
    cache.insert(
        "ns/svc",
        "a.example.com",
        "fp1",
        vec![ip("1.2.3.4")],
        SEC5,
        now,
        DnsLookupFamily::All,
    );
    cache.insert(
        "ns/svc",
        "a.example.com",
        "fp2",
        vec![ip("5.6.7.8")],
        SEC5,
        now,
        DnsLookupFamily::All,
    );
    assert_eq!(
        cache.get("ns/svc", "a.example.com", "fp1", now, DnsLookupFamily::All),
        Some(vec![ip("1.2.3.4")])
    );
    assert_eq!(
        cache.get("ns/svc", "a.example.com", "fp2", now, DnsLookupFamily::All),
        Some(vec![ip("5.6.7.8")])
    );
}

#[test]
fn reinsert_same_key_refreshes_entry() {
    let cache = cache();
    let now = Instant::now();
    cache.insert(
        "ns/svc",
        "a.example.com",
        "fp1",
        vec![ip("1.2.3.4")],
        SEC5,
        now,
        DnsLookupFamily::All,
    );
    // Re-resolve under the same identity overwrites with the new address set.
    cache.insert(
        "ns/svc",
        "a.example.com",
        "fp1",
        vec![ip("9.9.9.9")],
        SEC5,
        now,
        DnsLookupFamily::All,
    );
    assert_eq!(
        cache.get("ns/svc", "a.example.com", "fp1", now, DnsLookupFamily::All),
        Some(vec![ip("9.9.9.9")])
    );
    assert_eq!(cache.len(), 1);
}

// ── lifecycle ────────────────────────────────────────────────────

#[test]
fn invalidate_services_drops_only_the_named_services() {
    let cache = cache();
    let now = Instant::now();
    // Two hostnames and two resolvers under the doomed Service, plus a
    // bystander Service that must survive.
    cache.insert(
        "ns/gone",
        "a.example.com",
        "fp1",
        vec![ip("1.1.1.1")],
        SEC5,
        now,
        DnsLookupFamily::All,
    );
    cache.insert(
        "ns/gone",
        "b.example.com",
        "fp1",
        vec![ip("2.2.2.2")],
        SEC5,
        now,
        DnsLookupFamily::All,
    );
    cache.insert(
        "ns/gone",
        "a.example.com",
        "fp2",
        vec![ip("3.3.3.3")],
        SEC5,
        now,
        DnsLookupFamily::All,
    );
    cache.insert(
        "ns/kept",
        "c.example.com",
        "fp1",
        vec![ip("4.4.4.4")],
        SEC5,
        now,
        DnsLookupFamily::All,
    );
    assert_eq!(cache.len(), 4);

    cache.invalidate_services(&HashSet::from(["ns/gone".to_string()]));

    assert_eq!(cache.len(), 1);
    assert_eq!(
        cache.get("ns/gone", "a.example.com", "fp1", now, DnsLookupFamily::All),
        None
    );
    assert_eq!(
        cache.get("ns/gone", "b.example.com", "fp1", now, DnsLookupFamily::All),
        None
    );
    assert_eq!(
        cache.get("ns/gone", "a.example.com", "fp2", now, DnsLookupFamily::All),
        None
    );
    assert_eq!(
        cache.get("ns/kept", "c.example.com", "fp1", now, DnsLookupFamily::All),
        Some(vec![ip("4.4.4.4")])
    );
}

#[test]
fn invalidate_services_is_a_noop_for_an_empty_set() {
    let cache = cache();
    let now = Instant::now();
    cache.insert(
        "ns/svc",
        "a.example.com",
        "fp1",
        vec![ip("1.2.3.4")],
        SEC5,
        now,
        DnsLookupFamily::All,
    );
    cache.invalidate_services(&HashSet::new());
    assert_eq!(
        cache.get("ns/svc", "a.example.com", "fp1", now, DnsLookupFamily::All),
        Some(vec![ip("1.2.3.4")])
    );
}

#[test]
fn capacity_bounds_growth_across_unique_services() {
    // Single shard so capacity is exact rather than per-shard.
    let cache = DnsCache::with_capacity(4, 1);
    let now = Instant::now();
    for index in 0..50 {
        let service_key = format!("ns/svc-{index}");
        cache.insert(
            &service_key,
            "a.example.com",
            "fp1",
            vec![ip("1.2.3.4")],
            SEC5,
            now,
            DnsLookupFamily::All,
        );
    }
    assert_eq!(cache.len(), 4);
    // The oldest inserts were evicted; the most recent survive.
    assert_eq!(
        cache.get("ns/svc-0", "a.example.com", "fp1", now, DnsLookupFamily::All),
        None
    );
    assert_eq!(
        cache.get("ns/svc-49", "a.example.com", "fp1", now, DnsLookupFamily::All),
        Some(vec![ip("1.2.3.4")])
    );
}

#[test]
fn capacity_bounds_growth_across_resolver_and_hostname_churn() {
    let cache = DnsCache::with_capacity(4, 1);
    let now = Instant::now();
    for index in 0..50 {
        // One Service whose hostname and resolver both keep changing.
        let external_name = format!("h{index}.example.com");
        let fingerprint = format!("fp{index}");
        cache.insert(
            "ns/svc",
            &external_name,
            &fingerprint,
            vec![ip("1.2.3.4")],
            SEC5,
            now,
            DnsLookupFamily::All,
        );
    }
    assert_eq!(cache.len(), 4);
}

#[test]
fn expired_entry_is_reclaimed_in_preference_to_evicting_a_live_one() {
    // The LRU receives the same TTL, so when the cache is full it prunes the
    // expired entry rather than displacing a live one. Reclamation is lazy:
    // this asserts the behavior under capacity pressure, which is where the
    // retention bound is actually enforced.
    let cache = DnsCache::with_capacity(2, 1);
    let now = Instant::now();
    // Order matters: `ns/live` goes in FIRST so it is the LRU tail. If the
    // TTL were not handed to the LRU, `prune_expired` would find nothing and
    // the third insert would displace the tail — evicting the live entry and
    // failing this test. Inserting the stale entry first would make it both
    // the expired entry and the eviction victim, which plain LRU removes
    // either way, and the test would prove nothing.
    // A 60s TTL (not SEC5) keeps the live entry clear of the LRU's real-clock
    // deadline no matter how long the surrounding test run stalls.
    cache.insert(
        "ns/live",
        "b.example.com",
        "fp1",
        vec![ip("2.2.2.2")],
        MIN1,
        now,
        DnsLookupFamily::All,
    );
    cache.insert(
        "ns/stale",
        "a.example.com",
        "fp1",
        vec![ip("1.1.1.1")],
        Duration::from_millis(20),
        now,
        DnsLookupFamily::All,
    );
    assert_eq!(cache.len(), 2);

    std::thread::sleep(Duration::from_millis(40));
    let later = Instant::now();
    cache.insert(
        "ns/new",
        "c.example.com",
        "fp1",
        vec![ip("3.3.3.3")],
        MIN1,
        later,
        DnsLookupFamily::All,
    );

    assert_eq!(cache.len(), 2);
    assert_eq!(
        cache.get("ns/stale", "a.example.com", "fp1", later, DnsLookupFamily::All),
        None
    );
    // The live entry was not displaced to make room for the new one.
    assert_eq!(
        cache.get("ns/live", "b.example.com", "fp1", later, DnsLookupFamily::All),
        Some(vec![ip("2.2.2.2")])
    );
    assert_eq!(
        cache.get("ns/new", "c.example.com", "fp1", later, DnsLookupFamily::All),
        Some(vec![ip("3.3.3.3")])
    );
}

#[test]
fn concurrent_readers_and_invalidation_stay_consistent() {
    use std::sync::Arc;
    use std::thread;

    let cache = Arc::new(DnsCache::with_capacity(256, 8));
    let now = Instant::now();
    // Each key gets its OWN address so a reader can assert it got back the
    // value belonging to the key it asked for. A single shared value would
    // make the assertion unfalsifiable and would not catch a shard-index or
    // key-hashing mistake.
    for index in 0..32 {
        cache.insert(
            &format!("ns/svc-{index}"),
            "a.example.com",
            "fp1",
            vec![ip(&format!("10.0.0.{index}"))],
            MIN1,
            now,
            DnsLookupFamily::All,
        );
    }

    let mut handles = Vec::new();
    for _ in 0..4 {
        let cache = Arc::clone(&cache);
        handles.push(thread::spawn(move || {
            for _ in 0..200 {
                for index in 0..32 {
                    // Either this key's own address set or nothing — never
                    // another key's value.
                    let got = cache.get(
                        &format!("ns/svc-{index}"),
                        "a.example.com",
                        "fp1",
                        Instant::now(),
                        DnsLookupFamily::All,
                    );
                    if let Some(addrs) = got {
                        assert_eq!(addrs, vec![ip(&format!("10.0.0.{index}"))]);
                    }
                }
            }
        }));
    }
    {
        let cache = Arc::clone(&cache);
        handles.push(thread::spawn(move || {
            for index in 0..32 {
                cache.invalidate_services(&HashSet::from([format!("ns/svc-{index}")]));
            }
        }));
    }
    for handle in handles {
        handle.join().expect("worker thread panicked");
    }
    assert_eq!(cache.len(), 0);
}

#[test]
fn borrowed_query_preserves_owned_key_identity_and_hash() {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    #[derive(Hash)]
    struct ExpectedKey {
        service_key: String,
        external_name: String,
        fingerprint: String,
        family: DnsLookupFamily,
    }

    fn hash(value: &impl Hash) -> u64 {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

    let cases = [
        ("", "", ""),
        ("ab", "c", "system"),
        ("a", "bc", "system"),
        ("a", "b", "csystem"),
        ("ns/service", "A.test", "system"),
        ("ns/service", "a.test", "system"),
        ("ns/service", "a.test", "std:192.0.2.53"),
        ("ns/other", "a.test", "system"),
        ("ns/é", "ñ.test", "π"),
    ];
    for (index, &(service_key, external_name, fingerprint)) in cases.iter().enumerate() {
        let owned = DnsCacheKey::new(service_key, external_name, fingerprint, DnsLookupFamily::All);
        let expected = ExpectedKey {
            service_key: service_key.to_string(),
            external_name: external_name.to_string(),
            fingerprint: fingerprint.to_string(),
            family: DnsLookupFamily::All,
        };
        assert_eq!(hash(&owned), hash(&expected));
        for (other, &(service_key, external_name, fingerprint)) in cases.iter().enumerate() {
            let query = DnsCacheQuery {
                family: DnsLookupFamily::All,
                service_key: Cow::Borrowed(service_key),
                external_name: Cow::Borrowed(external_name),
                fingerprint: Cow::Borrowed(fingerprint),
            };
            let borrowed: &DnsCacheQuery<'_> = owned.borrow();
            assert_eq!(borrowed == &query, index == other);
            if index == other {
                assert_eq!(hash(&query), hash(&expected));
            }
        }
    }
}

#[test]
fn lookup_allocates_only_the_owned_address_result() {
    let cache = cache();
    let now = Instant::now();
    let addresses = vec![ip("192.0.2.4"), ip("2001:db8::4")];
    cache.insert(
        "ns/service",
        "a.test",
        "system",
        addresses.clone(),
        MIN1,
        now,
        DnsLookupFamily::All,
    );
    let (_, result_allocations) = crate::test_allocation_counter::count_allocations(|| addresses.clone());
    let (hit, allocations) = crate::test_allocation_counter::count_allocations(|| {
        cache.get("ns/service", "a.test", "system", now, DnsLookupFamily::All)
    });
    assert_eq!(hit, Some(addresses));
    assert_eq!(allocations, result_allocations);

    for (service, host, resolver) in [
        ("ns/other", "a.test", "system"),
        ("ns/service", "other.test", "system"),
        ("ns/service", "a.test", "std:192.0.2.53"),
    ] {
        let (miss, allocations) = crate::test_allocation_counter::count_allocations(|| {
            cache.get(service, host, resolver, now, DnsLookupFamily::All)
        });
        assert!(miss.is_none());
        assert_eq!(allocations, 0);
    }
}
