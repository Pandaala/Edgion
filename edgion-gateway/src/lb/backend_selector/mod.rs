//! Simple load balancing selectors
//!
//! This module provides simple load balancing algorithms for backend selection.

mod selector;
mod weighted_selector;

pub use selector::{BackendSelector, ERR_INCONSISTENT_WEIGHT, ERR_NO_BACKEND_REFS, ERR_WRR_BUILD_FAILED};
pub use weighted_selector::{WeightedRoundRobin, WrrError};

#[cfg(test)]
#[path = "../../../tests/unit/lb/backend_selector/mod_test.rs"]
mod tests;
