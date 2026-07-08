//! Optional load balancing algorithms module
//!
//! Provides additional load balancing algorithms (Consistent, LeastConnection)
//! that can be optionally enabled per service based on configuration.

mod policy_store;
mod types;

pub use policy_store::{get_global_policy_store, PolicyStore, PolicyStoreStats};
pub use types::LbPolicy;
