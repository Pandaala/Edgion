//! LeastConnection load balancing algorithm
//!
//! Per-backend connection tracking lives on `LbBackend`. Selection is
//! performed by `lb::selection::p2c`. This module owns the background cleaner.

mod cleaner;

pub use cleaner::BackendCleaner;

#[cfg(test)]
#[path = "../../../tests/unit/lb/leastconn/mod_test.rs"]
mod tests;
