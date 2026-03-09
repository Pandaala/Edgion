use pingora_load_balancing::Backend;

use super::select_by_min_metric;
use crate::core::gateway::lb::runtime_state;

/// Select the backend with the fewest active connections.
///
/// Returns the first healthy candidate ordered by ascending connection count.
/// Draining/removed backends are filtered via `runtime_state`.
pub fn select(
    backends: &[Backend],
    service_key: &str,
    max_iterations: usize,
    health_filter: impl Fn(&Backend) -> bool,
) -> Option<Backend> {
    select_by_min_metric(
        backends,
        service_key,
        max_iterations,
        |svc, addr| runtime_state::get_count(svc, addr) as u64,
        health_filter,
    )
}
