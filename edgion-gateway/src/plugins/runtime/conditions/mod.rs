//! Common plugin structures and utilities
//!
//! This module provides shared types and utilities for plugin conditions,
//! enabling conditional execution of plugins based on various criteria.

mod evaluator;

pub use evaluator::{ConditionEvalResult, EvaluationResult};
// Gateway-internal evaluation extension traits (crate-private).
pub(crate) use evaluator::{ConditionExt, PluginConditionsExt};
// Condition config types live in the `edgion-resources` schema crate.
// Re-exported here so `runtime::conditions::*` consumers need no import change.
pub use edgion_resources::resources::edgion_plugins::conditions::{
    Condition, ExcludeCondition, IncludeCondition, KeyExistCondition, KeyMatchCondition, PluginConditions,
    ProbabilityCondition, TimeRangeCondition,
};
