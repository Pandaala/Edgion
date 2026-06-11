//! Common plugin structures and utilities
//!
//! This module provides shared types and utilities for plugin conditions,
//! enabling conditional execution of plugins based on various criteria.

mod evaluator;

pub use evaluator::{ConditionEvalResult, EvaluationResult};
// Condition config types now live in the `types` schema layer (leaf crate).
// Re-export them here so existing `runtime::conditions::*` consumers are unaffected.
pub use crate::types::resources::edgion_plugins::conditions::{
    Condition, ExcludeCondition, IncludeCondition, KeyExistCondition, KeyMatchCondition, PluginConditions,
    ProbabilityCondition, TimeRangeCondition,
};
