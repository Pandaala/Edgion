//! `EdgionBackendTrafficPolicy` gateway data-plane wiring.
//!
//! - `store` — synced policy cache + oldest-wins arbitration; pushes the
//!   effective config into the health-check, circuit-breaker, panic-threshold
//!   and load-balancer stores.
//! - `conf_handler_impl` — the `ConfHandler` that receives synced policies.

pub mod authority;
pub mod conf_handler_impl;
mod retry_budget;
pub mod store;

pub use conf_handler_impl::create_backend_traffic_policy_handler;
pub use retry_budget::RetryBudget;
pub use store::{get_backend_traffic_policy_store, BackendTrafficPolicyStore, EffectiveBackendTrafficPolicy};

use edgion_resources::resources::lb_policy::ParsedLBPolicy;

/// Resolve the load-balancer policy for a Service key on the request path.
/// Reads the store's LB map (single `DashMap` get + clone) — no tracing here.
pub fn resolve_lb_policy(service_key: &str) -> Option<ParsedLBPolicy> {
    get_backend_traffic_policy_store().get_lb_policy(service_key)
}
