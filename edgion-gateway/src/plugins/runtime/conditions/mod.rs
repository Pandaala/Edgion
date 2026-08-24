//! Common plugin structures and utilities
//!
//! This module provides shared types and utilities for plugin conditions,
//! enabling conditional execution of plugins based on various criteria.

mod evaluator;
mod extract;

pub use evaluator::{ConditionEvalResult, EvaluationResult};
pub use extract::{extract_jwt_claim_values, extract_mtls_values};
// Gateway-internal evaluation extension traits (crate-private).
pub(crate) use evaluator::{CompiledCondition, CompiledPluginConditions, ConditionExt, PluginConditionsExt};
// Condition config types live in the `edgion-resources` schema crate.
// Re-exported here so `runtime::conditions::*` consumers need no import change.
pub use edgion_resources::resources::edgion_plugins::conditions::{
    Condition, JwtClaimMatchCondition, KeyExistCondition, KeyMatchCondition, MtlsField, MtlsMatchCondition,
    PluginConditions, ProbabilityCondition, SanType, TimeRangeCondition,
};
