//! Common plugin structures and utilities
//!
//! This module provides shared types and utilities for plugin conditions,
//! enabling conditional execution of plugins based on various criteria.

mod evaluator;

pub use evaluator::{ConditionEvalResult, EvaluationResult};
// Gateway-internal evaluation extension traits (crate-private, like the methods
// they replaced).
pub(crate) use evaluator::{ConditionExt, PluginConditionsExt};
// Condition config types now live in the `types` schema layer (leaf crate).
// Re-export them here so existing `runtime::conditions::*` consumers are unaffected.
pub use edgion_resources::resources::edgion_plugins::conditions::{
    Condition, ExcludeCondition, IncludeCondition, KeyExistCondition, KeyMatchCondition, PluginConditions,
    ProbabilityCondition, TimeRangeCondition,
};
