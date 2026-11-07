use super::*;
use crate::dns::StandardDnsServers;
use std::sync::atomic::{AtomicU64, AtomicUsize};
use tokio::sync::Semaphore;

const A: &str = "192.0.2.1";
const B: &str = "192.0.2.2";

struct Harness {
    cache: Arc<DnsAnswerCache>,
    offset_ms: Arc<AtomicU64>,
    calls: Arc<AtomicUsize>,
}

impl Harness {
    fn new() -> Self {
        Self::with_capacity(64)
    }

    fn with_capacity(capacity: usize) -> Self {
        let base = Instant::now();
        let offset_ms = Arc::new(AtomicU64::new(0));
        let clock_offset = offset_ms.clone();
        let cache = DnsAnswerCache::with_clock(
            capacity,
            1,
            Arc::new(move || base + Duration::from_millis(clock_offset.load(Ordering::SeqCst))),
        );
        Self {
            cache: Arc::new(cache),
            offset_ms,
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn advance(&self, by: Duration) {
        self.offset_ms.fetch_add(by.as_millis() as u64, Ordering::SeqCst);
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// Look up `name` with a resolver that returns `result` and counts calls.
    async fn lookup(
        &self,
        spec: &ResolverSpec,
        name: &str,
        query_policy: DnsQueryPolicy,
        policy: &DnsCachePolicy,
        result: Result<DnsAnswer, DnsError>,
    ) -> Result<Vec<IpAddr>, DnsError> {
        let calls = self.calls.clone();
        self.cache
            .lookup_with(spec, name, query_policy, policy, move |_, _, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                std::future::ready(result)
            })
            .await
    }
}

fn answer(ip: &str, ttl: Option<u64>) -> Result<DnsAnswer, DnsError> {
    Ok(DnsAnswer {
        addrs: vec![ip.parse().unwrap()],
        ttl: ttl.map(Duration::from_secs),
    })
}

fn ips(ip: &str) -> Vec<IpAddr> {
    vec![ip.parse().unwrap()]
}

fn policy() -> DnsCachePolicy {
    DnsCachePolicy::new(Duration::from_secs(5))
}

const NATIVE: DnsQueryPolicy = DnsQueryPolicy::Native;

#[tokio::test]
async fn fresh_answer_is_served_without_resolving_again() {
    let h = Harness::new();
    let spec = ResolverSpec::System;
    assert_eq!(
        h.lookup(&spec, "a.test", NATIVE, &policy(), answer(A, Some(60)))
            .await
            .unwrap(),
        ips(A)
    );
    assert_eq!(
        h.lookup(&spec, "a.test", NATIVE, &policy(), answer(B, Some(60)))
            .await
            .unwrap(),
        ips(A)
    );
    assert_eq!(h.calls(), 1);
    assert_eq!(h.cache.stats().entries, 1);
    assert_eq!(h.cache.stats().in_flight, 0);
}

#[tokio::test]
async fn resolver_name_and_query_policy_are_separate_keys() {
    let h = Harness::new();
    let system = ResolverSpec::System;
    let standard = ResolverSpec::StandardDns(StandardDnsServers::new(vec!["192.0.2.53".into()]));
    let family = DnsQueryPolicy::Family(crate::dns::DnsLookupFamily::Ipv4Only);
    h.lookup(&system, "a.test", NATIVE, &policy(), answer(A, None))
        .await
        .unwrap();
    h.lookup(&standard, "a.test", NATIVE, &policy(), answer(A, None))
        .await
        .unwrap();
    h.lookup(&system, "b.test", NATIVE, &policy(), answer(A, None))
        .await
        .unwrap();
    h.lookup(&system, "a.test", family, &policy(), answer(A, None))
        .await
        .unwrap();
    assert_eq!(h.calls(), 4);
    h.lookup(&standard, "a.test", NATIVE, &policy(), answer(B, None))
        .await
        .unwrap();
    assert_eq!(h.calls(), 4, "each key is now warm");
}

#[tokio::test]
async fn record_ttl_is_clamped_and_missing_ttl_uses_the_fallback() {
    let mut no_stale = policy();
    no_stale.stale_ttl = Duration::ZERO;
    let spec = ResolverSpec::System;

    // TTL 0 is floored to five seconds (Envoy dns_min_refresh_rate).
    let h = Harness::new();
    h.lookup(&spec, "zero.test", NATIVE, &no_stale, answer(A, Some(0)))
        .await
        .unwrap();
    h.advance(Duration::from_millis(4_900));
    h.lookup(&spec, "zero.test", NATIVE, &no_stale, answer(B, Some(0)))
        .await
        .unwrap();
    assert_eq!(h.calls(), 1);
    h.advance(Duration::from_millis(200));
    h.lookup(&spec, "zero.test", NATIVE, &no_stale, answer(B, Some(0)))
        .await
        .unwrap();
    assert_eq!(h.calls(), 2);

    // A long TTL is capped at max_ttl.
    let h = Harness::new();
    h.lookup(&spec, "long.test", NATIVE, &no_stale, answer(A, Some(86_400)))
        .await
        .unwrap();
    h.advance(Duration::from_secs(299));
    h.lookup(&spec, "long.test", NATIVE, &no_stale, answer(B, None))
        .await
        .unwrap();
    assert_eq!(h.calls(), 1);
    h.advance(Duration::from_secs(2));
    assert_eq!(
        h.lookup(&spec, "long.test", NATIVE, &no_stale, answer(B, None))
            .await
            .unwrap(),
        ips(B)
    );
    assert_eq!(h.calls(), 2);

    // No reported TTL: the consumer fallback (5 s) applies.
    let h = Harness::new();
    h.lookup(&spec, "none.test", NATIVE, &no_stale, answer(A, None))
        .await
        .unwrap();
    h.advance(Duration::from_millis(4_900));
    h.lookup(&spec, "none.test", NATIVE, &no_stale, answer(B, None))
        .await
        .unwrap();
    assert_eq!(h.calls(), 1);
    h.advance(Duration::from_millis(200));
    h.lookup(&spec, "none.test", NATIVE, &no_stale, answer(B, None))
        .await
        .unwrap();
    assert_eq!(h.calls(), 2);
}

#[tokio::test]
async fn zero_fallback_disables_the_cache_and_literals_bypass_it() {
    let h = Harness::new();
    let spec = ResolverSpec::System;
    let disabled = DnsCachePolicy::new(Duration::ZERO);
    for _ in 0..3 {
        h.lookup(&spec, "a.test", NATIVE, &disabled, answer(A, Some(60)))
            .await
            .unwrap();
    }
    assert_eq!(h.calls(), 3);
    for _ in 0..2 {
        h.lookup(&spec, "192.0.2.9", NATIVE, &policy(), answer(A, Some(60)))
            .await
            .unwrap();
        h.lookup(&spec, "[2001:db8::9]", NATIVE, &policy(), answer(A, Some(60)))
            .await
            .unwrap();
    }
    assert_eq!(h.calls(), 7);
    assert_eq!(h.cache.stats().entries, 0);
}

#[tokio::test]
async fn concurrent_cold_misses_share_one_resolver_call() {
    let h = Harness::new();
    let gate = Arc::new(Semaphore::new(0));
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..20 {
        let cache = h.cache.clone();
        let calls = h.calls.clone();
        let gate = gate.clone();
        tasks.spawn(async move {
            let spec = ResolverSpec::System;
            cache
                .lookup_with(&spec, "cold.test", NATIVE, &policy(), move |_, _, _| async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    gate.acquire().await.unwrap().forget();
                    answer(A, Some(60))
                })
                .await
        });
    }
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    assert_eq!(h.cache.stats().in_flight, 1);
    gate.add_permits(1);
    bounded(async {
        while let Some(result) = tasks.join_next().await {
            assert_eq!(result.unwrap().unwrap(), ips(A));
        }
    })
    .await;
    assert_eq!(h.calls(), 1);
    assert_eq!(h.cache.stats().in_flight, 0);
}

#[tokio::test]
async fn stale_answer_is_served_without_waiting_while_one_request_refreshes() {
    let h = Harness::new();
    let spec = ResolverSpec::System;
    h.lookup(&spec, "stale.test", NATIVE, &policy(), answer(A, Some(5)))
        .await
        .unwrap();
    h.advance(Duration::from_secs(6));

    let gate = Arc::new(Semaphore::new(0));
    let refresher = {
        let cache = h.cache.clone();
        let calls = h.calls.clone();
        let gate = gate.clone();
        tokio::spawn(async move {
            let spec = ResolverSpec::System;
            cache
                .lookup_with(&spec, "stale.test", NATIVE, &policy(), move |_, _, _| async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    gate.acquire().await.unwrap().forget();
                    answer(B, Some(60))
                })
                .await
        })
    };
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert_eq!(h.calls(), 2, "the refresher is resolving");
    // Concurrent readers get the stale answer immediately and never resolve.
    assert_eq!(
        bounded(h.lookup(&spec, "stale.test", NATIVE, &policy(), answer(B, Some(60))))
            .await
            .unwrap(),
        ips(A)
    );
    assert_eq!(h.calls(), 2);

    gate.add_permits(1);
    assert_eq!(bounded(refresher).await.unwrap().unwrap(), ips(B));
    assert_eq!(
        h.lookup(&spec, "stale.test", NATIVE, &policy(), answer(A, Some(60)))
            .await
            .unwrap(),
        ips(B)
    );
    assert_eq!(h.calls(), 2);
}

#[tokio::test]
async fn transient_refresh_failure_keeps_stale_and_backs_off() {
    let h = Harness::new();
    let spec = ResolverSpec::System;
    let io = || Err(DnsError::Io("timeout".into()));
    h.lookup(&spec, "flaky.test", NATIVE, &policy(), answer(A, Some(5)))
        .await
        .unwrap();
    h.advance(Duration::from_secs(6));
    assert_eq!(
        h.lookup(&spec, "flaky.test", NATIVE, &policy(), io()).await.unwrap(),
        ips(A)
    );
    assert_eq!(h.calls(), 2);
    // Inside the back-off window no further refresh is attempted.
    assert_eq!(
        h.lookup(&spec, "flaky.test", NATIVE, &policy(), io()).await.unwrap(),
        ips(A)
    );
    assert_eq!(h.calls(), 2);
    h.advance(Duration::from_millis(1_100));
    assert_eq!(
        h.lookup(&spec, "flaky.test", NATIVE, &policy(), io()).await.unwrap(),
        ips(A)
    );
    assert_eq!(h.calls(), 3);
    // The back-off doubles after the second consecutive failure.
    h.advance(Duration::from_millis(1_100));
    h.lookup(&spec, "flaky.test", NATIVE, &policy(), io()).await.unwrap();
    assert_eq!(h.calls(), 3, "still inside the 2 s back-off");
    h.advance(Duration::from_millis(1_000));
    h.lookup(&spec, "flaky.test", NATIVE, &policy(), io()).await.unwrap();
    assert_eq!(h.calls(), 4);
    // An unused answer still expires: after its (slid) stale window the
    // failure surfaces and is cached as negative.
    h.advance(Duration::from_secs(40));
    assert!(h.lookup(&spec, "flaky.test", NATIVE, &policy(), io()).await.is_err());
    assert_eq!(h.calls(), 5);
}

#[tokio::test]
async fn requested_answer_survives_a_resolver_outage_until_its_stale_limit() {
    let h = Harness::new();
    let spec = ResolverSpec::System;
    let io = || Err(DnsError::Io("timeout".into()));
    h.lookup(&spec, "outage.test", NATIVE, &policy(), answer(A, Some(5)))
        .await
        .unwrap();
    // A failing resolver with steady traffic every 9 seconds: the last good
    // answer keeps being served (Envoy keeps the last resolution) up to its
    // stale limit, 300 s past its 5 s freshness.
    for _ in 0..33 {
        h.advance(Duration::from_secs(9));
        assert_eq!(
            h.lookup(&spec, "outage.test", NATIVE, &policy(), io()).await.unwrap(),
            ips(A)
        );
    }
    // Back-off 1, 2, 4, 8 s, then capped at 10 s: once capped, every other
    // 9-second request is inside the back-off and does not query DNS.
    assert_eq!(h.calls() - 1, 19, "refreshes follow the capped exponential back-off");
    // Past the stale limit (t = 306 s > 305 s) the failure surfaces: a deleted
    // record that the resolver reports as a transient error stops being dialed.
    h.advance(Duration::from_secs(9));
    assert_eq!(
        h.lookup(&spec, "outage.test", NATIVE, &policy(), io()).await,
        Err(DnsError::Io("timeout".into()))
    );
    // Recovery writes a fresh answer and resets the back-off.
    h.advance(Duration::from_secs(11));
    assert_eq!(
        h.lookup(&spec, "outage.test", NATIVE, &policy(), answer(B, Some(60)))
            .await
            .unwrap(),
        ips(B)
    );
}

#[tokio::test]
async fn refresh_failure_at_the_stale_limit_drops_the_answer() {
    let h = Harness::new();
    let spec = ResolverSpec::System;
    let io = || Err(DnsError::Io("timeout".into()));
    h.lookup(&spec, "limit.test", NATIVE, &policy(), answer(A, Some(5)))
        .await
        .unwrap();
    // Keep the answer requested so its stale window slides to the limit.
    for _ in 0..30 {
        h.advance(Duration::from_secs(10));
        h.lookup(&spec, "limit.test", NATIVE, &policy(), io()).await.unwrap();
    }
    // t = 300 s: the window can no longer slide past 305 s.
    h.advance(Duration::from_millis(4_900));
    assert_eq!(
        h.lookup(&spec, "limit.test", NATIVE, &policy(), io()).await.unwrap(),
        ips(A),
        "still inside the stale limit"
    );
    h.advance(Duration::from_millis(200));
    assert!(
        h.lookup(&spec, "limit.test", NATIVE, &policy(), io()).await.is_err(),
        "a refresh failure past the stale limit no longer keeps the answer"
    );
}

#[tokio::test]
async fn authoritative_empty_answer_drops_the_stale_answer() {
    let h = Harness::new();
    let spec = ResolverSpec::System;
    h.lookup(&spec, "gone.test", NATIVE, &policy(), answer(A, Some(5)))
        .await
        .unwrap();
    h.advance(Duration::from_secs(6));
    assert_eq!(
        h.lookup(&spec, "gone.test", NATIVE, &policy(), Err(DnsError::NoAddresses))
            .await,
        Err(DnsError::NoAddresses)
    );
    assert_eq!(
        h.lookup(&spec, "gone.test", NATIVE, &policy(), answer(A, Some(60)))
            .await,
        Err(DnsError::NoAddresses),
        "the negative entry answers without resolving"
    );
    assert_eq!(h.calls(), 2);
}

#[tokio::test]
async fn failures_are_cached_for_the_negative_window() {
    let h = Harness::new();
    let spec = ResolverSpec::System;
    let io = || Err(DnsError::Io("refused".into()));
    assert!(h.lookup(&spec, "down.test", NATIVE, &policy(), io()).await.is_err());
    assert!(h
        .lookup(&spec, "down.test", NATIVE, &policy(), answer(A, None))
        .await
        .is_err());
    assert_eq!(h.calls(), 1);
    // Consecutive failures double the negative window: 1 s, 2 s, 4 s.
    h.advance(Duration::from_millis(1_100));
    assert!(h.lookup(&spec, "down.test", NATIVE, &policy(), io()).await.is_err());
    assert_eq!(h.calls(), 2);
    h.advance(Duration::from_millis(1_900));
    assert!(h
        .lookup(&spec, "down.test", NATIVE, &policy(), answer(A, None))
        .await
        .is_err());
    assert_eq!(h.calls(), 2, "still inside the 2 s window");
    h.advance(Duration::from_millis(200));
    assert!(h.lookup(&spec, "down.test", NATIVE, &policy(), io()).await.is_err());
    assert_eq!(h.calls(), 3);
    h.advance(Duration::from_millis(3_900));
    assert!(h
        .lookup(&spec, "down.test", NATIVE, &policy(), answer(A, None))
        .await
        .is_err());
    assert_eq!(h.calls(), 3, "still inside the 4 s window");
    h.advance(Duration::from_millis(200));
    assert_eq!(
        h.lookup(&spec, "down.test", NATIVE, &policy(), answer(A, None))
            .await
            .unwrap(),
        ips(A)
    );
    assert_eq!(h.calls(), 4);
}

#[test]
fn empty_cache_reports_zero() {
    let cache = DnsAnswerCache::new(8, 1);
    let stats = cache.stats();
    assert_eq!(stats.entries, 0);
    assert_eq!(stats.in_flight, 0);
    assert!(stats.in_flight_keys.is_empty());
}

/// Fail instead of hanging when a coordination bug leaves a future pending.
async fn bounded<F: std::future::Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("cache coordination must not hang")
}

#[tokio::test]
async fn freshness_and_negative_deadlines_are_exclusive() {
    let mut no_stale = policy();
    no_stale.stale_ttl = Duration::ZERO;
    let spec = ResolverSpec::System;
    let h = Harness::new();
    h.lookup(&spec, "edge.test", NATIVE, &no_stale, answer(A, Some(5)))
        .await
        .unwrap();
    h.advance(Duration::from_millis(4_999));
    h.lookup(&spec, "edge.test", NATIVE, &no_stale, answer(B, Some(5)))
        .await
        .unwrap();
    assert_eq!(h.calls(), 1);
    h.advance(Duration::from_millis(1));
    assert_eq!(
        h.lookup(&spec, "edge.test", NATIVE, &no_stale, answer(B, Some(5)))
            .await
            .unwrap(),
        ips(B),
        "an answer is no longer fresh exactly at its deadline"
    );
    assert_eq!(h.calls(), 2);

    let io = || Err(DnsError::Io("refused".into()));
    assert!(h.lookup(&spec, "neg.test", NATIVE, &no_stale, io()).await.is_err());
    h.advance(Duration::from_secs(1));
    assert_eq!(
        h.lookup(&spec, "neg.test", NATIVE, &no_stale, answer(A, None))
            .await
            .unwrap(),
        ips(A)
    );
    assert_eq!(h.calls(), 4, "a negative entry expires exactly at its deadline");
}

#[tokio::test]
async fn disabled_policy_never_caches_failures_and_negative_entries_keep_the_error() {
    let h = Harness::new();
    let spec = ResolverSpec::System;
    let disabled = DnsCachePolicy::new(Duration::ZERO);
    let io = || Err(DnsError::Io("refused".into()));
    for _ in 0..2 {
        assert!(h.lookup(&spec, "off.test", NATIVE, &disabled, io()).await.is_err());
    }
    assert_eq!(h.calls(), 2);
    assert_eq!(h.cache.stats().entries, 0);

    let refused = Err(DnsError::Io("refused".into()));
    assert_eq!(h.lookup(&spec, "neg.test", NATIVE, &policy(), io()).await, refused);
    assert_eq!(
        h.lookup(&spec, "neg.test", NATIVE, &policy(), answer(A, None)).await,
        refused,
        "the cached negative entry returns the original error"
    );
    assert_eq!(h.calls(), 3);
}

#[tokio::test]
async fn fallback_ttl_is_capped_and_overlong_names_bypass_the_cache() {
    let mut long_fallback = DnsCachePolicy::new(Duration::from_secs(3_600));
    long_fallback.stale_ttl = Duration::ZERO;
    let spec = ResolverSpec::System;
    let h = Harness::new();
    h.lookup(&spec, "sys.test", NATIVE, &long_fallback, answer(A, None))
        .await
        .unwrap();
    h.advance(Duration::from_secs(299));
    h.lookup(&spec, "sys.test", NATIVE, &long_fallback, answer(B, None))
        .await
        .unwrap();
    assert_eq!(h.calls(), 1);
    h.advance(Duration::from_secs(2));
    h.lookup(&spec, "sys.test", NATIVE, &long_fallback, answer(B, None))
        .await
        .unwrap();
    assert_eq!(h.calls(), 2, "a TTL-less answer is still capped at max_ttl");

    let huge = DnsCachePolicy::new(Duration::MAX);
    h.lookup(&spec, "max.test", NATIVE, &huge, answer(A, None))
        .await
        .unwrap();
    h.lookup(&spec, "max.test", NATIVE, &huge, answer(B, None))
        .await
        .unwrap();
    assert_eq!(
        h.calls(),
        3,
        "an unrepresentable fallback is capped, not a silent bypass"
    );

    let long_name = format!("{}.test", "a".repeat(260));
    for _ in 0..2 {
        h.lookup(&spec, &long_name, NATIVE, &policy(), answer(A, None))
            .await
            .unwrap();
    }
    assert_eq!(h.calls(), 5, "names longer than a DNS name are never cached");
}

#[tokio::test]
async fn queued_waiter_takes_over_from_a_cancelled_leader() {
    let h = Harness::new();
    let leader = {
        let cache = h.cache.clone();
        let calls = h.calls.clone();
        tokio::spawn(async move {
            let spec = ResolverSpec::System;
            cache
                .lookup_with(&spec, "handover.test", NATIVE, &policy(), move |_, _, _| async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    std::future::pending::<()>().await;
                    answer(A, Some(60))
                })
                .await
        })
    };
    let waiter = {
        let cache = h.cache.clone();
        let calls = h.calls.clone();
        tokio::spawn(async move {
            let spec = ResolverSpec::System;
            cache
                .lookup_with(&spec, "handover.test", NATIVE, &policy(), move |_, _, _| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    std::future::ready(answer(B, Some(60)))
                })
                .await
        })
    };
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert_eq!(h.calls(), 1, "the leader is resolving and the waiter is queued");
    assert_eq!(h.cache.stats().in_flight, 1);
    leader.abort();
    assert_eq!(bounded(waiter).await.unwrap().unwrap(), ips(B));
    assert_eq!(h.calls(), 2);
    assert_eq!(h.cache.stats().in_flight, 0);
}

#[tokio::test]
async fn failed_stale_refresh_never_overwrites_a_newer_answer() {
    for late_error in [DnsError::Io("timeout".into()), DnsError::NoAddresses] {
        let h = Harness::new();
        let spec = ResolverSpec::System;
        h.lookup(&spec, "race.test", NATIVE, &policy(), answer(A, Some(5)))
            .await
            .unwrap();
        h.advance(Duration::from_secs(6));
        let gate = Arc::new(Semaphore::new(0));
        let refresher = {
            let cache = h.cache.clone();
            let gate = gate.clone();
            let late_error = late_error.clone();
            tokio::spawn(async move {
                let spec = ResolverSpec::System;
                cache
                    .lookup_with(&spec, "race.test", NATIVE, &policy(), move |_, _, _| async move {
                        gate.acquire().await.unwrap().forget();
                        Err(late_error)
                    })
                    .await
            })
        };
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        // The refresher is still resolving past the answer's stale limit
        // (5 s fresh + 300 s); a cold miss then resolves and writes a newer
        // answer (before the limit it would be served the claimed answer).
        h.advance(Duration::from_secs(300));
        assert_eq!(
            h.lookup(&spec, "race.test", NATIVE, &policy(), answer(B, Some(60)))
                .await
                .unwrap(),
            ips(B)
        );
        gate.add_permits(1);
        assert_eq!(
            bounded(refresher).await.unwrap().unwrap(),
            ips(B),
            "{late_error:?}: the late failure is served the newer answer"
        );
        assert_eq!(
            h.lookup(&spec, "race.test", NATIVE, &policy(), Err(DnsError::NoAddresses))
                .await
                .unwrap(),
            ips(B),
            "{late_error:?}: the newer answer survives the late failure"
        );
    }
}

#[tokio::test]
async fn concurrent_cold_failures_share_one_resolver_call() {
    let h = Harness::new();
    let gate = Arc::new(Semaphore::new(0));
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..20 {
        let cache = h.cache.clone();
        let calls = h.calls.clone();
        let gate = gate.clone();
        tasks.spawn(async move {
            let spec = ResolverSpec::System;
            cache
                .lookup_with(&spec, "down.test", NATIVE, &policy(), move |_, _, _| async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    gate.acquire().await.unwrap().forget();
                    Err::<DnsAnswer, _>(DnsError::Io("refused".into()))
                })
                .await
        });
    }
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    gate.add_permits(20);
    bounded(async {
        while let Some(result) = tasks.join_next().await {
            assert_eq!(result.unwrap(), Err(DnsError::Io("refused".into())));
        }
    })
    .await;
    assert_eq!(h.calls(), 1, "queued callers are served the leader's negative entry");
    assert_eq!(h.cache.stats().in_flight, 0);
}

#[tokio::test]
async fn cancelled_stale_refresh_counts_as_a_failed_refresh() {
    let h = Harness::new();
    let spec = ResolverSpec::System;
    h.lookup(&spec, "cancel.test", NATIVE, &policy(), answer(A, Some(5)))
        .await
        .unwrap();
    h.advance(Duration::from_secs(6));
    let refresher = {
        let cache = h.cache.clone();
        let calls = h.calls.clone();
        tokio::spawn(async move {
            let spec = ResolverSpec::System;
            cache
                .lookup_with(&spec, "cancel.test", NATIVE, &policy(), move |_, _, _| async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    std::future::pending::<()>().await;
                    answer(B, Some(60))
                })
                .await
        })
    };
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert_eq!(h.calls(), 2, "the refresher claimed the entry");
    // The caller's deadline cancels the refresh (the resolver outlived it).
    refresher.abort();
    assert!(bounded(refresher).await.unwrap_err().is_cancelled());
    // Treated as a failed refresh: the answer is kept and the next refresh
    // backs off instead of every request re-claiming a hung resolver.
    assert_eq!(
        h.lookup(&spec, "cancel.test", NATIVE, &policy(), answer(B, Some(60)))
            .await
            .unwrap(),
        ips(A)
    );
    assert_eq!(h.calls(), 2, "inside the 1 s back-off");
    // The window slid from the cancellation time, so after the stale window
    // the answer would otherwise have ended (t = 35 s) it is still served.
    h.advance(Duration::from_secs(1));
    assert_eq!(
        h.lookup(&spec, "cancel.test", NATIVE, &policy(), answer(B, Some(60)))
            .await
            .unwrap(),
        ips(B),
        "the claim was released, so the next request refreshes"
    );
    assert_eq!(h.calls(), 3);
}

#[tokio::test]
async fn repeatedly_cancelled_refreshes_keep_the_answer_until_its_stale_limit() {
    let h = Harness::new();
    let spec = ResolverSpec::System;
    h.lookup(&spec, "hung.test", NATIVE, &policy(), answer(A, Some(5)))
        .await
        .unwrap();
    // Every refresh hangs past its caller's deadline and is cancelled.
    for _ in 0..30 {
        h.advance(Duration::from_secs(10));
        let refresher = {
            let cache = h.cache.clone();
            tokio::spawn(async move {
                let spec = ResolverSpec::System;
                cache
                    .lookup_with(&spec, "hung.test", NATIVE, &policy(), move |_, _, _| async move {
                        std::future::pending::<()>().await;
                        answer(B, Some(60))
                    })
                    .await
            })
        };
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        refresher.abort();
        let _ = bounded(refresher).await;
        assert_eq!(
            h.lookup(
                &spec,
                "hung.test",
                NATIVE,
                &policy(),
                Err(DnsError::Io("timeout".into()))
            )
            .await
            .unwrap(),
            ips(A)
        );
    }
    // t = 306 s: past the stale limit nothing keeps the answer.
    h.advance(Duration::from_secs(6));
    assert!(h
        .lookup(
            &spec,
            "hung.test",
            NATIVE,
            &policy(),
            Err(DnsError::Io("timeout".into()))
        )
        .await
        .is_err());
}

#[tokio::test]
async fn cold_lookup_after_the_window_serves_an_answer_whose_refresh_is_running() {
    let h = Harness::new();
    let spec = ResolverSpec::System;
    h.lookup(&spec, "overlap.test", NATIVE, &policy(), answer(A, Some(5)))
        .await
        .unwrap();
    // Claim the refresh near the end of the stale window (fresh 5 s + stale 30 s).
    h.advance(Duration::from_secs(33));
    let gate = Arc::new(Semaphore::new(0));
    let refresher = {
        let cache = h.cache.clone();
        let calls = h.calls.clone();
        let gate = gate.clone();
        tokio::spawn(async move {
            let spec = ResolverSpec::System;
            cache
                .lookup_with(&spec, "overlap.test", NATIVE, &policy(), move |_, _, _| async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    gate.acquire().await.unwrap().forget();
                    Err::<DnsAnswer, _>(DnsError::Io("timeout".into()))
                })
                .await
        })
    };
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    // The window ends while the refresh still runs. A cold lookup neither
    // resolves a second time nor writes a failure over the claimed answer.
    h.advance(Duration::from_secs(10));
    assert_eq!(
        h.lookup(
            &spec,
            "overlap.test",
            NATIVE,
            &policy(),
            Err(DnsError::Io("timeout".into()))
        )
        .await
        .unwrap(),
        ips(A)
    );
    assert_eq!(h.calls(), 2);
    gate.add_permits(1);
    assert_eq!(bounded(refresher).await.unwrap().unwrap(), ips(A));
    assert_eq!(
        h.lookup(&spec, "overlap.test", NATIVE, &policy(), answer(B, Some(60)))
            .await
            .unwrap(),
        ips(A),
        "the failed refresh kept the answer and slid its window"
    );
}

#[tokio::test]
async fn refresh_failure_keeps_an_answer_evicted_while_it_ran() {
    let h = Harness::with_capacity(1);
    let spec = ResolverSpec::System;
    h.lookup(&spec, "evicted.test", NATIVE, &policy(), answer(A, Some(5)))
        .await
        .unwrap();
    h.advance(Duration::from_secs(6));
    let gate = Arc::new(Semaphore::new(0));
    let refresher = {
        let cache = h.cache.clone();
        let gate = gate.clone();
        tokio::spawn(async move {
            let spec = ResolverSpec::System;
            cache
                .lookup_with(&spec, "evicted.test", NATIVE, &policy(), move |_, _, _| async move {
                    gate.acquire().await.unwrap().forget();
                    Err::<DnsAnswer, _>(DnsError::Io("timeout".into()))
                })
                .await
        })
    };
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    // Another name evicts the entry from the one-slot cache.
    h.lookup(&spec, "other.test", NATIVE, &policy(), answer(B, Some(60)))
        .await
        .unwrap();
    gate.add_permits(1);
    assert_eq!(bounded(refresher).await.unwrap().unwrap(), ips(A));
    let calls = h.calls();
    assert_eq!(
        h.lookup(&spec, "evicted.test", NATIVE, &policy(), Err(DnsError::NoAddresses))
            .await
            .unwrap(),
        ips(A),
        "the last good answer was stored again, inside its refresh back-off"
    );
    assert_eq!(h.calls(), calls);
}

#[tokio::test]
async fn capacity_bounds_distinct_names() {
    let h = Harness::with_capacity(8);
    let spec = ResolverSpec::System;
    for index in 0..40 {
        h.lookup(&spec, &format!("n{index}.test"), NATIVE, &policy(), answer(A, Some(60)))
            .await
            .unwrap();
    }
    assert!(h.cache.stats().entries <= 8, "{}", h.cache.stats().entries);
}

#[test]
fn answers_are_retained_one_stale_window_past_their_stale_limit() {
    let now = Instant::now();
    let policy = policy();
    // A 5 s answer: stale limit 305 s, retained 335 s, so a refresh that
    // outlives the 30 s stale window never finds its answer reclaimed.
    assert_eq!(
        policy.answer_retention(now + Duration::from_secs(305), now),
        Duration::from_secs(335)
    );
    assert_eq!(policy.answer_retention(now, now), STALE_ANSWER_WINDOW);
}

#[tokio::test]
async fn a_failure_never_replaces_a_newer_answer_whose_refresh_is_running() {
    let h = Harness::with_capacity(1);
    let spec = ResolverSpec::System;
    let gated_failure = |gate: Arc<Semaphore>| {
        let cache = h.cache.clone();
        tokio::spawn(async move {
            let spec = ResolverSpec::System;
            cache
                .lookup_with(&spec, "newer.test", NATIVE, &policy(), move |_, _, _| async move {
                    gate.acquire().await.unwrap().forget();
                    Err::<DnsAnswer, _>(DnsError::Io("timeout".into()))
                })
                .await
        })
    };
    h.lookup(&spec, "newer.test", NATIVE, &policy(), answer(A, Some(5)))
        .await
        .unwrap();
    h.advance(Duration::from_secs(6));
    let first_gate = Arc::new(Semaphore::new(0));
    let first = gated_failure(first_gate.clone());
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    // Evict the claimed answer; a cold lookup then writes a newer one (B).
    h.lookup(&spec, "other.test", NATIVE, &policy(), answer(A, Some(60)))
        .await
        .unwrap();
    assert_eq!(
        h.lookup(&spec, "newer.test", NATIVE, &policy(), answer(B, Some(5)))
            .await
            .unwrap(),
        ips(B)
    );
    // B goes stale (t = 37 s) and a second request claims its refresh.
    h.advance(Duration::from_secs(31));
    let second_gate = Arc::new(Semaphore::new(0));
    let second = gated_failure(second_gate.clone());
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    // Past B's stale window (41 s), the first refresh fails: B is still under
    // its own refresh, so the failure must not replace it.
    h.advance(Duration::from_secs(10));
    first_gate.add_permits(1);
    assert_eq!(bounded(first).await.unwrap().unwrap(), ips(B));
    second_gate.add_permits(1);
    assert_eq!(
        bounded(second).await.unwrap().unwrap(),
        ips(B),
        "B is the last good answer"
    );
}

#[tokio::test]
async fn refresh_claimed_before_the_stale_limit_that_fails_after_it_drops_the_answer() {
    let h = Harness::new();
    let spec = ResolverSpec::System;
    let io = || Err(DnsError::Io("timeout".into()));
    h.lookup(&spec, "late.test", NATIVE, &policy(), answer(A, Some(5)))
        .await
        .unwrap();
    // Slide the window to the 305 s limit; the last refresh (t = 290 s)
    // blocks refreshes until 300 s.
    for _ in 0..29 {
        h.advance(Duration::from_secs(10));
        h.lookup(&spec, "late.test", NATIVE, &policy(), io()).await.unwrap();
    }
    h.advance(Duration::from_secs(11));
    let gate = Arc::new(Semaphore::new(0));
    let refresher = {
        let cache = h.cache.clone();
        let gate = gate.clone();
        tokio::spawn(async move {
            let spec = ResolverSpec::System;
            cache
                .lookup_with(&spec, "late.test", NATIVE, &policy(), move |_, _, _| async move {
                    gate.acquire().await.unwrap().forget();
                    Err::<DnsAnswer, _>(DnsError::Io("timeout".into()))
                })
                .await
        })
    };
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    let calls = h.calls();
    // The refresh claimed at t = 301 s returns after the limit.
    h.advance(Duration::from_secs(5));
    gate.add_permits(1);
    assert!(
        bounded(refresher).await.unwrap().is_err(),
        "never served past the limit"
    );
    // The failure count carries on: the 10 s back-off still applies.
    h.advance(Duration::from_secs(5));
    assert!(h
        .lookup(&spec, "late.test", NATIVE, &policy(), answer(B, Some(60)))
        .await
        .is_err());
    assert_eq!(h.calls(), calls, "inside the carried-over 10 s back-off");
}

#[tokio::test]
async fn listed_flight_keys_name_the_query_but_not_the_resolver() {
    let h = Harness::new();
    let gate = Arc::new(Semaphore::new(0));
    let leader = {
        let cache = h.cache.clone();
        let gate = gate.clone();
        tokio::spawn(async move {
            let spec = ResolverSpec::StandardDns(StandardDnsServers::new(vec!["192.0.2.53".into()]));
            cache
                .lookup_with(&spec, "keys.test", NATIVE, &policy(), move |_, _, _| async move {
                    gate.acquire().await.unwrap().forget();
                    answer(A, Some(60))
                })
                .await
        })
    };
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert_eq!(h.cache.stats().in_flight_keys, vec!["Native|keys.test".to_string()]);
    let counts = h.cache.stats_counts_only();
    assert_eq!(counts.in_flight, 1);
    assert!(counts.in_flight_keys.is_empty());
    gate.add_permits(1);
    bounded(leader).await.unwrap().unwrap();
}

#[tokio::test]
async fn slow_refresh_that_outlives_the_stale_window_still_keeps_the_last_good_answer() {
    let h = Harness::new();
    let spec = ResolverSpec::System;
    h.lookup(&spec, "slow.test", NATIVE, &policy(), answer(A, Some(5)))
        .await
        .unwrap();
    // Claim the refresh near the end of the stale window (fresh 5 s + stale 30 s).
    h.advance(Duration::from_secs(33));
    let gate = Arc::new(Semaphore::new(0));
    let refresher = {
        let cache = h.cache.clone();
        let gate = gate.clone();
        tokio::spawn(async move {
            let spec = ResolverSpec::System;
            cache
                .lookup_with(&spec, "slow.test", NATIVE, &policy(), move |_, _, _| async move {
                    gate.acquire().await.unwrap().forget();
                    Err(DnsError::Io("timeout".into()))
                })
                .await
        })
    };
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    // The resolver times out after the window has ended.
    h.advance(Duration::from_secs(10));
    gate.add_permits(1);
    assert_eq!(
        bounded(refresher).await.unwrap().unwrap(),
        ips(A),
        "the request that claimed a servable answer keeps it"
    );
    assert_eq!(
        h.lookup(&spec, "slow.test", NATIVE, &policy(), answer(B, Some(60)))
            .await
            .unwrap(),
        ips(A),
        "the window slid from the failure time, so the answer is still served"
    );
}

/// Real time: the LRU reclaims entries by wall clock, so retention is only
/// observable without the injected clock.
#[tokio::test]
async fn negative_entries_outlive_their_window_so_the_back_off_grows() {
    let cache = DnsAnswerCache::new(64, 1);
    let mut short = policy();
    short.negative_ttl = Duration::from_secs(2);
    short.max_negative_ttl = Duration::from_secs(20);
    let spec = ResolverSpec::System;
    let calls = Arc::new(AtomicUsize::new(0));
    let fail = |calls: Arc<AtomicUsize>| {
        move |_: &ResolverSpec, _: &str, _: DnsQueryPolicy| {
            calls.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Err::<DnsAnswer, _>(DnsError::Io("refused".into())))
        }
    };
    assert!(cache
        .lookup_with(&spec, "real.test", NATIVE, &short, fail(calls.clone()))
        .await
        .is_err());
    // After the first 2 s window the second failure must double to 4 s, which
    // needs the first negative entry to outlive its window in the LRU.
    tokio::time::sleep(Duration::from_millis(3_000)).await;
    assert!(cache
        .lookup_with(&spec, "real.test", NATIVE, &short, fail(calls.clone()))
        .await
        .is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    // Between the two candidate deadlines: a 2 s window (count lost) would
    // have expired, the doubled 4 s window has not (1 s margin each side).
    tokio::time::sleep(Duration::from_millis(3_000)).await;
    assert!(cache
        .lookup_with(&spec, "real.test", NATIVE, &short, fail(calls.clone()))
        .await
        .is_err());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "3 s into the doubled 4 s back-off the cached failure is served"
    );
}
