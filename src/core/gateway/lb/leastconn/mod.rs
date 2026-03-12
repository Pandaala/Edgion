//! LeastConnection load balancing algorithm
//!
//! Service-scoped connection tracking and selection now live in
//! `lb::runtime_state` and `lb::selection::least_conn` respectively.
//! This module owns the `BackendState` enum and the background cleaner.

pub mod backend_state;
mod cleaner;

pub use backend_state::BackendState;
pub use cleaner::BackendCleaner;
