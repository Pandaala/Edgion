use super::*;
use pingora_core::protocols::l4::socket::SocketAddr as PSocketAddr;
use std::net::{IpAddr, Ipv4Addr, SocketAddr as StdSocketAddr};

fn make_backend(i: u32) -> Backend {
    let octets = i.to_be_bytes();
    let ip = IpAddr::V4(Ipv4Addr::new(10, octets[1], octets[2], octets[3]));
    Backend {
        addr: PSocketAddr::Inet(StdSocketAddr::new(ip, 80)),
        weight: 1,
        ext: Default::default(),
    }
}

#[test]
fn empty_backend_list_returns_none() {
    let ring = ConsistentHashRing::build(&[]);
    assert!(ring.select(b"any-key", 16, |_| true).is_none());
}

#[test]
fn same_key_always_maps_to_same_backend() {
    let backends: Vec<Backend> = (1..=8).map(make_backend).collect();
    let ring = ConsistentHashRing::build(&backends);
    let key = b"user-session-42";
    let first = ring.select(key, 16, |_| true).expect("healthy backend");
    for _ in 0..10 {
        let next = ring.select(key, 16, |_| true).expect("healthy backend");
        assert_eq!(next.addr, first.addr, "ketama must be deterministic for the same key");
    }
}

#[test]
fn different_keys_can_pick_different_backends() {
    // With 8 backends and 32 keys, key->backend distribution should not
    // collapse onto a single bucket. We assert at least 2 distinct backends
    // get selected, which is statistically near-certain for a healthy ring.
    let backends: Vec<Backend> = (1..=8).map(make_backend).collect();
    let ring = ConsistentHashRing::build(&backends);
    let mut seen = std::collections::HashSet::new();
    for i in 0..32u32 {
        let key = format!("key-{}", i);
        let picked = ring.select(key.as_bytes(), 16, |_| true).expect("healthy");
        seen.insert(picked.addr.clone());
    }
    assert!(
        seen.len() >= 2,
        "ring with 8 backends + 32 keys must pick more than one backend, got {}",
        seen.len()
    );
}

#[test]
fn unhealthy_primary_falls_back_to_next_on_ring() {
    let backends: Vec<Backend> = (1..=4).map(make_backend).collect();
    let ring = ConsistentHashRing::build(&backends);
    let key = b"hot-key";
    let primary = ring.select(key, 32, |_| true).expect("healthy backend").addr.clone();
    // Mark the primary unhealthy; ring walk should return some other backend.
    let fallback = ring
        .select(key, 32, |b| b.addr != primary)
        .expect("fallback must succeed when others are healthy");
    assert_ne!(fallback.addr, primary, "fallback must not return the unhealthy primary");
}

#[test]
fn all_unhealthy_returns_none() {
    let backends: Vec<Backend> = (1..=4).map(make_backend).collect();
    let ring = ConsistentHashRing::build(&backends);
    assert!(ring.select(b"key", 32, |_| false).is_none());
}

#[test]
fn high_weight_unhealthy_backend_does_not_starve_healthy_peers() {
    // A high-weight unhealthy backend places ~160*weight virtual nodes on the
    // ring. Only distinct-backend attempts consume the iteration budget, so a
    // budget equal to the distinct-backend count always suffices to reach a
    // healthy peer.
    let mut backends: Vec<Backend> = (1..=4).map(make_backend).collect();
    backends[0].weight = 200; // dominates ~98% of the ring slots
    let heavy_addr = backends[0].addr.clone();
    let ring = ConsistentHashRing::build(&backends);

    // With a budget equal to the number of distinct backends, the walk must
    // still fall back to a healthy peer no matter how the heavy backend's
    // duplicate vnodes cluster around the key.
    for i in 0..32u32 {
        let key = format!("f004-key-{i}");
        let picked = ring
            .select(key.as_bytes(), backends.len(), |b| b.addr != heavy_addr)
            .expect("must fall back to a healthy peer within the distinct-backend budget");
        assert_ne!(picked.addr, heavy_addr, "must not return the unhealthy heavy backend");
    }
}

#[test]
fn all_unhealthy_with_heavy_backend_terminates() {
    // Termination guard: `Continuum::get_addr` never returns None on a
    // non-empty ring, so the walk could spin forever without the
    // distinct-backend break. A budget far larger than the backend count
    // ensures it is the break (not the budget cap) that ends the walk;
    // if the guard were missing this test would hang.
    let mut backends: Vec<Backend> = (1..=4).map(make_backend).collect();
    backends[0].weight = 200;
    let ring = ConsistentHashRing::build(&backends);
    assert!(ring.select(b"f004-key", 1_000_000, |_| false).is_none());
}

#[test]
fn huge_weight_is_clamped_and_ring_stays_bounded() {
    // Defense-in-depth: a weight that would overflow the `usize -> u32` cast
    // (silently truncating) and explode the ~160*weight vnode count must be
    // clamped. Build must not OOM/panic and selection must still succeed.
    // (Today all weights are 1; this guards the day per-endpoint weights land.)
    let mut backends: Vec<Backend> = (1..=3).map(make_backend).collect();
    backends[0].weight = usize::MAX;
    let ring = ConsistentHashRing::build(&backends);
    assert!(
        ring.select(b"clamp-key", 8, |_| true).is_some(),
        "ring built from a clamped huge weight must still select a backend"
    );
}

#[test]
fn adding_backend_keeps_majority_of_keys_stable() {
    // Consistent-hashing guarantee: adding 1 backend to N must move at most
    // ~1/(N+1) of keys to the new backend (in expectation). We assert a
    // generous upper bound — most keys keep their owner.
    let n = 8u32;
    let before: Vec<Backend> = (1..=n).map(make_backend).collect();
    let mut after = before.clone();
    after.push(make_backend(99)); // distinct address (10.0.0.99)

    let ring_before = ConsistentHashRing::build(&before);
    let ring_after = ConsistentHashRing::build(&after);

    let mut moved = 0usize;
    let total = 200usize;
    for i in 0..total {
        let key = format!("k-{i}");
        let a = ring_before.select(key.as_bytes(), 16, |_| true).unwrap().addr.clone();
        let b = ring_after.select(key.as_bytes(), 16, |_| true).unwrap().addr.clone();
        if a != b {
            moved += 1;
        }
    }

    // Strict ketama bound is ~1/(N+1) ≈ 11%. We allow a generous 40% tail
    // for small samples; most keys must remain on the same backend.
    let upper = total * 40 / 100;
    assert!(
        moved <= upper,
        "adding one backend moved {} of {} keys (allowed ≤ {})",
        moved,
        total,
        upper
    );
}

#[test]
fn removing_backend_only_redistributes_its_keys() {
    // After removing backend X, only keys previously owned by X should
    // change destination. Other keys must remain on their original backend.
    let n = 8u32;
    let before: Vec<Backend> = (1..=n).map(make_backend).collect();
    let removed_addr = before[3].addr.clone();
    let after: Vec<Backend> = before.iter().filter(|b| b.addr != removed_addr).cloned().collect();

    let ring_before = ConsistentHashRing::build(&before);
    let ring_after = ConsistentHashRing::build(&after);

    let total = 200usize;
    for i in 0..total {
        let key = format!("k-{i}");
        let a = ring_before.select(key.as_bytes(), 16, |_| true).unwrap().addr.clone();
        let b = ring_after.select(key.as_bytes(), 16, |_| true).unwrap().addr.clone();
        if a != removed_addr {
            assert_eq!(
                a, b,
                "key {} previously on {:?} must keep its backend after removal",
                key, a
            );
        } else {
            assert_ne!(b, removed_addr, "removed backend must not appear after deletion");
        }
    }
}

#[cfg(unix)]
#[test]
fn non_inet_addrs_are_skipped_in_ring() {
    use std::os::unix::net::SocketAddr as StdUnixSockAddr;
    use std::path::Path;

    // Mix Inet + Unix socket addrs; only Inet should make it into the ring.
    let mut backends = vec![make_backend(1), make_backend(2)];
    let unix_addr = StdUnixSockAddr::from_pathname(Path::new("/tmp/edgion-lb-ut.sock")).unwrap();
    backends.push(Backend {
        addr: PSocketAddr::Unix(unix_addr),
        weight: 1,
        ext: Default::default(),
    });
    let ring = ConsistentHashRing::build(&backends);
    // Only Inet backends should ever be returned.
    for i in 0..16u32 {
        let key = format!("k-{i}");
        let picked = ring.select(key.as_bytes(), 16, |_| true).expect("healthy");
        assert!(matches!(picked.addr, PSocketAddr::Inet(_)));
    }
}

#[cfg(unix)]
#[test]
fn preparation_caps_inet_backends_and_preserves_original_indices() {
    use std::os::unix::net::SocketAddr as StdUnixSockAddr;
    use std::path::Path;

    let unix_addr = StdUnixSockAddr::from_pathname(Path::new("/tmp/edgion-ch-cap-ut.sock")).unwrap();
    let mut backends = vec![Backend {
        addr: PSocketAddr::Unix(unix_addr),
        weight: 1,
        ext: Default::default(),
    }];
    backends.extend((1..=(MAX_CH_BACKENDS as u32 + 1)).map(make_backend));

    let (buckets, addr_to_idx) = prepare_ring(&backends);
    let PSocketAddr::Inet(first_addr) = backends[1].addr else {
        unreachable!("test backend must be Inet");
    };
    let PSocketAddr::Inet(last_addr) = backends[MAX_CH_BACKENDS].addr else {
        unreachable!("test backend must be Inet");
    };
    let PSocketAddr::Inet(excluded_addr) = backends[MAX_CH_BACKENDS + 1].addr else {
        unreachable!("test backend must be Inet");
    };

    assert_eq!(buckets.len(), MAX_CH_BACKENDS);
    assert_eq!(addr_to_idx.len(), MAX_CH_BACKENDS);
    assert_eq!(buckets.first(), Some(&Bucket::new(first_addr, 1)));
    assert_eq!(buckets.last(), Some(&Bucket::new(last_addr, 1)));
    assert_eq!(addr_to_idx.get(&first_addr), Some(&1));
    assert_eq!(addr_to_idx.get(&last_addr), Some(&MAX_CH_BACKENDS));
    assert!(!addr_to_idx.contains_key(&excluded_addr));
}

#[test]
fn weight_increases_share_of_ring() {
    // A backend with weight=10 plus several weight=1 peers should win the
    // majority of keys. This is a sanity check that weights propagate
    // through to ketama bucket sizing.
    let mut backends = vec![make_backend(1), make_backend(2), make_backend(3)];
    backends[0].weight = 10;
    let ring = ConsistentHashRing::build(&backends);

    let heavy_addr = backends[0].addr.clone();
    let mut heavy_hits = 0usize;
    let total = 400usize;
    for i in 0..total {
        let key = format!("k-{i}");
        let picked = ring.select(key.as_bytes(), 16, |_| true).unwrap();
        if picked.addr == heavy_addr {
            heavy_hits += 1;
        }
    }
    // 10/12 share ≈ 83%. Allow a wide window for hash variance.
    assert!(
        heavy_hits > total / 2,
        "heavy backend (w=10) should win majority, got {}/{}",
        heavy_hits,
        total
    );
}

#[test]
fn large_backend_count_uses_vec_seen_path() {
    // >128 backends forces SeenSet::Large (the Vec<bool> fallback). Verify
    // selection stays correct and deterministic on that branch, and that
    // the all-unhealthy walk still terminates with None (exercising both
    // the newly-inserted and already-seen arms of the Large bitset).
    let backends: Vec<Backend> = (1..=200).map(make_backend).collect();
    let ring = ConsistentHashRing::build(&backends);

    let key = b"large-fleet-key";
    let first = ring.select(key, 256, |_| true).expect("healthy backend");
    for _ in 0..10 {
        let next = ring.select(key, 256, |_| true).expect("healthy backend");
        assert_eq!(next.addr, first.addr, "ketama must be deterministic for the same key");
    }

    assert!(
        ring.select(key, 256, |_| false).is_none(),
        "all-unhealthy walk over a large fleet must terminate with None"
    );
}

#[test]
fn primary_and_fallback_borrow_ring_storage() {
    let ring = ConsistentHashRing::build(&[make_backend(1), make_backend(2)]);
    let primary = ring.select(b"borrowed-owner", 2, |_| true).unwrap();
    let primary_slot = ring
        .backends
        .iter()
        .find(|backend| backend.addr == primary.addr)
        .unwrap();
    assert!(std::ptr::eq(primary, primary_slot));
    let fallback = ring
        .select(b"borrowed-owner", 2, |candidate| candidate.addr != primary.addr)
        .unwrap();
    let fallback_slot = ring
        .backends
        .iter()
        .find(|backend| backend.addr == fallback.addr)
        .unwrap();
    assert_ne!(primary.addr, fallback.addr);
    assert!(std::ptr::eq(fallback, fallback_slot));
}
