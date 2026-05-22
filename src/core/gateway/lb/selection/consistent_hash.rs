use pingora_core::protocols::l4::socket::SocketAddr;
use pingora_ketama::{Bucket, Continuum};
use pingora_load_balancing::Backend;
use std::collections::{HashMap, HashSet};

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
                    Some(Bucket::new(addr, b.weight.max(1) as u32))
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
        let mut seen = HashSet::new();
        let mut steps = 0;

        loop {
            if steps >= max_iterations {
                break;
            }

            let Some(addr) = self.ring.get_addr(&mut idx) else {
                break;
            };

            steps += 1;

            if let Some(&backend_idx) = self.addr_to_idx.get(addr) {
                if seen.contains(&backend_idx) {
                    continue;
                }
                seen.insert(backend_idx);

                let backend = &self.backends[backend_idx];
                if health_filter(backend) {
                    return Some(backend.clone());
                }
            }
        }

        None
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
}
