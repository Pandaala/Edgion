//! Optional load balancing algorithms module
//! 
//! Provides additional load balancing algorithms (Consistent, FnvHash, LeastConnection)
//! that can be optionally enabled per service based on configuration.

mod types;
mod config;
mod policy_store;

pub use types::{OptionalLoadBalancers, LbPolicy};
pub use config::get_policies_for_service;
pub use policy_store::{PolicyStore, get_global_policy_store};

