use super::*;
use metrics_exporter_prometheus::PrometheusBuilder;
use pingora_core::protocols::l4::socket::SocketAddr as PSocketAddr;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

fn make_backend(i: u32) -> Backend {
    let octets = i.to_be_bytes();
    let ip = IpAddr::V4(Ipv4Addr::new(10, octets[1], octets[2], octets[3]));
    Backend {
        addr: PSocketAddr::Inet(SocketAddr::new(ip, 80)),
        weight: 1,
        ext: Default::default(),
    }
}

fn make_backend_weighted(i: u32, weight: usize) -> Backend {
    let mut b = make_backend(i);
    b.weight = weight;
    b
}

#[test]
fn build_truncates_over_u16_max() {
    let backends: Vec<Backend> = (0..(u16::MAX as u32 + 2)).map(make_backend).collect();
    let selector = RoundRobinSelector::build(&backends);
    assert_eq!(selector.backend_count(), u16::MAX as usize);
}

#[test]
fn truncation_increments_counter() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        let backends: Vec<Backend> = (0..(u16::MAX as u32 + 1)).map(make_backend).collect();
        let _ = RoundRobinSelector::build(&backends);
        let rendered = handle.render();
        assert!(
            rendered.contains("edgion_rr_selector_truncated_total"),
            "counter must appear on truncation, got:\n{rendered}"
        );
    });
}

#[test]
fn empty_backend_list_returns_none() {
    let selector = RoundRobinSelector::build(&[]);
    assert!(selector.select(8, |_| true).is_none());
    assert_eq!(selector.backend_count(), 0);
}

#[test]
fn single_backend_returns_it_when_healthy() {
    let backends = vec![make_backend(1)];
    let selector = RoundRobinSelector::build(&backends);
    for _ in 0..5 {
        let picked = selector.select(8, |_| true).expect("healthy backend");
        assert_eq!(picked.addr, backends[0].addr);
    }
}

#[test]
fn single_backend_returns_none_when_unhealthy() {
    let backends = vec![make_backend(1)];
    let selector = RoundRobinSelector::build(&backends);
    assert!(selector.select(8, |_| false).is_none());
}

#[test]
fn distribution_uniform_for_equal_weights() {
    // N equal-weight backends, N consecutive selections must touch each
    // backend exactly once thanks to the deterministic counter.
    let n = 5u32;
    let backends: Vec<Backend> = (1..=n).map(make_backend).collect();
    let selector = RoundRobinSelector::build(&backends);

    let mut counts: HashMap<PSocketAddr, usize> = HashMap::new();
    for _ in 0..n {
        let picked = selector.select(8, |_| true).expect("healthy backend");
        *counts.entry(picked.addr).or_default() += 1;
    }
    assert_eq!(counts.len(), n as usize, "each backend must be picked once");
    for (_, c) in counts {
        assert_eq!(c, 1);
    }
}

#[test]
fn distribution_respects_weights() {
    // Weights 1:2:5 => over 8 calls each backend appears `weight` times.
    let backends = vec![
        make_backend_weighted(1, 1),
        make_backend_weighted(2, 2),
        make_backend_weighted(3, 5),
    ];
    let selector = RoundRobinSelector::build(&backends);

    let total: usize = backends.iter().map(|b| b.weight).sum();
    let mut counts: HashMap<PSocketAddr, usize> = HashMap::new();
    for _ in 0..total {
        let picked = selector.select(8, |_| true).expect("healthy backend");
        *counts.entry(picked.addr).or_default() += 1;
    }
    // The selector sorts by addr; addresses 10.0.0.1/2/3 keep order.
    assert_eq!(counts.get(&backends[0].addr).copied().unwrap_or(0), 1);
    assert_eq!(counts.get(&backends[1].addr).copied().unwrap_or(0), 2);
    assert_eq!(counts.get(&backends[2].addr).copied().unwrap_or(0), 5);
}

#[test]
fn zero_weight_treated_as_one() {
    // weight = 0 should be normalized to 1, so the backend still appears.
    let backends = vec![make_backend_weighted(1, 0), make_backend_weighted(2, 0)];
    let selector = RoundRobinSelector::build(&backends);

    let mut seen: HashMap<PSocketAddr, usize> = HashMap::new();
    for _ in 0..4 {
        let picked = selector.select(8, |_| true).expect("healthy backend");
        *seen.entry(picked.addr).or_default() += 1;
    }
    assert_eq!(seen.len(), 2, "both zero-weight backends must rotate in");
}

#[test]
fn all_unhealthy_returns_none() {
    let backends: Vec<Backend> = (1..=4).map(make_backend).collect();
    let selector = RoundRobinSelector::build(&backends);
    assert!(selector.select(16, |_| false).is_none());
}

#[test]
fn fallback_walks_to_lone_healthy_backend() {
    let backends: Vec<Backend> = (1..=8).map(make_backend).collect();
    let selector = RoundRobinSelector::build(&backends);
    let healthy = backends[5].addr.clone();
    let picked = selector
        .select(16, |b| b.addr == healthy)
        .expect("must locate the lone healthy backend");
    assert_eq!(picked.addr, healthy);
}

#[test]
fn weighted_fallback_starts_after_actual_primary() {
    use std::cell::Cell;

    let backends = vec![
        make_backend_weighted(1, 3),
        make_backend_weighted(2, 1),
        make_backend_weighted(3, 1),
    ];
    let selector = RoundRobinSelector::build(&backends);

    // Advance to cursor 2. Its weighted primary is still backend 0, while
    // cursor % backend_count is 2. Anchoring fallback to the raw cursor
    // would recheck backend 0 and miss healthy backend 1 within two tries.
    for _ in 0..2 {
        selector.select(1, |_| true).expect("warm-up pick must succeed");
    }

    let primary = backends[0].addr.clone();
    let healthy = backends[1].addr.clone();
    let primary_checks = Cell::new(0usize);
    let picked = selector
        .select(2, |backend| {
            if backend.addr == primary {
                primary_checks.set(primary_checks.get() + 1);
            }
            backend.addr == healthy
        })
        .expect("fallback must reach the next backend after the weighted primary");

    assert_eq!(picked.addr, healthy);
    assert_eq!(primary_checks.get(), 1, "fallback must not recheck the primary");
}

#[test]
fn max_iterations_caps_fallback_search() {
    // With max_iterations = 1 the fallback loop body never runs (the
    // condition is `1..max_iterations.min(blen)` which is empty), so the
    // lone healthy backend must NOT be reached when the first weighted
    // pick is unhealthy.
    let backends: Vec<Backend> = (1..=4).map(make_backend).collect();
    let selector = RoundRobinSelector::build(&backends);
    let healthy = backends[2].addr.clone();
    // Force the first counter pick to land on a non-healthy backend by
    // exhausting until counter % 4 == 0 (backends[0]).
    // counter starts at 0 → idx 0 (unhealthy). Fallback does nothing.
    let picked = selector.select(1, |b| b.addr == healthy);
    assert!(
        picked.is_none(),
        "max_iterations=1 must skip fallback walk, got {:?}",
        picked
    );
}

#[test]
fn huge_per_backend_weight_is_capped_and_light_backend_survives() {
    // Defense-in-depth: an adversarially large per-backend weight must not
    // expand the `weighted` index array without bound. With the cap, build
    // must not OOM and every backend must keep at least one slot — proven by
    // the fallback walk still locating the light backend when the heavy one
    // is rejected. (Today all weights are 1; this guards the future.)
    let backends = vec![make_backend_weighted(1, 100_000_000), make_backend_weighted(2, 1)];
    let light = backends[1].addr.clone();
    let selector = RoundRobinSelector::build(&backends);
    let picked = selector
        .select(8, |b| b.addr == light)
        .expect("light backend must remain reachable after weight capping");
    assert_eq!(picked.addr, light);
}

#[test]
fn common_factor_weights_are_reduced() {
    // GCD reduction: weights [100, 100] collapse to [1, 1], so the index
    // array length is 2 (not 200) and two consecutive selections touch each
    // backend once. Without the reduction both selections would land on the
    // first backend's run of duplicate slots.
    let backends = vec![make_backend_weighted(1, 100), make_backend_weighted(2, 100)];
    let selector = RoundRobinSelector::build(&backends);
    let mut counts: HashMap<PSocketAddr, usize> = HashMap::new();
    for _ in 0..2 {
        let picked = selector.select(8, |_| true).expect("healthy backend");
        *counts.entry(picked.addr).or_default() += 1;
    }
    assert_eq!(
        counts.len(),
        2,
        "equal weights must round-robin one-each regardless of magnitude"
    );
}

#[test]
fn extreme_coprime_weights_stay_bounded_without_overflow() {
    let normalized = normalize_rr_weights(vec![usize::MAX, usize::MAX - 1]);

    assert_eq!(normalized.len(), 2);
    assert!(normalized.iter().all(|&weight| weight > 0));
    assert!(normalized.iter().sum::<usize>() <= MAX_RR_WEIGHTED_LEN);
}

#[test]
fn backends_accessor_returns_sorted_unique_list() {
    // Build with shuffled input; ensure backends() is sorted by addr.
    let mut backends: Vec<Backend> = (1..=4).map(make_backend).collect();
    backends.swap(0, 3);
    backends.swap(1, 2);
    let selector = RoundRobinSelector::build(&backends);
    let stored = selector.backends();
    assert_eq!(stored.len(), 4);
    for w in stored.windows(2) {
        assert!(w[0].addr <= w[1].addr, "backends must be sorted by addr");
    }
}
