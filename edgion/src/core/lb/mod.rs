//! Backend selection algorithms
//!
//! This module provides weighted load balancing algorithms for lb selection.

mod weighted_selector;
mod backend_selector;
pub mod optional_lb;

pub use weighted_selector::WeightedRoundRobin;
pub use backend_selector::{BackendSelector, ERR_NO_BACKEND_REFS, ERR_INCONSISTENT_WEIGHT};

