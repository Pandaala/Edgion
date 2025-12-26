//! Backend selection algorithms
//!
//! This module provides weighted load balancing algorithms for lb selection.

pub mod leastconn;
pub mod ewma;
pub mod lb_policy;
pub mod backend_selector;

pub use backend_selector::{BackendSelector, WeightedRoundRobin, ERR_NO_BACKEND_REFS, ERR_INCONSISTENT_WEIGHT};

