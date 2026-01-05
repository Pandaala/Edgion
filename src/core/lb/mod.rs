//! Backend selection algorithms
//!
//! This module provides weighted load balancing algorithms for lb selection.

pub mod backend_selector;
pub mod ewma;
pub mod lb_policy;
pub mod leastconn;

pub use backend_selector::{BackendSelector, WeightedRoundRobin, ERR_INCONSISTENT_WEIGHT, ERR_NO_BACKEND_REFS};
