//! Backend selection algorithms
//!
//! This module provides weighted load balancing algorithms for lb selection.

pub mod attempt;
pub mod backend_selector;
pub mod compiled_selector;
pub mod edgion_backend;
pub mod ewma;
pub mod leastconn;
pub mod runtime_state;
pub mod selection;

pub use attempt::{commit_attempt, finalize_attempt, FinalizeMode, LbCommit};
pub use backend_selector::{
    BackendSelector, WeightedRoundRobin, WrrError, ERR_INCONSISTENT_WEIGHT, ERR_NO_BACKEND_REFS, ERR_WRR_BUILD_FAILED,
};
pub use compiled_selector::{
    drop_cell, get_or_create_cell, try_get_cell, CompiledSelector, PolicyKind, ServiceSelectorCell,
};
pub use edgion_backend::EdgionBackend;
