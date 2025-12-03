//! Backend selection algorithms
//!
//! This module provides weighted load balancing algorithms for lb selection.

pub mod simple_lb;
pub mod lb_policy;

pub use simple_lb::{BackendSelector, WeightedRoundRobin, ERR_NO_BACKEND_REFS, ERR_INCONSISTENT_WEIGHT};

