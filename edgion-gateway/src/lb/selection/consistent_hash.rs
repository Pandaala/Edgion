use pingora_core::protocols::l4::socket::SocketAddr;
use pingora_ketama::{Bucket, Continuum};
use pingora_load_balancing::Backend;
use std::collections::HashMap;

/// Upper bound on a single backend's ketama weight.
///
/// Each unit of weight expands into ~160 virtual nodes on the ring, and the
/// value is cast `usize -> u32` before reaching `Bucket::new`. Per-endpoint
/// weights are hard-coded to 1 today, so the clamp is a no-op; it is
/// defense-in-depth for the day per-endpoint weights are introduced, when an
/// unclamped value would (a) silently truncate on the cast and (b) expand into
/// a multi-GB ring. A 1024:1 ratio is finer than any realistic backend split.
/// Mirrors the WeightedRoundRobin weight cap (lb-F005).
const MAX_CH_BACKEND_WEIGHT: usize = 1 << 10;

/// Consistent hash ring wrapping `pingora_ketama::Continuum`.
///
/// Built from a backend list; walks the ring clockwise for fallback.
pub struct ConsistentHashRing {
    ring: Continuum,
    backends: Vec<Backend>,
    addr_to_idx: HashMap<std::net::SocketAddr, usize>,
}

impl ConsistentHashRing {
    pub fn build(backends: &[Backend]) -> Self {
        let buckets: Vec<_> = backends
            .iter()
            .filter_map(|b| {
                if let SocketAddr::Inet(addr) = b.addr {
                    Some(Bucket::new(addr, b.weight.clamp(1, MAX_CH_BACKEND_WEIGHT) as u32))
                } else {
                    None
                }
            })
            .collect();

        let ring = Continuum::new(&buckets);
        let backends_vec: Vec<Backend> = backends.to_vec();
        let addr_to_idx: HashMap<_, _> = backends_vec
            .iter()
            .enumerate()
            .filter_map(|(i, b)| {
                if let SocketAddr::Inet(addr) = b.addr {
                    Some((addr, i))
                } else {
                    None
                }
            })
            .collect();

        ConsistentHashRing {
            ring,
            backends: backends_vec,
            addr_to_idx,
        }
    }

    /// Select a backend for the given hash key, walking the ring on rejection.
    pub fn select(
        &self,
        hash_key: &[u8],
        max_iterations: usize,
        health_filter: impl Fn(&Backend) -> bool,
    ) -> Option<Backend> {
        if self.backends.is_empty() {
            return None;
        }

        let mut idx = self.ring.node_idx(hash_key);
        let mut seen = SeenSet::new(self.backends.len());
        // Number of distinct backends reachable on the ring (unique Inet addrs).
        let distinct_backends = self.addr_to_idx.len();
        // Counts distinct-backend attempts, not raw ring steps. Duplicate virtual
        // nodes of an already-seen backend make no health-check progress and must
        // not consume the budget: otherwise a high-weight unhealthy backend whose
        // ~160*weight vnodes cluster near the key can exhaust `max_iterations`
        // before the walk ever reaches a healthy peer. Because `seen.insert`
        // succeeds at most once per backend, `steps` equals the number of
        // distinct backends evaluated so far.
        let mut steps = 0;

        loop {
            if steps >= max_iterations {
                break;
            }

            let Some(addr) = self.ring.get_addr(&mut idx) else {
                break;
            };

            if let Some(&backend_idx) = self.addr_to_idx.get(addr) {
                if !seen.insert(backend_idx) {
                    continue;
                }
                steps += 1;

                let backend = &self.backends[backend_idx];
                if health_filter(backend) {
                    return Some(backend.clone());
                }

                // All distinct backends have been evaluated and none is healthy.
                // Break instead of spinning over duplicate vnodes forever:
                // `Continuum::get_addr` never returns `None` on a non-empty ring,
                // so the distinct-backend count is the only progress guarantee.
                if steps >= distinct_backends {
                    break;
                }
            }
        }

        None
    }
}

/// Allocation-free (common case) tracker for backend indices already visited
/// during a ring walk.
///
/// Backend indices are dense (`0..backends.len()`) and real deployments are
/// O(10-100) backends, so a stack `u128` bitmask covers the common case with
/// zero heap cost on the per-request hot path. Larger fleets fall back to a
/// single `Vec<bool>` allocation (no rehashing), which still beats the old
/// per-call `HashSet` that reallocated as it grew.
enum SeenSet {
    /// Bitmask for `backend_count <= 128`; bit `i` set means index `i` seen.
    Bits(u128),
    /// One-shot dense bitset for `backend_count > 128`.
    Large(Vec<bool>),
}

impl SeenSet {
    fn new(backend_count: usize) -> Self {
        if backend_count <= 128 {
            SeenSet::Bits(0)
        } else {
            SeenSet::Large(vec![false; backend_count])
        }
    }

    /// Marks `idx` as seen. Returns `true` if it was newly inserted, `false`
    /// if it had already been seen (mirrors `HashSet::insert` semantics).
    fn insert(&mut self, idx: usize) -> bool {
        match self {
            // `Bits` is only constructed when `backend_count <= 128`, and every
            // `idx` is a valid backend index (`< backend_count <= 128`), so the
            // shift width is always in `0..128` and cannot overflow.
            SeenSet::Bits(bits) => {
                let mask = 1u128 << idx;
                let newly = *bits & mask == 0;
                *bits |= mask;
                newly
            }
            SeenSet::Large(seen) => {
                if seen[idx] {
                    false
                } else {
                    seen[idx] = true;
                    true
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
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
            seen.insert(picked.addr);
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
        let primary = ring.select(key, 32, |_| true).expect("healthy backend").addr;
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
        // Regression for F-004: a high-weight unhealthy backend places ~160*weight
        // virtual nodes on the ring. Previously every duplicate vnode it owned
        // consumed the iteration budget, so the walk could exhaust the budget
        // before reaching a distinct healthy peer and return None. With the fix,
        // only distinct-backend attempts consume the budget, so a budget equal to
        // the distinct-backend count always suffices to find a healthy peer.
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
        // Termination guard for F-004: because `Continuum::get_addr` never yields
        // None on a non-empty ring, counting only distinct-backend attempts could
        // spin forever once every backend has been seen-but-rejected. The
        // distinct-backend break must stop the walk. A budget far larger than the
        // backend count ensures it is the break (not the budget cap) that ends the
        // loop; if the guard were missing this test would hang.
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
            let a = ring_before.select(key.as_bytes(), 16, |_| true).unwrap().addr;
            let b = ring_after.select(key.as_bytes(), 16, |_| true).unwrap().addr;
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
            let a = ring_before.select(key.as_bytes(), 16, |_| true).unwrap().addr;
            let b = ring_after.select(key.as_bytes(), 16, |_| true).unwrap().addr;
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
}
