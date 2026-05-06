//! LeastConnection load balancing algorithm
//!
//! Service-scoped connection tracking and selection now live in
//! `lb::runtime_state` and `lb::selection::least_conn` respectively.
//! This module owns the background cleaner.

mod cleaner;

pub use cleaner::BackendCleaner;
