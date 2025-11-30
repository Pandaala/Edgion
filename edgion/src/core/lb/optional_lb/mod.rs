//! Optional load balancing algorithms module
//! 
//! Provides additional load balancing algorithms (Ketama, FnvHash, LeastConnection)
//! that can be optionally enabled per service based on configuration.

mod types;
mod config;

pub use types::{OptionalLoadBalancers, LbPolicy};
pub use config::get_policies_for_service;

