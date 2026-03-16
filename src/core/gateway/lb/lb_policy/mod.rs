//! Optional load balancing algorithms module
//!
//! Provides additional load balancing algorithms (Consistent, LeastConnection)
//! that can be optionally enabled per service based on configuration.

mod config;
mod policy_store;
mod types;

pub use config::get_policies_for_service;
pub use policy_store::{get_global_policy_store, PolicyStore, PolicyStoreStats};
pub use types::LbPolicy;
