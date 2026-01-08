//! Simple load balancing selectors
//!
//! This module provides simple load balancing algorithms for backend selection.

mod selector;
mod weighted_selector;

pub use selector::{BackendSelector, ERR_INCONSISTENT_WEIGHT, ERR_NO_BACKEND_REFS};
pub use weighted_selector::WeightedRoundRobin;
