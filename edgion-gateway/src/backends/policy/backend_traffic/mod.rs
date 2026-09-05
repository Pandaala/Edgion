//! `EdgionBackendTrafficPolicy` gateway data-plane wiring.
//!
//! - `store` — synced policy cache + oldest-wins arbitration; pushes the
//!   effective config into the health-check, circuit-breaker, degrade-threshold
//!   and load-balancer stores.
//! - `conf_handler_impl` — the `ConfHandler` that receives synced policies.

pub mod authority;
pub mod conf_handler_impl;
mod retry_budget;
pub mod store;

pub use conf_handler_impl::create_backend_traffic_policy_handler;
pub use retry_budget::RetryBudget;
pub use store::{
    get_backend_traffic_policy_store, BackendTrafficPolicySnapshot, BackendTrafficPolicyStore,
    EffectiveBackendTrafficPolicy,
};

use edgion_resources::resources::lb_policy::ParsedLBPolicy;

/// Admin/test helper: current snapshot LB policy. Not a proxy hot-path read.
pub fn current_lb_policy_for_admin(service_key: &str) -> Option<ParsedLBPolicy> {
    get_backend_traffic_policy_store().get_lb_policy(service_key)
}
