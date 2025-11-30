//! Simple load balancing selectors
//!
//! This module provides simple load balancing algorithms for backend selection.

mod weighted_selector;
mod backend_selector;

pub use weighted_selector::WeightedRoundRobin;
pub use backend_selector::{BackendSelector, ERR_NO_BACKEND_REFS, ERR_INCONSISTENT_WEIGHT};

