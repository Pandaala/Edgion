//! Common plugin structures and utilities
//!
//! This module provides shared types and utilities for plugin conditions,
//! enabling conditional execution of plugins based on various criteria.

mod evaluator;
mod types;

pub use evaluator::{ConditionEvalResult, EvaluationResult};
pub use types::{
    Condition, ExcludeCondition, IncludeCondition, KeyExistCondition, KeyMatchCondition, PluginConditions,
    ProbabilityCondition, TimeRangeCondition,
};
