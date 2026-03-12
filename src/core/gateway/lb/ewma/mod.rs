//! EWMA (Exponentially Weighted Moving Average) load balancing
//!
//! The service-scoped EWMA latency tracking and selection live in
//! `lb::runtime_state` and `lb::selection::ewma` respectively.
//! This module only owns the global alpha (smoothing factor) parameter.

pub mod metrics;

pub use metrics::{get_alpha, set_alpha};
