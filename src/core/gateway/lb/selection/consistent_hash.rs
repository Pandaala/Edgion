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
