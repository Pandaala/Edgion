//! EWMA (Exponentially Weighted Moving Average) load balancing
//!
//! This module implements EWMA-based backend selection for load balancing.
//! It tracks response latency using exponential smoothing and selects backends
//! with the lowest EWMA response time.
//!
//! # Usage
//!
//! ```ignore
//! use edgion::core::gateway::lb::ewma;
//!
//! // Update EWMA after receiving response
//! ewma::update(&backend_addr, latency_us);
//!
//! // Get current EWMA value
//! let ewma_value = ewma::get_ewma(&backend_addr);
//!
//! // Configure alpha parameter (0-100, default 20 = 0.2)
//! ewma::set_alpha(30); // More responsive to recent changes
//! ```

pub mod metrics;
mod selection;

// Re-export public APIs
pub use metrics::{get_alpha, get_ewma, remove, set_alpha, update};
pub use selection::Ewma;
