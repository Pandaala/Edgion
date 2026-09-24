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
/// Mirrors the WeightedRoundRobin weight cap.
const MAX_CH_BACKEND_WEIGHT: usize = 1 << 10;

/// Keep consistent-hash construction within the same backend-count boundary as
/// round robin. The original backend indices remain stable for health lookups.
const MAX_CH_BACKENDS: usize = u16::MAX as usize;

fn prepare_ring(backends: &[Backend]) -> (Vec<Bucket>, HashMap<std::net::SocketAddr, usize>) {
    let prepared: Vec<_> = backends
        .iter()
        .enumerate()
        .filter_map(|(original_index, backend)| {
            let SocketAddr::Inet(addr) = backend.addr else {
                return None;
            };
            Some((
                original_index,
                addr,
                backend.weight.clamp(1, MAX_CH_BACKEND_WEIGHT) as u32,
            ))
        })
        .take(MAX_CH_BACKENDS)
        .collect();

    let buckets = prepared
        .iter()
        .map(|&(_, addr, weight)| Bucket::new(addr, weight))
        .collect();
    let addr_to_idx = prepared
        .into_iter()
        .map(|(original_index, addr, _)| (addr, original_index))
        .collect();
    (buckets, addr_to_idx)
}

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
        let (buckets, addr_to_idx) = prepare_ring(backends);
        let ring = Continuum::new(&buckets);
        let backends_vec: Vec<Backend> = backends.to_vec();

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
#[path = "../../../tests/unit/lb/selection/consistent_hash_test.rs"]
mod tests;
