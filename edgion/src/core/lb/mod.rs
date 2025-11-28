//! Backend selection algorithms
//!
//! This module provides weighted load balancing algorithms for lb selection.

mod weighted_selector;

pub use weighted_selector::WeightedRoundRobin;

