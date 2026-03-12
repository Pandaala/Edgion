pub mod consistent_hash;
pub mod ewma;
pub mod least_conn;
pub mod round_robin;

use pingora_core::protocols::l4::socket::SocketAddr;
use pingora_load_balancing::Backend;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// Shared min-metric selection used by both LeastConn and EWMA.
///
/// Builds a min-heap over `metric_fn(service_key, &backend.addr)` for all active
/// backends, then pops candidates until `health_filter` accepts one.
/// Each backend is pushed at most once, so no dedup is needed on pop.
pub fn select_by_min_metric(
    backends: &[Backend],
    service_key: &str,
    max_iterations: usize,
    metric_fn: impl Fn(&str, &SocketAddr) -> u64,
    health_filter: impl Fn(&Backend) -> bool,
) -> Option<Backend> {
    use super::runtime_state;

    let mut heap = BinaryHeap::with_capacity(backends.len());

    for (i, backend) in backends.iter().enumerate() {
        if !runtime_state::is_backend_active(service_key, &backend.addr) {
            continue;
        }
        let metric = metric_fn(service_key, &backend.addr);
        heap.push(Reverse((metric, i)));
    }

    let mut steps = 0;
    while let Some(Reverse((_, idx))) = heap.pop() {
        if steps >= max_iterations {
            break;
        }
        steps += 1;

        let backend = &backends[idx];
        if health_filter(backend) {
            return Some(backend.clone());
        }
    }

    None
}
