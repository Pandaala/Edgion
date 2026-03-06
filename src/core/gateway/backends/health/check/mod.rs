pub mod annotation;
pub mod config_store;
pub mod manager;
pub mod probes;
pub mod status_store;

use pingora_load_balancing::Backend;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub use config_store::{get_hc_config_store, HealthCheckConfigStore};
pub use manager::{get_health_check_manager, HealthCheckManager};
pub use status_store::{get_health_status_store, HealthStatusStore};

#[inline]
pub(crate) fn backend_hash(backend: &Backend) -> u64 {
    let mut hasher = DefaultHasher::new();
    backend.addr.hash(&mut hasher);
    backend.weight.hash(&mut hasher);
    hasher.finish()
}
